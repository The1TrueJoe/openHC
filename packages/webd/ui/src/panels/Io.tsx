import { useEffect, useRef, useState } from 'react';
import { Zap, CircleDot, Circle } from 'lucide-react';
import { iod, type Capabilities, type IodEvent } from '../api';

export function IoPanel({ caps }: { caps: Capabilities }) {
  const [closed, setClosed] = useState<boolean[]>([]);
  const [linkUp, setLinkUp] = useState(true);
  const [busy, setBusy] = useState<number | null>(null);
  const [note, setNote] = useState<string | null>(null);

  // Contacts are LIVE. Polling from the browser would sample the same UART from
  // every open tab; iod already polls once and publishes transitions, so this
  // just listens.
  useEffect(() => {
    if (!caps.contacts) return;
    const ws = iod.events();
    ws.onmessage = (m) => {
      const e: IodEvent = JSON.parse(m.data);
      if (e.type === 'contact_snapshot') setClosed(e.closed);
      else if (e.type === 'contact') setClosed((p) => p.map((v, i) => (i === e.index ? e.closed : v)));
      else if (e.type === 'mcu_link') setLinkUp(e.up);
    };
    return () => ws.close();
  }, [caps.contacts]);

  async function toggle(i: number) {
    setBusy(i);
    setNote(null);
    try {
      await iod.toggleRelays(1 << i);
    } catch (e) {
      setNote(String((e as Error).message));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="space-y-6">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">IO</h1>
        <p className="text-sm text-muted">
          {caps.backend === 'mcu' ? 'Behind the IO microcontroller' : 'Native GPIO'}
        </p>
      </header>

      {!linkUp && (
        <div className="rounded-lg border border-alarm/30 bg-alarm/5 px-4 py-3 text-sm text-alarm">
          The IO microcontroller stopped answering. Readings below are stale.
        </div>
      )}

      {caps.contacts && (
        <section>
          <h2 className="mb-3 text-sm font-medium">Contacts</h2>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
            {Array.from({ length: caps.contacts.count }, (_, i) => {
              const on = closed[i];
              return (
                <div
                  key={i}
                  className={`flex items-center gap-3 rounded-xl border p-3 transition ${
                    on ? 'border-live/40 bg-live/10' : 'border-white/6 bg-white/2'
                  }`}
                >
                  {on ? <CircleDot size={18} className="text-live" /> : <Circle size={18} className="text-muted" />}
                  <div>
                    <div className="text-sm">Contact {i + 1}</div>
                    <div className={`text-xs ${on ? 'text-live' : 'text-muted'}`}>
                      {closed.length ? (on ? 'closed' : 'open') : '—'}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        </section>
      )}

      {caps.relays && (
        <section>
          <h2 className="mb-1 text-sm font-medium">Relays</h2>
          {/* Honest about a real gap: the firmware's RELAY_STATE encoding is not
              decoded (a four-relay board answers ff 00), so we can command a
              toggle but cannot draw a trustworthy on/off state. Showing a
              confident toggle here would be a lie about a contact that may be
              switching a real load. */}
          <p className="mb-3 text-xs text-warm">
            State reporting is not decoded yet — these send a toggle, they do not show on/off.
          </p>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
            {Array.from({ length: caps.relays.count }, (_, i) => (
              <button
                key={i}
                onClick={() => toggle(i)}
                disabled={busy !== null || !linkUp}
                className="flex items-center gap-3 rounded-xl border border-white/6 bg-white/2 p-3 text-left transition hover:border-accent/40 hover:bg-accent/10 disabled:opacity-40"
              >
                <Zap size={18} className={busy === i ? 'text-warm' : 'text-accent'} />
                <div>
                  <div className="text-sm">Relay {i + 1}</div>
                  <div className="text-xs text-muted">{busy === i ? 'sending…' : 'toggle'}</div>
                </div>
              </button>
            ))}
          </div>
          {note && <p className="mt-2 text-xs text-alarm">{note}</p>}
        </section>
      )}

      {caps.ir && <IrSection caps={caps} />}
    </div>
  );
}

function IrSection({ caps }: { caps: Capabilities }) {
  const [pronto, setPronto] = useState('0000 006D 0022 0002 0157 00AC 0016 0016');
  const [port, setPort] = useState(0);
  const [note, setNote] = useState<string | null>(null);
  const ref = useRef<HTMLTextAreaElement>(null);

  async function send() {
    setNote(null);
    try {
      await iod.sendIr(port, pronto.trim());
      setNote('sent');
    } catch (e) {
      setNote(String((e as Error).message));
    }
  }

  return (
    <section>
      <h2 className="mb-3 text-sm font-medium">Infrared</h2>
      <div className="rounded-xl border border-white/6 bg-white/2 p-4">
        <div className="mb-3 flex flex-wrap items-center gap-2">
          {Array.from({ length: caps.ir!.total }, (_, i) => {
            // The blaster is an internal emitter, not a rear jack — label it so
            // nobody plugs an emitter into a socket that does not exist.
            const isBlaster = i >= caps.ir!.out;
            return (
              <button
                key={i}
                onClick={() => setPort(i)}
                className={`rounded-lg px-3 py-1.5 text-sm transition ${
                  port === i ? 'bg-accent/20 text-ink' : 'bg-white/5 text-muted hover:text-ink'
                }`}
              >
                {isBlaster ? 'Blaster' : `Out ${i + 1}`}
              </button>
            );
          })}
        </div>
        <textarea
          ref={ref}
          value={pronto}
          onChange={(e) => setPronto(e.target.value)}
          rows={3}
          spellCheck={false}
          className="w-full rounded-lg border border-white/8 bg-black/30 p-3 font-mono text-xs text-ink outline-none focus:border-accent/50"
        />
        <div className="mt-2 flex items-center gap-3">
          <button
            onClick={send}
            className="rounded-lg bg-accent/20 px-4 py-2 text-sm transition hover:bg-accent/30"
          >
            Send
          </button>
          <span className="text-xs text-muted">
            Pronto hex, type <code>0000</code> only. Durations are carrier periods.
          </span>
          {note && <span className={`text-xs ${note === 'sent' ? 'text-live' : 'text-alarm'}`}>{note}</span>}
        </div>
      </div>
    </section>
  );
}
