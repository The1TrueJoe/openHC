import { useEffect, useRef, useState } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Users } from 'lucide-react';
import { io, serialSocket, type Capabilities } from '../api';
import { useIoState } from '../App';

/** Bauds iod will accept. Sent in the capabilities so the list cannot drift
 *  from what the daemon actually supports. */
const FALLBACK = [9600, 19200, 38400, 57600, 115200];

export function SerialPanel({ caps }: { caps: Capabilities }) {
  const ports = caps.serials ?? [];
  const [at, setAt] = useState(0);
  const port = ports[at];
  const state = useIoState();

  return (
    <div className="flex h-full flex-col gap-4">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">Serial</h1>
        <p className="text-sm text-muted">
          iod holds one session per port and every viewer shares it — what you type, others see.
        </p>
      </header>

      <div className="flex flex-wrap gap-2">
        {ports.map((p, i) => {
          const live = state.serial?.[i];
          return (
            <button
              key={i}
              onClick={() => setAt(i)}
              className={`rounded-lg px-3 py-1.5 text-sm transition ${
                at === i ? 'bg-accent/20 text-ink' : 'shade text-muted hover:text-ink'
              }`}
            >
              {p.label || `Port ${i + 1}`}
              <span className="ml-2 text-xs text-muted">{live?.baud ?? p.baud}</span>
            </button>
          );
        })}
      </div>

      {port && port.transport === 'mcu' ? (
        // Not a failure — a different transport. This port has no device node;
        // its bytes travel over the IO protocol's UART opcodes, which iod does
        // not bridge yet. Saying that beats a terminal that silently never
        // receives anything.
        <div className="rounded-xl border border-warm/30 bg-warm/5 p-4 text-sm text-warm">
          <strong>{port.label}</strong> is routed through the IO microcontroller, not a host UART.
          There is no <code>/dev/tty</code> behind it, and iod does not bridge that path yet.
        </div>
      ) : (
        port && (
          <Term
            key={at}
            index={at}
            label={port.label}
            dev={port.dev ?? ''}
            baud={state.serial?.[at]?.baud ?? port.baud}
            viewers={state.serial?.[at]?.viewers ?? 0}
            bauds={caps.bauds ?? FALLBACK}
          />
        )
      )}
    </div>
  );
}

/** Read the page's own tokens so the terminal matches whichever scheme is in
 *  force. Hard-coding a foreground here would give white-on-white in light. */
function termTheme() {
  const s = getComputedStyle(document.documentElement);
  const v = (n: string, fb: string) => s.getPropertyValue(n).trim() || fb;
  return {
    background: 'rgba(0,0,0,0)',
    foreground: v('--ink', '#e8ecf3'),
    cursor: v('--accent', '#4ade80'),
    selectionBackground: v('--edge', 'rgba(128,128,128,0.3)'),
  };
}

function Term({
  index, label, dev, baud, viewers, bauds,
}: {
  index: number; label: string; dev: string; baud: number; viewers: number; bauds: number[];
}) {
  const host = useRef<HTMLDivElement>(null);
  const term = useRef<XTerm | null>(null);
  const [conn, setConn] = useState<'connecting' | 'open' | 'closed'>('connecting');
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    if (!host.current) return;
    const t = new XTerm({
      fontFamily: 'IBM Plex Mono, ui-monospace, Menlo, monospace',
      fontSize: 13,
      cursorBlink: true,
      // Match the panel, not xterm's default black — the terminal is part of
      // the page, not a window sitting on top of it.
      theme: termTheme(),
    });
    term.current = t;
    const fit = new FitAddon();
    t.loadAddon(fit);
    t.open(host.current);
    fit.fit();

    const ws = serialSocket(index);
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => setConn('open');
    ws.onclose = () => setConn('closed');
    // Bytes in both directions, untouched. A terminal that rewrites what it
    // carries is a terminal you cannot type into.
    ws.onmessage = (m) => t.write(typeof m.data === 'string' ? m.data : new Uint8Array(m.data));
    t.onData((d) => ws.readyState === WebSocket.OPEN && ws.send(d));

    const ro = new ResizeObserver(() => { try { fit.fit(); } catch { /* not laid out yet */ } });
    ro.observe(host.current);

    // Follow the system scheme while open, rather than only at mount.
    const scheme = window.matchMedia('(prefers-color-scheme: dark)');
    const repaint = () => { t.options.theme = termTheme(); };
    scheme.addEventListener('change', repaint);

    return () => {
      scheme.removeEventListener('change', repaint);
      ro.disconnect();
      ws.close();
      t.dispose();
      term.current = null;
    };
  }, [index]);

  function changeBaud(next: number) {
    setNote(null);
    try {
      io.setBaud(index, next);
      // Every viewer moves together — the session is shared, so a console at
      // two different bauds is not a thing that can exist. iod announces the
      // reopen on the stream itself, which is why nothing is printed here.
    } catch (e) {
      setNote(String((e as Error).message));
    }
  }

  return (
    <div className="hair flex min-h-0 flex-1 flex-col rounded-xl border bg-panel">
      <div className="hair flex flex-wrap items-center gap-x-4 gap-y-2 border-b px-3 py-2 text-xs">
        <span className="text-muted">
          {label} · <span className="font-mono">{dev}</span>
        </span>

        <label className="flex items-center gap-1.5 text-muted">
          baud
          <select
            value={baud}
            onChange={(e) => changeBaud(Number(e.target.value))}
            className="hair rounded-md border bg-raised px-1.5 py-0.5 text-xs text-ink outline-none focus:border-accent/50"
          >
            {/* If the port is running at something not in the list — set in
                board.env — keep it selectable rather than silently showing the
                wrong rate. */}
            {(bauds.includes(baud) ? bauds : [baud, ...bauds]).map((b) => (
              <option key={b} value={b}>{b}</option>
            ))}
          </select>
        </label>

        {viewers > 1 && (
          // Worth saying plainly: somebody else's keystrokes will appear here,
          // and yours will appear on their screen.
          <span className="flex items-center gap-1.5 text-warm" title="This session is shared">
            <Users size={13} />
            {viewers} viewers
          </span>
        )}

        {note && <span className="text-alarm">{note}</span>}

        <span className={`ml-auto ${conn === 'open' ? 'text-live' : conn === 'closed' ? 'text-alarm' : 'text-muted'}`}>
          {conn}
        </span>
      </div>
      <div ref={host} className="min-h-0 flex-1 p-2" />
    </div>
  );
}
