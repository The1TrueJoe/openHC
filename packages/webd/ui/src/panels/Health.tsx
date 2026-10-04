import { useEffect, useRef, useState } from 'react';
import { Activity, Fan } from 'lucide-react';
import { io, type History } from '../api';
import { useIoState } from '../App';

type Sample = History['samples'][number];

/* Board health, live over MQTT — nothing here polls REST.
   sysmond samples every five seconds and keeps the ring; iod republishes its
   latest sample as `health/now` and the ring, at one point a minute, as
   `health/history` (both retained, so they are there on connect). Trend lines
   are that minute history plus the live samples seen since its last point. */
export function HealthSection() {
  const h = useIoState().health;
  const t = h?.now;
  const hist = h?.history;
  const histOk = !!hist && Array.isArray(hist.samples) && Array.isArray(hist.series);
  const lastHistAt = histOk ? hist!.samples[hist!.samples.length - 1]?.at ?? 0 : 0;

  /* The live tail: samples newer than the last history point, positional
     against t.series (which can differ from the history's series if a sensor
     bound late — trend() matches by identity, not index). */
  const tail = useRef<Sample[]>([]);
  const [, bump] = useState(0);
  useEffect(() => {
    if (!t?.at || !Array.isArray(t.values)) return;
    const next = tail.current.filter((s) => s.at > lastHistAt && s.at !== t.at);
    next.push({ at: t.at, v: t.values, cpu: t.cpu, load1: t.load1, mem_used_pct: t.mem_used_pct });
    tail.current = next.slice(-30);
    bump((n) => n + 1);
  }, [t?.at, lastHistAt]);

  /* A board with no hwmon and no sysmond simply has no panel, the same way a
     board with no relays has no relay panel. */
  if (!t || !Array.isArray(t.series) || !t.series.length) return null;

  const val = (i: number) => t.values?.[i] ?? null;
  const warm = (c: number) => (c >= 70 ? 'text-alarm' : c >= 55 ? 'text-warm' : 'text-live');

  /* Trend for a sensor series, matched by slug+kind rather than index — the two
     endpoints share a series list, but matching by identity survives a sensor
     that binds late and shifts the order. */
  const tailNow = tail.current.filter((s) => s.at > lastHistAt);
  const trend = (slug: string, kind: string): (number | null)[] | null => {
    const hi = histOk ? hist!.series.findIndex((s) => s.slug === slug && s.kind === kind) : -1;
    const ti = t.series.findIndex((s) => s.slug === slug && s.kind === kind);
    const past = hi >= 0 ? hist!.samples.map((s) => s.v[hi] ?? null) : [];
    const recent = ti >= 0 ? tailNow.map((s) => s.v[ti] ?? null) : [];
    const all = [...past, ...recent];
    return all.length > 1 ? all : null;
  };
  const machineTrend = (pick: (s: Sample) => number | null) => {
    const all = [...(histOk ? hist!.samples.map(pick) : []), ...tailNow.map(pick)];
    return all.length > 1 ? all : null;
  };

  /* Order so like sits with like: temps, then fans, then speeds. */
  const order = { temp: 0, fan: 1, pwm: 2 } as const;
  const indexed = t.series.map((s, i) => ({ s, i }));
  indexed.sort((a, b) => order[a.s.kind] - order[b.s.kind]);

  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Activity size={15} className="text-muted" />
        Health
      </h2>
      <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {indexed.map(({ s, i }) => {
          const v = val(i);
          const isTemp = s.kind === 'temp';
          const isPwm = s.kind === 'pwm';
          /* pwm is a 0-255 duty register; a user thinks in percent. */
          const pct = isPwm && v !== null ? Math.round((v / 255) * 100) : null;
          const display =
            v === null ? '—'
              : isTemp ? `${v}°C`
              : isPwm ? `${pct}%`
              /* A fan header with nothing plugged into it reads 0. Saying
                 "no fan" stops that looking like a stalled one. */
              : v === 0 ? 'no fan'
              : `${v} rpm`;
          const tone =
            v === null ? 'text-muted'
              : isTemp ? warm(v)
              : isPwm ? 'text-ink'
              : v ? 'text-live' : 'text-muted';
          /* Plot pwm as percent so its sparkline shares the readout's scale. */
          const raw = trend(s.slug, s.kind);
          const series = isPwm && raw ? raw.map((x) => (x === null ? null : Math.round((x / 255) * 100))) : raw;
          return (
            <Stat key={`${s.kind}-${s.slug}`} label={s.label} sub={s.chip} value={display} tone={tone} series={series} />
          );
        })}
        {t.cpu !== null && (
          <Stat
            label="CPU"
            sub={t.load1 !== null ? `load ${t.load1.toFixed(2)}` : ''}
            value={`${t.cpu}%`}
            tone="text-ink"
            series={machineTrend((s) => s.cpu)}
          />
        )}
        {t.mem_used_pct !== null && (
          <Stat
            label="Memory"
            sub={t.mem_total_kb ? `${Math.round(t.mem_total_kb / 1024)} MB` : ''}
            value={`${t.mem_used_pct}%`}
            tone="text-ink"
            series={machineTrend((s) => s.mem_used_pct)}
          />
        )}
        {t.uptime_s !== null && <Stat label="Uptime" sub="" value={fmtUptime(t.uptime_s)} tone="text-ink" />}
      </div>
      <FanControl />
    </section>
  );
}

