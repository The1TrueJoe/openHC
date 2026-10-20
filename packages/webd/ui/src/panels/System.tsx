import { useState } from 'react';
import { Download, Lightbulb, RotateCcw, TriangleAlert } from 'lucide-react';
import { io, type Capabilities, type IoState } from '../api';
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
      <UpdateSection />
      <RestoreSection />
    </div>
  );
}

/* Software updates (ohc-updated): the channel, what it offers this board, and
   an Install that downloads, installs and reboots. Only on boards built with
   the `update` feature — elsewhere there is no retained system/update. */
function UpdateSection() {
  const u = useIoState().system?.update;
  const [armed, setArmed] = useState(false);
  if (!u) return null;
  const busy = ['checking', 'downloading', 'installing', 'rebooting'].includes(u.state);
  const when = u.checked_at ? new Date(u.checked_at * 1000).toLocaleString() : null;
  const doing: Record<string, string> = {
    checking: 'Checking…',
    downloading: `Downloading ${u.latest ?? ''}… ${u.progress ?? 0}%`,
    installing: `Installing ${u.latest ?? ''}…`,
    rebooting: 'Installed — rebooting. This page will reconnect when the controller is back.',
  };
  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Download size={15} className="text-muted" />
        Software update
      </h2>
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-xs">
        <span>Running <code>{u.current}</code></span>
        <label className="flex items-center gap-2">
          Channel
          <select
            value={u.channel}
            disabled={busy}
            onChange={(e) => { setArmed(false); io.updateChannel(e.target.value as 'stable' | 'dev'); }}
            className="hair rounded-md border bg-raised px-2 py-1 text-xs"
          >
            <option value="stable">Stable — published releases</option>
            <option value="dev">Dev — latest build of main</option>
          </select>
        </label>
        <button disabled={busy} onClick={() => io.updateCheck()}
                className="hair rounded-lg border px-3 py-1 transition hover:border-accent/50 disabled:opacity-50">
          Check now
        </button>
      </div>
      <p className="mt-3 text-xs text-muted">
        {busy ? doing[u.state]
          : u.state === 'error' ? <span className="text-alarm">{u.error}</span>
          : u.latest === null ? (when ? `Nothing on the ${u.channel} channel for this board yet.` : 'Not checked yet.')
          : u.available ? <>Version <code>{u.latest}</code> is available on the {u.channel} channel.</>
          : <>Up to date with the {u.channel} channel ({u.latest}).</>}
        {when && !busy && <span className="ml-2">Checked {when}.</span>}
      </p>
      {u.state === 'downloading' && (
        <div className="mt-2 h-1.5 w-full overflow-hidden rounded bg-raised">
          <div className="h-full bg-accent transition-all" style={{ width: `${u.progress ?? 0}%` }} />
        </div>
      )}
      {u.available && !busy && (
        u.installable ? (
          <div className="mt-3 flex items-center gap-2">
            {!armed ? (
              <button onClick={() => setArmed(true)}
                      className="rounded-lg bg-accent/20 px-3 py-1.5 text-xs font-medium transition hover:bg-accent/30">
                Install {u.latest}…
              </button>
            ) : (
              <>
                <span className="text-xs">Downloads, installs and reboots the controller.</span>
                <button onClick={() => { setArmed(false); io.updateInstall(); }}
                        className="rounded-lg bg-accent/30 px-3 py-1.5 text-xs font-medium transition hover:bg-accent/40">
                  Install and reboot
                </button>
                <button onClick={() => setArmed(false)}
                        className="hair rounded-lg border px-3 py-1.5 text-xs transition hover:border-accent/50">
                  Cancel
                </button>
              </>
            )}
          </div>
        ) : (
          <p className="mt-2 text-xs text-muted">
            This board installs updates with <code>ohc-flash</code> for now; on-box install is coming.
          </p>
        )
      )}
    </section>
  );
}

/* Return to stock. Only appears on boards that actually support it (those built
   with the `restore` feature, which ships ohc-restore: the EA family and the
   HC-800); on anything else iod's
   retained `system/restore` says `available: false` and this renders nothing.
   It is the one destructive control in the UI, so it is two-step: an arm, then
   a confirm — and iod acts only on the literal payload "confirm". */
function RestoreSection() {
  const avail = useIoState().system?.restore;
  const [armed, setArmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  if (!avail?.available) return null;

  const start = () => {
    setBusy(true);
    io.restoreStock();
    setMsg('Returning to stock — the controller is rebooting. This page will stop responding.');
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
          : 'Hands the controller to Control4’s own factory recovery: it reboots, re-images itself to the factory image, and comes back as stock Control4. openHC is removed until reinstalled.'}
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
