import { useEffect, useRef, useState } from 'react';
import { Terminal as XTerm } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { iod, type Capabilities } from '../api';

export function SerialPanel({ caps }: { caps: Capabilities }) {
  const ports = caps.serials ?? [];
  const [at, setAt] = useState(0);
  const port = ports[at];

  return (
    <div className="flex h-full flex-col gap-4">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">Serial</h1>
        <p className="text-sm text-muted">iod owns these ports; this terminal is a client of it.</p>
      </header>

      <div className="flex flex-wrap gap-2">
        {ports.map((p, i) => (
          <button
            key={i}
            onClick={() => setAt(i)}
            className={`rounded-lg px-3 py-1.5 text-sm transition ${
              at === i ? 'bg-accent/20 text-ink' : 'bg-white/5 text-muted hover:text-ink'
            }`}
          >
            {p.label || `Port ${i + 1}`}
            <span className="ml-2 text-xs text-muted">{p.baud}</span>
          </button>
        ))}
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
        port && <Term key={at} index={at} label={port.label} dev={port.dev ?? ''} />
      )}
    </div>
  );
}

function Term({ index, label, dev }: { index: number; label: string; dev: string }) {
  const host = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<'connecting' | 'open' | 'closed'>('connecting');

  useEffect(() => {
    if (!host.current) return;
    const term = new XTerm({
      fontFamily: 'IBM Plex Mono, ui-monospace, Menlo, monospace',
      fontSize: 13,
      cursorBlink: true,
      // Match the panel, not xterm's default black — the terminal is part of
      // the page, not a window sitting on top of it.
      theme: { background: 'rgba(0,0,0,0)', foreground: '#e6eaf2', cursor: '#4ade80' },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host.current);
    fit.fit();

    const ws = iod.serial(index);
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => setState('open');
    ws.onclose = () => setState('closed');
    // Bytes in both directions, untouched. A terminal that rewrites what it
    // carries is a terminal you cannot type into.
    ws.onmessage = (m) =>
      term.write(typeof m.data === 'string' ? m.data : new Uint8Array(m.data));
    term.onData((d) => ws.readyState === WebSocket.OPEN && ws.send(d));

    const ro = new ResizeObserver(() => { try { fit.fit(); } catch { /* not laid out yet */ } });
    ro.observe(host.current);

    return () => { ro.disconnect(); ws.close(); term.dispose(); };
  }, [index]);

  return (
    <div className="flex min-h-0 flex-1 flex-col rounded-xl border border-white/6 bg-black/30">
      <div className="flex items-center justify-between border-b border-white/6 px-3 py-2 text-xs">
        <span className="text-muted">{label} · <span className="font-mono">{dev}</span></span>
        <span className={state === 'open' ? 'text-live' : state === 'closed' ? 'text-alarm' : 'text-muted'}>
          {state}
        </span>
      </div>
      <div ref={host} className="min-h-0 flex-1 p-2" />
    </div>
  );
}