/* Fan control. Only on boards whose sysmond reports a controllable fan (the EA
   family, where ohc-fand exists); everywhere else `available:false` and this
   renders nothing — the same rule the rest of the UI uses for absent hardware. */
function FanControl() {
  const fan = useIoState().health?.fan;
  const [busy, setBusy] = useState(false);
  // A command's effect arrives as a new health/fan; that ends the busy state.
  useEffect(() => setBusy(false), [fan?.mode, fan?.pct]);

  if (!fan?.available) return null;

  const act = (v: number | 'auto') => {
    setBusy(true);
    io.setFan(v);
    // Never stay stuck if the state does not change (e.g. same value again).
    setTimeout(() => setBusy(false), 4000);
  };
  const err = fan.error ?? null;

  const manual = fan.mode === 'manual';
  const pct = fan.pct ?? 0;

  return (
    <div className="hair mt-3 rounded-xl border bg-raised p-4">
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center gap-2 text-sm font-medium">
          <Fan size={15} className="text-muted" />
          Fan
        </div>
        <span
          className={`rounded-full px-2 py-0.5 text-xs font-medium ${
            manual ? 'bg-warm/15 text-warm' : 'bg-live/15 text-live'
          }`}
        >
          {manual ? `Manual · ${pct}%` : `Auto · ${pct}%`}
        </span>
      </div>

      <Slider value={pct} disabled={busy} onCommit={(p) => act(p)} />

      <div className="mt-3 flex flex-wrap items-center gap-2">
        {[0, 25, 50, 75, 100].map((p) => (
          <button
            key={p}
            disabled={busy}
            onClick={() => act(p)}
            className={`hair rounded-lg border px-2.5 py-1 text-xs tabular-nums transition hover:border-accent/50 disabled:opacity-50 ${
              manual && pct === p ? 'border-accent/60 text-ink' : 'text-muted'
            }`}
          >
            {p}%
          </button>
        ))}
        <button
          disabled={busy || !manual}
          onClick={() => act('auto')}
          className="ml-auto hair rounded-lg border px-2.5 py-1 text-xs transition hover:border-accent/50 disabled:opacity-40"
        >
          Release to automatic
        </button>
      </div>
      {/* The thermal fail-safe is not optional, so say the manual hold has a
          floor the board enforces regardless. */}
      <p className="mt-2 text-xs text-muted">
        {manual
          ? 'Held manually. A critical temperature still forces 100%.'
          : 'Following the temperature curve.'}
      </p>
      {err && <p className="mt-1 text-xs text-alarm">{err}</p>}
    </div>
  );
}

