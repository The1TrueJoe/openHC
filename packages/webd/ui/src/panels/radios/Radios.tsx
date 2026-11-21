import { useEffect, useRef, useState } from 'react';
import {
  Plus, Minus, Lightbulb, ToggleRight, Plug, Thermometer, Droplets, DoorOpen, PersonStanding, Lock, Gauge, Battery,
  BatteryLow, Signal, ShieldCheck, ShieldAlert, Zap, Flame, Sun, Radio, Cpu, X, Pencil, RefreshCw, Trash2, Check,
  CircleHelp, Gamepad2, History, ChevronDown,
} from 'lucide-react';
import {
  type Device, type Kind, type Proto, type Reading, useTree, zigbeeDevices, zwaveDevices, info, present, describe,
  setOn, setLevel, setColorTemp, rename, reinterview, remove, trees,
} from './model';
import { AddDevice } from './AddDevice';

/* Devices: everything on the box's Zigbee and Z-Wave networks, as things you
   can see and use. The networks themselves are run by zigbee-bridge and
   zwave-bridge (see model.ts); this page never shows their topics. */

export { startRadios, useTree, present } from './model';

export function RadiosPanel() {
  const zb = useTree('zigbee');
  const zw = useTree('zwave');
  const protos: Proto[] = [...(present(zb) ? (['zigbee'] as const) : []), ...(present(zw) ? (['zwave'] as const) : [])];
  const devices = [...(present(zb) ? zigbeeDevices(zb) : []), ...(present(zw) ? zwaveDevices(zw) : [])];
  const [filter, setFilter] = useState<Proto | 'all'>('all');
  const [open, setOpen] = useState<string | null>(null);
  const [wizard, setWizard] = useState<null | 'add' | 'remove'>(null);
  const shown = devices.filter((d) => filter === 'all' || d.proto === filter);
  const selected = devices.find((d) => d.key === open);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <div>
          <h1 className="text-base font-semibold tracking-tight">Devices</h1>
          <p className="text-xs text-muted">
            {devices.length ? `${devices.length} on ${protos.map(label).join(' and ')}` : `Nothing paired yet on ${protos.map(label).join(' or ')}`}
          </p>
        </div>
        <div className="ml-auto flex gap-2">
          {protos.includes('zwave') && (
            <button className={ghost} onClick={() => setWizard('remove')}>
              <Minus size={15} /> Remove a device
            </button>
          )}
          <button className={primary} onClick={() => setWizard('add')}>
            <Plus size={15} /> Add device
          </button>
        </div>
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        {present(zb) && <NetworkCard proto="zigbee" />}
        {present(zw) && <NetworkCard proto="zwave" />}
      </div>

      {protos.length > 1 && devices.length > 0 && (
        <div className="flex gap-1">
          {(['all', ...protos] as const).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`rounded-lg px-3 py-1 text-xs ${filter === f ? 'bg-accent/15 text-ink' : 'text-muted hover:text-ink'}`}
            >
              {f === 'all' ? 'All' : label(f)}
            </button>
          ))}
        </div>
      )}

      {shown.length ? (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {shown.map((d) => <DeviceCard key={d.key} d={d} onOpen={() => setOpen(d.key)} />)}
        </div>
      ) : (
        <div className="hair rounded-xl border border-dashed bg-panel p-8 text-center">
          <Radio size={22} className="mx-auto mb-2 text-muted" />
          <p className="text-sm">No devices yet</p>
          <p className="mt-1 text-xs text-muted">Add a light, plug, switch or sensor and it shows up here.</p>
          <button className={`${primary} mt-4`} onClick={() => setWizard('add')}>
            <Plus size={15} /> Add device
          </button>
        </div>
      )}

      <Activity />

      {selected && <DeviceSheet d={selected} onClose={() => setOpen(null)} onExclude={() => { setOpen(null); setWizard('remove'); }} />}
      {wizard && <AddDevice protos={protos} mode={wizard} onClose={() => setWizard(null)} />}
    </div>
  );
}

