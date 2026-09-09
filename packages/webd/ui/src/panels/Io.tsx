import { useEffect, useState } from 'react';
import { Zap, CircleDot, Circle, Radio } from 'lucide-react';
import { io, panel, type Capabilities, type IoState } from '../api';
import { SerialSection } from './Serial';
import { HealthSection } from './Health';
import { useIoState } from '../App';

export function IoPanel({ caps }: { caps: Capabilities }) {
  // Contacts and relays are STATE, mirrored from iod. Nothing here polls: iod
  // polls the MCU once on everyone's behalf and publishes changes, so every
  // open tab agrees and the UART is asked once rather than once per viewer.
  const state = useIoState();
  const [busy, setBusy] = useState<number | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const linkUp = state.mcu?.link !== false;

  function setRelay(i: number, on: boolean) {
    setBusy(i);
    setNote(null);
    try {
      // set, not toggle: the UI knows what it wants the relay to BE. Toggle
      // from a stale view closes a relay that somebody else just closed.
      io.relaySet(i, on);
    } catch (e) {
      setNote(String((e as Error).message));
    } finally {
      // Publishing is fire-and-forget: the answer arrives as the retained
      // relay topic changing, which every other open page sees too.
      setTimeout(() => setBusy(null), 150);
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
              const on = state.contact?.[panel(i)];
              return (
                <div
                  key={i}
                  className={`hair flex items-center gap-3 rounded-xl border p-3 transition ${
                    on ? 'border-live/40 bg-live/10' : 'bg-panel'
                  }`}
                >
                  {on ? <CircleDot size={18} className="text-live" /> : <Circle size={18} className="text-muted" />}
                  <div>
                    <div className="text-sm">Contact {i + 1}</div>
                    <div className={`text-xs ${on ? 'text-live' : 'text-muted'}`}>
                      {on === undefined ? '—' : on ? 'closed' : 'open'}
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
          <h2 className="mb-3 text-sm font-medium">Relays</h2>
          <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
            {Array.from({ length: caps.relays.count }, (_, i) => {
              const on = state.relay?.[panel(i)];
              return (
                <button
                  key={i}
                  onClick={() => setRelay(i, !on)}
                  disabled={busy !== null || !linkUp}
                  className={`hair flex items-center gap-3 rounded-xl border p-3 text-left transition disabled:opacity-40 ${
                    on ? 'border-warm/50 bg-warm/10' : 'bg-panel hover:border-accent/40'
                  }`}
                >
                  <Zap size={18} className={on ? 'text-warm' : 'text-muted'} />
                  <div className="min-w-0">
                    <div className="text-sm">Relay {i + 1}</div>
                    <div className={`text-xs ${on ? 'text-warm' : 'text-muted'}`}>
                      {busy === i ? 'switching…' : on === undefined ? '—' : on ? 'closed' : 'open'}
                    </div>
                  </div>
                </button>
              );
            })}
          </div>
          {note && <p className="mt-2 text-xs text-alarm">{note}</p>}
        </section>
      )}

      <HealthSection />
      {caps.leds?.length ? <LedSection caps={caps} state={state} /> : null}
      {caps.ir && <IrSection caps={caps} />}

      <SerialSection caps={caps} />
    </div>
  );
}

/* The front panel. Not a numbered row like the relays — these are named by the
   kernel, and one of them is a single tri-colour LED with three channels, so
   they are grouped by function rather than listed flat. */