/* A drag-to-set slider that only commits on release — dragging it would
   otherwise fire a shell-out per pixel. */
function Slider({ value, disabled, onCommit }: { value: number; disabled: boolean; onCommit: (p: number) => void }) {
  const [local, setLocal] = useState(value);
  const dragging = useRef(false);
  useEffect(() => {
    if (!dragging.current) setLocal(value);
  }, [value]);
  return (
    <input
      type="range"
      min={0}
      max={100}
      step={5}
      value={local}
      disabled={disabled}
      onChange={(e) => {
        dragging.current = true;
        setLocal(Number(e.target.value));
      }}
      onMouseUp={() => {
        dragging.current = false;
        onCommit(local);
      }}
      onTouchEnd={() => {
        dragging.current = false;
        onCommit(local);
      }}
      onKeyUp={() => onCommit(local)}
      className="w-full accent-accent disabled:opacity-50"
    />
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

function Stat({
  label,
  sub,
  value,
  tone,
  series,
}: {
  label: string;
  sub: string;
  value: string;
  tone: string;
  series?: (number | null)[] | null;
}) {
  return (
    <div className="hair rounded-xl border bg-raised p-4">
      <div className="text-xs text-muted">{label}</div>
      <div className={`mt-1 text-2xl font-semibold tabular-nums ${tone}`}>{value}</div>
      {series && series.some((v) => v !== null) ? (
        <Spark values={series} className={`mt-2 ${tone}`} />
      ) : (
        sub && <div className="mt-0.5 text-xs text-muted">{sub}</div>
      )}
      {series && series.some((v) => v !== null) && sub && <div className="mt-0.5 text-xs text-muted">{sub}</div>}
    </div>
  );
}

/* A hand-drawn SVG sparkline — no chart library. Normalised to its own min/max
   so a two-degree wobble is visible; a flat series draws a flat line rather than
   dividing by zero. Nulls break the line into segments (a sensor that bound late
   has no value at the old end of the ring). */
function Spark({ values, className }: { values: (number | null)[]; className?: string }) {
  const W = 120;
  const H = 28;
  const nums = values.filter((v): v is number => v !== null);
  if (!nums.length) return null;
  const min = Math.min(...nums);
  const max = Math.max(...nums);
  const span = max - min || 1;
  const n = values.length;
  const x = (i: number) => (n <= 1 ? 0 : (i / (n - 1)) * W);
  const y = (v: number) => H - 2 - ((v - min) / span) * (H - 4);

  // Build polyline segments, broken on nulls.
  const segs: string[] = [];
  let cur: string[] = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (cur.length) segs.push(cur.join(' '));
      cur = [];
    } else {
      cur.push(`${x(i).toFixed(1)},${y(v).toFixed(1)}`);
    }
  });
  if (cur.length) segs.push(cur.join(' '));

  return (
    <svg
      viewBox={`0 0 ${W} ${H}`}
      preserveAspectRatio="none"
      className={`block h-7 w-full ${className ?? ''}`}
      aria-hidden="true"
    >
      {segs.map((pts, i) =>
        pts.includes(' ') ? (
          <polyline
            key={i}
            points={pts}
            fill="none"
            stroke="currentColor"
            strokeWidth={1.5}
            strokeLinejoin="round"
            strokeLinecap="round"
            vectorEffect="non-scaling-stroke"
            opacity={0.85}
          />
        ) : (
          /* A single point can't be a line; draw a dot so it isn't invisible. */
          <circle key={i} cx={pts.split(',')[0]} cy={pts.split(',')[1]} r={1.3} fill="currentColor" />
        ),
      )}
    </svg>
  );
}