export const label = (p: Proto) => (p === 'zigbee' ? 'Zigbee' : 'Z-Wave');
export const primary = 'inline-flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-sm font-medium text-white hover:opacity-90 disabled:opacity-40';
export const ghost = 'hair inline-flex items-center gap-1.5 rounded-lg border px-3 py-1.5 text-sm text-ink hover:border-accent/50 disabled:opacity-40';

/* ---------------- networks ---------------- */

function NetworkCard({ proto }: { proto: Proto }) {
  const t = useTree(proto);
  const i = info(t) ?? {};
  const state = t.topics.get('bridge/state') ?? '';
  const online = /online/.test(state);
  const rows: [string, string][] =
    proto === 'zigbee'
      ? [
          ['Coordinator', `${(i.coordinator?.type ?? '').toUpperCase()} ${i.coordinator?.firmware ?? ''}`.trim() || '—'],
          ['Channel', String(i.network?.channel ?? '—')],
          ['PAN', i.network?.pan_id !== undefined ? `0x${Number(i.network.pan_id).toString(16).padStart(4, '0')}` : '—'],
        ]
      : [
          ['Controller', i.library_version ?? '—'],
          ['Home ID', i.home_id ?? '—'],
          ['Node', String(i.node_id ?? '—')],
        ];
  const pairing = proto === 'zigbee' && i.permit_join;
  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <div className="mb-2 flex items-center gap-2">
        <Radio size={15} className="text-muted" />
        <h2 className="text-sm font-medium">{label(proto)}</h2>
        <span className={`ml-1 inline-flex items-center gap-1 text-xs ${online ? 'text-live' : 'text-alarm'}`}>
          <span className={`h-1.5 w-1.5 rounded-full ${online ? 'bg-live' : 'bg-alarm'}`} />
          {online ? (pairing ? 'Pairing open' : 'Online') : 'Offline'}
        </span>
        <button
          className="ml-auto text-xs text-muted hover:text-ink"
          title="Save the network's keys and settings to the box"
          onClick={() => trees[proto].send('bridge/request/backup')}
        >
          Back up
        </button>
      </div>
      <dl className="grid grid-cols-3 gap-2 text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="min-w-0">
            <dt className="text-muted">{k}</dt>
            <dd className="truncate font-mono" title={v}>{v}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

/* ---------------- device cards ---------------- */

const KIND_ICON: Record<Kind, typeof Lightbulb> = {
  light: Lightbulb, switch: ToggleRight, plug: Plug, sensor: Thermometer, contact: DoorOpen, motion: PersonStanding,
  leak: Droplets, lock: Lock, thermostat: Thermometer, remote: Gamepad2, unknown: CircleHelp,
};

const READING_ICON: Record<string, typeof Lightbulb> = {
  temperature: Thermometer, humidity: Droplets, battery: Battery, power: Zap, light: Sun, contact: DoorOpen,
  motion: PersonStanding, leak: Droplets, alarm: Flame, gauge: Gauge,
};

function DeviceCard({ d, onOpen }: { d: Device; onOpen: () => void }) {
  const Icon = KIND_ICON[d.kind];
  const lit = d.on === true;
  return (
    <div
      className="hair group flex cursor-pointer flex-col gap-3 rounded-xl border bg-panel p-4 transition hover:border-accent/40"
      onClick={onOpen}
    >
      <div className="flex items-start gap-3">
        <div className={`grid h-9 w-9 shrink-0 place-items-center rounded-lg ${lit ? 'bg-warm/20 text-warm' : 'bg-raised text-muted'}`}>
          <Icon size={18} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium">{d.name}</div>
          <div className="truncate text-xs text-muted">{[d.vendor, d.model].filter(Boolean).join(' · ') || label(d.proto)}</div>
        </div>
        {d.switchable && <OnOff d={d} />}
      </div>

      {d.level !== undefined && <LevelSlider d={d} />}

      {d.readings.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {d.readings.filter((r) => r.icon !== 'battery').slice(0, 4).map((r) => <ReadingChip key={r.key} r={r} />)}
        </div>
      )}

      <div className="mt-auto flex items-center gap-3 text-xs text-muted">
        <span className="inline-flex items-center gap-1">
          <span className={`h-1.5 w-1.5 rounded-full ${d.healthy ? 'bg-live' : d.interviewing ? 'bg-warm' : 'bg-alarm'}`} />
          {d.status}
        </span>
        <span className="ml-auto flex items-center gap-2">
          {d.battery !== undefined && (
            <span className={`inline-flex items-center gap-0.5 ${d.battery < 15 ? 'text-alarm' : ''}`}>
              {d.battery < 15 ? <BatteryLow size={13} /> : <Battery size={13} />}
              {d.battery}%
            </span>
          )}
          {d.signal && <span className="inline-flex items-center gap-0.5" title={d.signal}><Signal size={12} /></span>}
          {d.proto === 'zwave' &&
            (d.security ? <ShieldCheck size={13} className="text-live" aria-label="Secure" /> : <ShieldAlert size={13} aria-label="Not secure" />)}
          <span className="rounded bg-raised px-1.5 py-0.5 text-[10px] uppercase tracking-wide">{label(d.proto)}</span>
        </span>
      </div>
    </div>
  );
}

