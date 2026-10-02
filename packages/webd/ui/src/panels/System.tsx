import { useEffect, useState } from 'react';
import { Lightbulb, RotateCcw, TriangleAlert } from 'lucide-react';
import { io, rest, type Capabilities, type IoState } from '../api';
import { useIoState } from '../App';
import { HealthSection } from './Health';

/* The box itself, as distinct from the IO it controls.
   Relays, contacts, IR and serial are things wired TO this controller and live
   on the IO page. Temperatures, the fan and the front-panel LEDs are the
   controller — you look at them to answer "is this unit healthy", not "is the
   garage door open". */
export function SystemPanel({ caps }: { caps: Capabilities }) {
  const state = useIoState();
  return (
    <div className="space-y-4">
      <HealthSection />
      {caps.leds?.length ? <LedSection caps={caps} state={state} /> : null}
      <RestoreSection />
    </div>
  );
}

/* Return to stock. Only appears on boards that actually support it (the EA /
   CEFDK family, where iod has the ohc-restore helper); on anything else the
   status call says `available: false` and this renders nothing. It is the one
   destructive control in the UI, so it is two-step: an arm, then a confirm. */
function RestoreSection() {
  const [avail, setAvail] = useState<null | { available: boolean; openhc?: boolean; detail?: string }>(null);
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    rest.restoreStatus().then(setAvail).catch(() => setAvail({ available: false }));
  }, []);

  if (!avail?.available) return null;

  const start = async () => {
    setBusy(true);
    setMsg(null);
    try {
      await rest.restoreStock();
      setMsg('Returning to stock — the controller is rebooting. This page will stop responding.');
    } catch (e) {
      setMsg(`Failed: ${e instanceof Error ? e.message : String(e)}`);
      setBusy(false);
      setArmed(false);
    }
  };

  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <RotateCcw size={15} className="text-muted" />
        Return to stock
      </h2>
      <p className="text-xs text-muted">
        {avail.openhc === false
          ? 'This controller is already on the stock boot path.'
          : 'Reverses the one boot-header change openHC made and hands the rootfs back to Control4’s recovery kernel. The controller reboots to the factory image and openHC is removed until reinstalled.'}
      </p>
      {msg && <p className="mt-3 text-xs">{msg}</p>}
      {!msg && (
        <div className="mt-3 flex items-center gap-2">
          {!armed ? (
            <button
              onClick={() => setArmed(true)}
              className="hair rounded-lg border px-3 py-1.5 text-xs font-medium text-red-500 transition hover:border-red-500/50"
            >
              Reset to stock…
            </button>
          ) : (
            <>
              <span className="flex items-center gap-1 text-xs text-red-500">
                <TriangleAlert size={14} /> This wipes openHC. Sure?
              </span>
              <button
                disabled={busy}
                onClick={start}
                className="rounded-lg bg-red-600 px-3 py-1.5 text-xs font-medium text-white transition hover:bg-red-500 disabled:opacity-50"
              >
                {busy ? 'Starting…' : 'Yes, return to stock'}
              </button>
              <button
                disabled={busy}
                onClick={() => setArmed(false)}
                className="hair rounded-lg border px-3 py-1.5 text-xs transition hover:border-accent/50"
              >
                Cancel
              </button>
            </>
          )}
        </div>
      )}
    </section>
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
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Lightbulb size={15} className="text-muted" />
        Front panel
      </h2>
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
                      className="hair flex h-9 w-9 items-center justify-center rounded-lg border bg-raised transition hover:border-accent/50"
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
    </section>
  );
}
