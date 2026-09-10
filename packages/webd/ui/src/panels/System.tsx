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
    <div className="space-y-6">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">System</h1>
        <p className="text-sm text-muted">The controller itself — sensors, fan and front panel</p>
      </header>
      <HealthSection />
      {caps.leds?.length ? <LedSection caps={caps} state={state} /> : null}
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
