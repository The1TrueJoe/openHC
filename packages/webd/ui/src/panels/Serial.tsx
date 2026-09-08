import { useEffect, useRef, useState } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { Users, X, Plug } from 'lucide-react';
import { io, serialSocket, type Capabilities, type SerialPort } from '../api';
import { useIoState } from '../App';

/** The serial ports, as a section of the IO page. Opening one raises a
 *  terminal window over the page — a console is a thing you visit, not a place
 *  you navigate to and lose your IO view behind. */
export function SerialSection({ caps }: { caps: Capabilities }) {
  const ports = caps.serials ?? [];
  const [open, setOpen] = useState<number | null>(null);
  const state = useIoState();
  if (!ports.length) return null;

  return (
    <section>
      <h2 className="mb-3 text-sm font-medium">Serial</h2>
      <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-3">
        {ports.map((p, i) => {
          const live = state.serial?.[i];
          const viewers = live?.viewers ?? 0;
          return (
            <button
              key={i}
              onClick={() => setOpen(i)}
              className="hair flex items-center gap-3 rounded-xl border bg-panel p-3 text-left transition hover:border-accent/40"
            >
              <Plug size={18} className="shrink-0 text-muted" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm">{p.label || `Port ${i + 1}`}</div>
                <div className="text-xs text-muted">
                  {live?.baud ?? p.baud} baud
                  {p.transport === 'mcu' && ' · via MCU'}
                </div>
              </div>
              {viewers > 0 && (
                <span className="flex shrink-0 items-center gap-1 text-xs text-warm" title="Somebody is on this console">
                  <Users size={12} />
                  {viewers}
                </span>
              )}
            </button>
          );
        })}
      </div>
      {open !== null && ports[open] && (
        <TerminalWindow
          index={open}
          port={ports[open]}
          baud={state.serial?.[open]?.baud ?? ports[open].baud}
          viewers={state.serial?.[open]?.viewers ?? 0}
          onClose={() => setOpen(null)}
        />
      )}
    </section>
  );
}

/** A terminal is one of the few things that should NOT follow the page theme.
 *  Black and green is what a console looks like, it is what the far end's
 *  colour codes were written against, and the contrast is the point when you
 *  are reading a boot log on a phone at the top of a ladder. */
const TERM_THEME = {
  background: '#000000',
  foreground: '#22c55e',
  cursor: '#22c55e',
  cursorAccent: '#000000',
  selectionBackground: 'rgba(34,197,94,0.3)',
  black: '#000000',
  green: '#22c55e',
  brightGreen: '#4ade80',
  white: '#d1fae5',
  brightWhite: '#ecfdf5',
};

function TerminalWindow({
  index, port, baud, viewers, onClose,
}: {
  index: number; port: SerialPort; baud: number; viewers: number; onClose: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const [conn, setConn] = useState<'connecting' | 'open' | 'closed'>('connecting');
  const [note, setNote] = useState<string | null>(null);
  const routed = port.transport === 'mcu';

  useEffect(() => {
    // Escape closes, as any window should.
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', esc);
    return () => window.removeEventListener('keydown', esc);
  }, [onClose]);

  useEffect(() => {
    if (routed || !host.current) return;
    const t = new XTerm({
      fontFamily: 'IBM Plex Mono, ui-monospace, Menlo, monospace',
      fontSize: 13,
      cursorBlink: true,
      theme: TERM_THEME,
    });
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
    return () => { ro.disconnect(); ws.close(); t.dispose(); };
  }, [index, routed]);

  function changeBaud(next: number) {
    setNote(null);
    try {
      // Applies to the shared session, so every viewer moves together — a
      // console at two different rates is not a thing that can exist. iod
      // announces the reopen on the stream itself.
      io.setBaud(index, next);
    } catch (e) {
      setNote(String((e as Error).message));
    }
  }

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4" onMouseDown={onClose}>
      <div
        className="flex h-[min(80vh,44rem)] w-full max-w-4xl flex-col overflow-hidden rounded-xl border border-black/40 bg-black shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        {/* The window chrome stays legible against the terminal's own black. */}
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-green-500/20 bg-black px-3 py-2 text-xs text-green-500/70">
          <span className="font-mono text-green-400">
            {port.label || `Port ${index + 1}`}
            {port.dev && <span className="ml-2 text-green-500/50">{port.dev}</span>}
          </span>

          <label className="flex items-center gap-1.5">
            baud
            <select
              value={baud}
              onChange={(e) => changeBaud(Number(e.target.value))}
              className="rounded border border-green-500/30 bg-black px-1.5 py-0.5 font-mono text-xs text-green-400 outline-none focus:border-green-500/60"
            >
              {/* Each port carries its own permitted rates: what a host 16550A
                  reaches is not what the IO microcontroller can be asked for.
                  A port already running at something off-list stays selectable
                  rather than silently showing the wrong rate. */}
              {(port.bauds.includes(baud) ? port.bauds : [baud, ...port.bauds]).map((b) => (
                <option key={b} value={b}>{b}</option>
              ))}
            </select>
          </label>

          {viewers > 1 && (
            // Worth saying plainly: somebody else's keystrokes will appear
            // here, and yours will appear on their screen.
            <span className="flex items-center gap-1.5 text-amber-400" title="This session is shared">
              <Users size={12} />
              {viewers} viewers
            </span>
          )}
          {note && <span className="text-red-400">{note}</span>}

          <span className={`ml-auto ${conn === 'open' ? 'text-green-400' : conn === 'closed' ? 'text-red-400' : 'text-green-500/50'}`}>
            {routed ? '' : conn}
          </span>
          <button onClick={onClose} className="text-green-500/70 transition hover:text-green-300" title="Close (Esc)">
            <X size={15} />
          </button>
        </div>

        {routed ? (
          // Not a failure — a different transport. This port has no device
          // node; its bytes travel over the IO protocol's UART opcodes, which
          // iod does not bridge yet. Saying so beats a terminal that silently
          // never receives anything.
          <div className="flex flex-1 items-center justify-center p-8 text-center text-sm text-green-500/60">
            <div>
              <p className="mb-2 text-green-400">{port.label} is routed through the IO microcontroller.</p>
              <p>There is no /dev/tty behind it, and iod does not bridge that path yet.</p>
            </div>
          </div>
        ) : (
          <div ref={host} className="min-h-0 flex-1 bg-black p-2" />
        )}
      </div>
    </div>
  );
}