function ReadingChip({ r }: { r: Reading }) {
  const I = READING_ICON[r.icon] ?? Gauge;
  return (
    <span className={`inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs ${r.alert ? 'bg-alarm/15 text-alarm' : 'bg-raised'}`} title={r.label}>
      <I size={12} />
      {r.text}
    </span>
  );
}

/** The on/off switch. Optimistic: flips at once, and the device's own report
 *  (which the bridge reads back) settles it. */
function OnOff({ d }: { d: Device }) {
  const [want, setWant] = useState<boolean | null>(null);
  useEffect(() => setWant(null), [d.on]);
  const on = want ?? !!d.on;
  return (
    <button
      role="switch"
      aria-checked={on}
      aria-label={on ? 'Turn off' : 'Turn on'}
      onClick={(e) => {
        e.stopPropagation();
        setWant(!on);
        setOn(d, !on);
      }}
      className={`relative h-6 w-11 shrink-0 rounded-full transition ${on ? 'bg-accent' : 'bg-edge'}`}
    >
      <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all ${on ? 'left-[1.375rem]' : 'left-0.5'}`} />
    </button>
  );
}

function LevelSlider({ d }: { d: Device }) {
  return (
    <div className="flex items-center gap-2" onClick={(e) => e.stopPropagation()}>
      <Sun size={13} className="text-muted" />
      <Committed value={d.level ?? 0} min={1} max={100} onCommit={(v) => setLevel(d, v)} label="Brightness" />
      <span className="w-9 text-right text-xs tabular-nums text-muted">{d.level}%</span>
    </div>
  );
}

/** A range input that sends once, when released, not on every pixel. */
function Committed({ value, min, max, onCommit, label }: { value: number; min: number; max: number; onCommit: (v: number) => void; label: string }) {
  const [local, setLocal] = useState(value);
  const dragging = useRef(false);
  useEffect(() => { if (!dragging.current) setLocal(value); }, [value]);
  const done = () => { dragging.current = false; onCommit(local); };
  return (
    <input
      type="range"
      aria-label={label}
      min={min}
      max={max}
      value={local}
      onChange={(e) => { dragging.current = true; setLocal(Number(e.target.value)); }}
      onMouseUp={done}
      onTouchEnd={done}
      onKeyUp={done}
      className="w-full accent-accent"
    />
  );
}

/* ---------------- detail sheet ---------------- */

function DeviceSheet({ d, onClose, onExclude }: { d: Device; onClose: () => void; onExclude: () => void }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(d.name);
  const [raw, setRaw] = useState(false);
  const Icon = KIND_ICON[d.kind];
  useEffect(() => setName(d.name), [d.name]);
  const save = () => {
    const n = name.trim();
    if (n && n !== d.name) rename(d, n);
    setEditing(false);
  };
  const dead = d.proto === 'zwave' && d.status === 'Not responding';
  return (
    <div className="fixed inset-0 z-40 flex justify-end bg-black/30" onMouseDown={onClose}>
      <aside
        className="hair flex h-full w-full max-w-md flex-col overflow-y-auto border-l bg-panel shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="hair flex items-center gap-3 border-b p-4">
          <div className="grid h-10 w-10 place-items-center rounded-lg bg-raised text-muted"><Icon size={20} /></div>
          <div className="min-w-0 flex-1">
            {editing ? (
              <div className="flex gap-1">
                <input
                  autoFocus
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  onKeyDown={(e) => { if (e.key === 'Enter') save(); if (e.key === 'Escape') setEditing(false); }}
                  className="hair w-full rounded-md border bg-raised px-2 py-1 text-sm outline-none focus:border-accent/50"
                />
                <button className={ghost} onClick={save} aria-label="Save name"><Check size={14} /></button>
              </div>
            ) : (
              <button className="flex items-center gap-1.5 text-left text-sm font-medium hover:text-accent" onClick={() => setEditing(true)}>
                {d.name} <Pencil size={12} className="text-muted" />
              </button>
            )}
            <div className="text-xs text-muted">{[label(d.proto), d.vendor, d.model].filter(Boolean).join(' · ')}</div>
          </div>
          <button className="text-muted hover:text-ink" onClick={onClose} aria-label="Close"><X size={18} /></button>
        </div>

        <div className="space-y-5 p-4">
          {(d.switchable || d.level !== undefined || d.colorTemp) && (
            <section className="space-y-3">
              <h3 className="text-xs font-medium uppercase tracking-wide text-muted">Control</h3>
              {d.switchable && (
                <div className="flex items-center justify-between text-sm">
                  <span>{d.on === undefined ? 'Not reported yet' : d.on ? 'On' : 'Off'}</span>
                  <OnOff d={d} />
                </div>
              )}
              {d.level !== undefined && <LevelSlider d={d} />}
              {d.colorTemp && (
                <div className="flex items-center gap-2">
                  <span className="text-xs text-muted">Warm</span>
                  <Committed value={d.colorTemp.mireds} min={d.colorTemp.min} max={d.colorTemp.max} onCommit={(v) => setColorTemp(d, v)} label="Colour temperature" />
                  <span className="text-xs text-muted">Cool</span>
                </div>
              )}
            </section>
          )}

          {d.readings.length > 0 && (
            <section className="space-y-2">
              <h3 className="text-xs font-medium uppercase tracking-wide text-muted">Readings</h3>
              <div className="grid grid-cols-2 gap-2">
                {d.readings.map((r) => {
                  const I = READING_ICON[r.icon] ?? Gauge;
                  return (
                    <div key={r.key} className={`rounded-lg p-2.5 ${r.alert ? 'bg-alarm/10' : 'bg-raised'}`}>
                      <div className="flex items-center gap-1 text-xs text-muted"><I size={12} /> {r.label}</div>
                      <div className={`mt-0.5 text-sm font-medium ${r.alert ? 'text-alarm' : ''}`}>{r.text}</div>
                    </div>
                  );
                })}
              </div>
            </section>
          )}

          <section className="space-y-2">
            <h3 className="text-xs font-medium uppercase tracking-wide text-muted">Status</h3>
            <div className="flex flex-wrap gap-2 text-xs">
              <span className="inline-flex items-center gap-1 rounded-md bg-raised px-2 py-1">
                <span className={`h-1.5 w-1.5 rounded-full ${d.healthy ? 'bg-live' : d.interviewing ? 'bg-warm' : 'bg-alarm'}`} />
                {d.status}
              </span>
              {d.signal && <span className="inline-flex items-center gap-1 rounded-md bg-raised px-2 py-1"><Signal size={12} /> {d.signal}</span>}
              <span className={`inline-flex items-center gap-1 rounded-md px-2 py-1 ${d.security ? 'bg-live/15 text-live' : 'bg-raised'}`}>
                {d.security ? <ShieldCheck size={12} /> : <ShieldAlert size={12} />}
                {d.security ? d.security.join(', ') : d.proto === 'zwave' ? 'Not secure' : 'Network key'}
              </span>
            </div>
          </section>

          <section className="space-y-1">
            <h3 className="text-xs font-medium uppercase tracking-wide text-muted">Device</h3>
            <dl className="divide-y text-xs hair">
              {d.details.map(([k, v]) => (
                <div key={k} className="hair flex gap-3 py-1.5">
                  <dt className="w-32 shrink-0 text-muted">{k}</dt>
                  <dd className="min-w-0 break-words font-mono">{v}</dd>
                </div>
              ))}
            </dl>
            <button className="mt-1 inline-flex items-center gap-1 text-xs text-muted hover:text-ink" onClick={() => setRaw(!raw)}>
              <Cpu size={12} /> {raw ? 'Hide' : 'Show'} technical record <ChevronDown size={12} className={raw ? 'rotate-180' : ''} />
            </button>
            {raw && <pre className="hair max-h-64 overflow-auto rounded-lg border bg-raised p-2 text-[11px]">{JSON.stringify(d.raw, null, 2)}</pre>}
          </section>

          <section className="flex flex-wrap gap-2 pt-1">
            <button className={ghost} onClick={() => reinterview(d)}>
              <RefreshCw size={14} /> Set up again
            </button>
            {d.proto === 'zigbee' ? (
              <button
                className={`${ghost} text-alarm`}
                onClick={() => {
                  if (!confirm(`Remove ${d.name} from the network?`)) return;
                  remove(d, !d.healthy);
                  onClose();
                }}
              >
                <Trash2 size={14} /> Remove
              </button>
            ) : dead ? (
              <button
                className={`${ghost} text-alarm`}
                onClick={() => {
                  if (!confirm(`${d.name} does not answer. Remove it from the controller?`)) return;
                  remove(d);
                  onClose();
                }}
              >
                <Trash2 size={14} /> Remove
              </button>
            ) : (
              <button className={`${ghost} text-alarm`} onClick={onExclude}>
                <Trash2 size={14} /> Remove…
              </button>
            )}
          </section>
        </div>
      </aside>
    </div>
  );
}

/* ---------------- activity ---------------- */

function Activity() {
  const zb = useTree('zigbee');
  const zw = useTree('zwave');
  const [open, setOpen] = useState(false);
  const lines = [
    ...zb.log.map((l) => ({ at: l.at, s: describe('zigbee', l.tail, l.data) })),
    ...zw.log.map((l) => ({ at: l.at, s: describe('zwave', l.tail, l.data) })),
  ]
    .filter((l): l is { at: number; s: string } => !!l.s)
    .sort((a, b) => b.at - a.at);
  if (!lines.length) return null;
  return (
    <section className="hair rounded-xl border bg-panel">
      <button className="flex w-full items-center gap-2 px-4 py-3 text-sm" onClick={() => setOpen(!open)}>
        <History size={14} className="text-muted" /> Recent activity
        <span className="text-xs text-muted">{lines[0].s}</span>
        <ChevronDown size={14} className={`ml-auto text-muted transition ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <ul className="hair space-y-1 border-t px-4 py-3 text-xs">
          {lines.slice(0, 30).map((l, i) => (
            <li key={i} className="flex gap-3">
              <span className="w-16 shrink-0 tabular-nums text-muted">{new Date(l.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}</span>
              <span>{l.s}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
