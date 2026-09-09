import { useEffect, useState } from 'react';
import { rest, type Telemetry } from '../api';

/* Board health, polled from sysmond over REST.
   Not MQTT: a temperature every five seconds is telemetry, not state an
   automation subscribes to, and a retained topic cannot answer "what did it do
   over the last hour" — which is the question this data exists for. */
export function HealthSection() {
  const [t, setT] = useState<Telemetry | null>(null);
  const [absent, setAbsent] = useState(false);

  useEffect(() => {
    let live = true;
    const tick = () =>
      rest
        .telemetry()
        .then((d) => live && setT(d))
        .catch(() => live && setAbsent(true));
    tick();
    /* 5 s matches sysmond's own sampling period; asking faster returns the same
       sample twice. */
    const id = setInterval(tick, 5000);
    return () => {
      live = false;
      clearInterval(id);
    };
  }, []);

  /* A board with no hwmon and no sysmond simply has no panel, the same way a
     board with no relays has no relay panel. */
  if (absent || !t || !t.series.length) return null;

  const val = (i: number) => t.values?.[i] ?? null;
  const warm = (c: number) => (c >= 70 ? 'text-alarm' : c >= 55 ? 'text-warm' : 'text-live');

  return (
    <section>
      <h2 className="mb-3 text-sm font-medium">Health</h2>
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {t.series.map((s, i) => {
          const v = val(i);
          if (s.kind === 'pwm') return null;
          const isTemp = s.kind === 'temp';
          return (
            <Stat
              key={`${s.kind}-${s.slug}`}
              label={s.label}
              sub={s.chip}
              value={
                v === null ? '—'
                  : isTemp ? `${v}°C`
                  /* A fan header with nothing plugged into it reads 0. Saying
                     "no fan" stops that looking like a stalled one. */
                  : v === 0 ? 'no fan'
                  : `${v} rpm`
              }
              tone={v === null ? 'text-muted' : isTemp ? warm(v) : v ? 'text-live' : 'text-muted'}
            />
          );
        })}
        {t.cpu !== null && (
          <Stat label="CPU" sub={t.load1 !== null ? `load ${t.load1}` : ''} value={`${t.cpu}%`} tone="text-ink" />
        )}
        {t.mem_used_pct !== null && (
          <Stat
            label="Memory"
            sub={t.mem_total_kb ? `${Math.round(t.mem_total_kb / 1024)} MB` : ''}
            value={`${t.mem_used_pct}%`}
            tone="text-ink"
          />
        )}
        {t.uptime_s !== null && <Stat label="Uptime" sub="" value={fmtUptime(t.uptime_s)} tone="text-ink" />}
      </div>
    </section>
  );
}

function fmtUptime(s: number) {
  const d = Math.floor(s / 86400);
  const hh = Math.floor((s % 86400) / 3600);
  const mm = Math.floor((s % 3600) / 60);
  if (d) return `${d}d ${hh}h`;
  if (hh) return `${hh}h ${mm}m`;
  return `${mm}m`;
}

function Stat({ label, sub, value, tone }: { label: string; sub: string; value: string; tone: string }) {
  return (
    <div className="hair rounded-xl border bg-panel p-4">
      <div className="text-xs text-muted">{label}</div>
      <div className={`mt-1 text-2xl font-semibold tabular-nums ${tone}`}>{value}</div>
      {sub && <div className="mt-0.5 text-xs text-muted">{sub}</div>}
    </div>
  );
}
