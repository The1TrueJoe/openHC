import { type Capabilities, type IoState } from '../api';

/* Board health. Everything here is retained MQTT state that iod refreshes every
   5 s, so this panel is a view — it polls nothing itself and holds no timers. */
export function HealthSection({ caps, state }: { caps: Capabilities; state: IoState }) {
  const h = state.health ?? {};
  const temps = caps.health?.temps ?? [];
  const fans = caps.health?.fans ?? [];
  if (!temps.length && !fans.length && h.cpu === undefined) return null;

  /* CPUTIN and SYSTIN are the chip's own names and they are kept, because
     renaming them to "cpu" and "board" would claim to know where the
     thermistors sit. The tooltip carries the chip so two sources of the same
     quantity are distinguishable. */
  const warm = (c: number) => (c >= 70 ? 'text-alarm' : c >= 55 ? 'text-warm' : 'text-live');

  return (
    <section>
      <h2 className="mb-3 text-sm font-medium">Health</h2>
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {temps.map((t) => {
          const v = h.temp?.[t.slug];
          return (
            <Stat
              key={t.slug}
              label={t.label}
              sub={t.chip}
              value={v === undefined ? '—' : `${v}°C`}
              tone={v === undefined ? '' : warm(v)}
            />
          );
        })}
        {fans.map((f) => {
          const v = h.fan?.[f.slug];
          /* A fan header with nothing plugged into it reads 0. Saying "idle"
             rather than "0 rpm" stops that looking like a stalled fan. */
          return (
            <Stat
              key={f.slug}
              label={f.label}
              sub={f.chip}
              value={v === undefined ? '—' : v === 0 ? 'no fan' : `${v} rpm`}
              tone={v ? 'text-live' : 'text-muted'}
            />
          );
        })}
        {h.cpu !== undefined && (
          <Stat label="CPU" sub={h.load1 !== undefined ? `load ${h.load1}` : ''} value={`${h.cpu}%`} tone="text-ink" />
        )}
        {h.mem?.used_pct !== undefined && (
          <Stat
            label="Memory"
            sub={h.mem.total_kb ? `${Math.round(h.mem.total_kb / 1024)} MB` : ''}
            value={`${h.mem.used_pct}%`}
            tone="text-ink"
          />
        )}
        {h.uptime_s !== undefined && <Stat label="Uptime" sub="" value={fmtUptime(h.uptime_s)} tone="text-ink" />}
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