function LedSection({ caps, state }: { caps: Capabilities; state: IoState }) {
  const leds = caps.leds ?? [];
  const groups = Array.from(new Set(leds.map((l) => l.function)));
  const swatch: Record<string, string> = {
    red: '#ef4444', yellow: '#eab308', blue: '#3b82f6',
    green: '#22c55e', white: '#e5e7eb', amber: '#f59e0b',
  };
  return (
    <section>
      <h2 className="mb-3 text-sm font-medium">Front panel</h2>
      <div className="hair rounded-xl border bg-panel p-4">
        <div className="flex flex-wrap gap-4">
          {groups.map((fn) => {
            const chans = leds.filter((l) => l.function === fn);
            return (
              <div key={fn} className="min-w-[7rem]">
                <div className="mb-2 text-xs text-muted">{fn}</div>
                <div className="flex gap-2">
                  {chans.map((l) => {
                    const on = (state.led?.[l.slug] ?? 0) > 0;
                    const c = swatch[l.colour] ?? '#94a3b8';
                    return (
                      <button
                        key={l.slug}
                        onClick={() => io.setLed(l.slug, !on)}
                        title={`${l.name}${l.trigger !== 'none' ? ` — trigger: ${l.trigger}` : ''}`}
                        className="hair flex h-9 w-9 items-center justify-center rounded-lg border transition hover:border-accent/50"
                      >
                        <span
                          className="h-4 w-4 rounded-full transition"
                          style={{
                            background: on ? c : 'transparent',
                            border: `1.5px solid ${c}`,
                            boxShadow: on ? `0 0 9px ${c}` : 'none',
                            opacity: on ? 1 : 0.45,
                          }}
                        />
                      </button>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </div>
        {/* A LED under a kernel trigger is not software-controlled until something
            writes brightness, which drops the trigger. Say so rather than letting
            a click look like it did nothing. */}
        {leds.some((l) => l.trigger !== 'none') && (
          <p className="mt-3 text-xs text-muted">
            {leds.filter((l) => l.trigger !== 'none').map((l) => `${l.slug} (${l.trigger})`).join(', ')}
            {' '}driven by a kernel trigger — clicking takes manual control.
          </p>
        )}
      </div>
    </section>
  );
}

function IrSection({ caps }: { caps: Capabilities }) {
  const [pronto, setPronto] = useState('0000 006D 0022 0002 0157 00AC 0016 0016');
  // null = the front blaster; a number = a rear jack, as labelled on the case.
  const [port, setPort] = useState<number | null>(0);
  const [note, setNote] = useState<string | null>(null);
  const [heard, setHeard] = useState<{ pronto: string; at: string }[]>([]);

  // The kernel gives every emitter its own lirc node, named. Showing which one
  // a button drives is the difference between a UI that is the only way in and
  // one that tells you how to do the same thing without it.
  const node = (name: string) =>
    caps.ir?.devices?.find((d) => d.name === name)?.device;

  // What the RECEIVER hears. This is an event, not state — a remote press has
  // no value between presses — and it is the same event an external control
  // system would trigger automations from.
  useEffect(() => {
    if (!caps.ir?.receiver) return;
    const off = io.subscribeEvents('ir/front/rx');
    const un = io.onEvent((topic: string, data: any) => {
      if (topic !== 'ir/front/rx') return;
      setHeard((p) => [{ pronto: data.pronto, at: new Date().toLocaleTimeString() }, ...p].slice(0, 6));
    });
    return () => { off(); un(); };
  }, [caps.ir?.receiver]);

  function send() {
    setNote(null);
    try {
      io.sendIr(port, pronto.trim());
      setNote('sent');
    } catch (e) {
      setNote(String((e as Error).message));
    }
  }

  return (
    <section>
      <h2 className="mb-3 text-sm font-medium">Infrared</h2>
      <div className="hair rounded-xl border bg-panel p-4">
        <div className="mb-3 flex flex-wrap items-center gap-2">
          {Array.from({ length: caps.ir!.out }, (_, i) => (
            <button
              key={i}
              onClick={() => setPort(i)}
              title={node(`openHC IR out ${i + 1}`) ?? undefined}
              className={`rounded-lg px-3 py-1.5 text-sm transition ${
                port === i ? 'bg-accent/20 text-ink' : 'shade text-muted hover:text-ink'
              }`}
            >
              Out {i + 1}
            </button>
          ))}
          {/* The blaster is an internal emitter behind the front panel, not a
              seventh socket. Naming it stops anyone hunting for the jack. */}
          {caps.ir!.blaster > 0 && (
            <button
              onClick={() => setPort(null)}
              className={`rounded-lg px-3 py-1.5 text-sm transition ${
                port === null ? 'bg-accent/20 text-ink' : 'shade text-muted hover:text-ink'
              }`}
              title={
                'Internal emitter on the front panel — same place as the receiver' +
                (node('openHC IR front blaster') ? ` (${node('openHC IR front blaster')})` : '')
              }
            >
              Front
            </button>
          )}
        </div>
        <textarea
          value={pronto}
          onChange={(e) => setPronto(e.target.value)}
          rows={3}
          spellCheck={false}
          className="hair w-full rounded-lg border bg-raised p-3 font-mono text-xs text-ink outline-none focus:border-accent/50"
        />
        <div className="mt-2 flex flex-wrap items-center gap-3">
          <button onClick={send} className="rounded-lg bg-accent/20 px-4 py-2 text-sm transition hover:bg-accent/30">
            Send
          </button>
          <span className="text-xs text-muted">
            Pronto hex, type <code>0000</code> only. Durations are carrier periods.
          </span>
          {/* Say where this actually goes. Each emitter is a separate device,
              so the same code can be sent without this page. */}
          {caps.ir?.via === 'lirc' && (
            <span className="text-xs text-muted">
              via <code>{node(port === null ? 'openHC IR front blaster' : `openHC IR out ${port + 1}`) ?? 'lirc'}</code>
            </span>
          )}
          {note && <span className={`text-xs ${note === 'sent' ? 'text-live' : 'text-alarm'}`}>{note}</span>}
        </div>
      </div>

      {caps.ir?.receiver ? (
        <div className="hair mt-3 rounded-xl border bg-panel p-4">
          <div className="mb-2 flex items-center gap-2 text-sm font-medium">
            <Radio size={15} className={heard.length ? 'text-live' : 'text-muted'} />
            Receiver
          </div>
          {heard.length === 0 ? (
            <p className="text-xs text-muted">
              Listening on <code>{node('openHC IR front receiver') ?? 'the front receiver'}</code>.
              Point a remote at the front of the controller.
            </p>
          ) : (
            <ul className="space-y-1">
              {heard.map((h, i) => (
                <li key={i} className="flex items-baseline gap-3">
                  <span className="shrink-0 text-xs text-muted">{h.at}</span>
                  <code className="truncate font-mono text-xs text-ink" title={h.pronto}>{h.pronto}</code>
                  {/* Captured, then re-sendable: that is IR learning. */}
                  <button onClick={() => setPronto(h.pronto)} className="ml-auto shrink-0 text-xs text-accent hover:underline">
                    use
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </section>
  );
}
