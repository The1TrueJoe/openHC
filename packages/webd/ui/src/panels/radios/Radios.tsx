import { useEffect, useRef, useState } from 'react';
import {
  Plus, Minus, Lightbulb, ToggleRight, Plug, Thermometer, Droplets, DoorOpen, PersonStanding, Lock, Gauge, Battery,
  BatteryLow, Signal, ShieldCheck, ShieldAlert, Zap, Flame, Sun, Radio, Cpu, X, Pencil, RefreshCw, Trash2, Check,
  CircleHelp, Gamepad2, History, ChevronDown, Save,
} from 'lucide-react';
import {
  type Control, type Device, type Kind, type Proto, type Reading, useTree, zigbeeDevices, zwaveDevices, info, describe,
  setOn, setLevel, setColorTemp, rename, reinterview, remove, trees, zwaveParams, configure,
} from './model';
import { AddDevice } from './AddDevice';

/* Devices: everything on the box's Zigbee and Z-Wave networks, as things you
   can see and use. The networks themselves are run by zigbee-bridge and
   zwave-bridge (see model.ts); this page never shows their topics. */

export { startRadios, useTree, present } from './model';

export function RadiosPanel({ proto }: { proto: Proto }) {
  const t = useTree(proto);
  const devices = proto === 'zigbee' ? zigbeeDevices(t) : zwaveDevices(t);
  const [open, setOpen] = useState<string | null>(null);
  const [wizard, setWizard] = useState<null | 'add' | 'remove'>(null);
  const selected = devices.find((d) => d.key === open);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <div>
          <h1 className="text-base font-semibold tracking-tight">{label(proto)}</h1>
          <p className="text-xs text-muted">
            {devices.length ? `${devices.length} device${devices.length === 1 ? '' : 's'}` : 'Nothing paired yet'}
          </p>
        </div>
        <div className="ml-auto flex gap-2">
          {proto === 'zwave' && (
            <button className={ghost} onClick={() => setWizard('remove')}>
              <Minus size={15} /> Remove a device
            </button>
          )}
          <button className={primary} onClick={() => setWizard('add')}>
            <Plus size={15} /> Add device
          </button>
        </div>
      </div>

      <NetworkCard proto={proto} />

      {devices.length ? (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {devices.map((d) => <DeviceCard key={d.key} d={d} onOpen={() => setOpen(d.key)} />)}
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

      <Activity proto={proto} />

      {selected && <DeviceSheet d={selected} onClose={() => setOpen(null)} onExclude={() => { setOpen(null); setWizard('remove'); }} />}
      {wizard && <AddDevice protos={[proto]} mode={wizard} onClose={() => setWizard(null)} />}
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
      {proto === 'zigbee' && <ChannelSettings current={i.network?.channel} />}
    </section>
  );
}

/** Channels 15, 20, 25 and 26 sit between Wi-Fi's 1, 6 and 11. */
const BETWEEN_WIFI = [15, 20, 25, 26];

function ChannelSettings({ current }: { current?: number }) {
  const t = useTree('zigbee');
  const [open, setOpen] = useState(false);
  const [asked, setAsked] = useState(0);
  const [pick, setPick] = useState<number | null>(null);
  const scan = t.log.find((l) => l.tail === 'bridge/response/energy_scan' && l.at >= asked);
  const moved = t.log.find((l) => l.tail === 'bridge/response/change_channel' && l.at >= asked);
  const channels: { channel: number; max_rssi: number }[] = scan?.data?.data?.channels ?? [];
  const scanning = asked > 0 && !scan;
  const quietest = [...channels].filter((c) => BETWEEN_WIFI.includes(c.channel)).sort((a, b) => a.max_rssi - b.max_rssi)[0]?.channel;
  const run = () => { setAsked(Date.now()); trees.zigbee.send('bridge/request/energy_scan'); };
  return (
    <div className="hair mt-3 border-t pt-3">
      <button className="flex w-full items-center gap-2 text-xs text-muted hover:text-ink" onClick={() => setOpen(!open)}>
        Channel settings <ChevronDown size={13} className={`transition ${open ? 'rotate-180' : ''}`} />
      </button>
      {open && (
        <div className="mt-3 space-y-3">
          <p className="text-xs text-muted">
            Zigbee shares 2.4 GHz with Wi-Fi. Channels 15, 20, 25 and 26 fall between Wi-Fi's 1, 6 and 11; a scan
            measures how busy each channel is right here. Moving takes the whole network along, without pairing again;
            sleepy devices follow when they next wake.
          </p>
          <div className="flex items-center gap-2">
            <button className={ghost} disabled={scanning} onClick={run}>
              {scanning ? <RefreshCw size={14} className="animate-spin" /> : <Signal size={14} />} {scanning ? 'Scanning…' : 'Scan channels'}
            </button>
            {quietest && <span className="text-xs text-muted">Quietest between Wi-Fi: <b className="text-ink">{quietest}</b></span>}
          </div>
          <div className="flex h-28 items-end gap-1">
            {Array.from({ length: 16 }, (_, k) => k + 11).map((ch) => {
              const r = channels.find((c) => c.channel === ch)?.max_rssi;
              // -100 dBm (silent) to -20 dBm (loud) as 4% to 100% height.
              const h = r === undefined ? 4 : Math.max(4, Math.min(100, ((r + 100) / 80) * 100));
              const isCur = ch === current;
              const sel = ch === pick;
              return (
                <button
                  key={ch}
                  title={r === undefined ? `Channel ${ch}` : `Channel ${ch}: ${r} dBm`}
                  onClick={() => setPick(ch === current ? null : ch)}
                  className="flex h-full flex-1 flex-col items-center justify-end gap-1"
                >
                  <span
                    className={`w-full rounded-t ${sel ? 'bg-accent' : isCur ? 'bg-live' : BETWEEN_WIFI.includes(ch) ? 'bg-muted/50' : 'bg-edge'}`}
                    style={{ height: `${h}%` }}
                  />
                  <span className={`text-[10px] tabular-nums ${isCur ? 'font-semibold text-live' : sel ? 'text-accent' : 'text-muted'}`}>{ch}</span>
                </button>
              );
            })}
          </div>
          <div className="flex items-center gap-2 text-xs">
            <span className="text-muted">Lower bars are quieter. Current: <b className="text-live">{current ?? '?'}</b></span>
            <button
              className={`${primary} ml-auto`}
              disabled={!pick}
              onClick={() => {
                if (!pick || !confirm(`Move the Zigbee network to channel ${pick}? Every device follows; sleepy ones when they next wake.`)) return;
                setAsked(Date.now());
                trees.zigbee.send('bridge/request/change_channel', { channel: pick });
                setPick(null);
              }}
            >
              Move to {pick ?? '…'}
            </button>
          </div>
          {moved?.data?.status === 'ok' && <p className="text-xs text-live">Moved to channel {moved.data.data?.channel}.</p>}
        </div>
      )}
    </div>
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
  const lit = d.controls.some((c) => c.on);
  const single = d.controls.length === 1 ? d.controls[0] : null;
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
        {single && <OnOff d={d} c={single} />}
      </div>

      {single?.level !== undefined && <LevelSlider d={d} c={single} />}
      {d.controls.length > 1 && <Channels d={d} />}

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
function OnOff({ d, c }: { d: Device; c: Control }) {
  const [want, setWant] = useState<boolean | null>(null);
  useEffect(() => setWant(null), [c.on]);
  const on = want ?? !!c.on;
  return (
    <button
      role="switch"
      aria-checked={on}
      aria-label={on ? 'Turn off' : 'Turn on'}
      onClick={(e) => {
        e.stopPropagation();
        setWant(!on);
        setOn(d, c, !on);
      }}
      className={`relative h-6 w-11 shrink-0 rounded-full transition ${on ? 'bg-accent' : 'bg-edge'}`}
    >
      <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all ${on ? 'left-[1.375rem]' : 'left-0.5'}`} />
    </button>
  );
}

function LevelSlider({ d, c }: { d: Device; c: Control }) {
  return (
    <div className="flex items-center gap-2" onClick={(e) => e.stopPropagation()}>
      <Sun size={13} className="text-muted" />
      <Committed value={c.level ?? 0} min={1} max={100} onCommit={(v) => setLevel(d, c, v)} label={`${c.label} brightness`} />
      <span className="w-9 text-right text-xs tabular-nums text-muted">{c.level}%</span>
    </div>
  );
}

/** A multi-channel device (a double relay, a two-gang switch): a row per channel. */
function Channels({ d }: { d: Device }) {
  return (
    <div className="space-y-1.5" onClick={(e) => e.stopPropagation()}>
      {d.controls.map((c) => (
        <div key={c.ep} className="space-y-1 rounded-lg bg-raised px-2.5 py-1.5">
          <div className="flex items-center gap-2 text-sm">
            <span className="min-w-0 flex-1 truncate">{c.label}</span>
            <span className="text-xs text-muted">{c.on === undefined ? '' : c.on ? 'On' : 'Off'}</span>
            <OnOff d={d} c={c} />
          </div>
          {c.level !== undefined && <LevelSlider d={d} c={c} />}
        </div>
      ))}
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
          {(d.controls.length > 0 || d.colorTemp) && (
            <section className="space-y-3">
              <h3 className="text-xs font-medium uppercase tracking-wide text-muted">Control</h3>
              {d.controls.length === 1 ? (
                <>
                  <div className="flex items-center justify-between text-sm">
                    <span>{d.controls[0].on === undefined ? 'Not reported yet' : d.controls[0].on ? 'On' : 'Off'}</span>
                    <OnOff d={d} c={d.controls[0]} />
                  </div>
                  {d.controls[0].level !== undefined && <LevelSlider d={d} c={d.controls[0]} />}
                </>
              ) : d.controls.length > 1 ? <Channels d={d} /> : null}
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

          {d.proto === 'zwave' && <ZwaveSettings id={d.id} />}

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

/* ---------------- Z-Wave settings (configuration parameters) ---------------- */

function ZwaveSettings({ id }: { id: string }) {
  const t = useTree('zwave');
  const params = zwaveParams(t, id);
  const [edits, setEdits] = useState<Record<string, number>>({});
  if (!params.length) {
    return (
      <section className="space-y-1">
        <h3 className="text-xs font-medium uppercase tracking-wide text-muted">Settings</h3>
        <p className="text-xs text-muted">No settings known for this device: it is not in the device library, or it has not been identified yet.</p>
      </section>
    );
  }
  const dirty = Object.keys(edits).length > 0;
  return (
    <section className="space-y-2">
      <div className="flex items-center">
        <h3 className="text-xs font-medium uppercase tracking-wide text-muted">Settings</h3>
        <button
          className={`${primary} ml-auto px-2.5 py-1 text-xs`}
          disabled={!dirty}
          onClick={() => { configure(id, edits); setEdits({}); }}
        >
          <Save size={13} /> Save{dirty ? ` (${Object.keys(edits).length})` : ''}
        </button>
      </div>
      <div className="space-y-2">
        {params.map((p) => {
          const shown = edits[p.key] ?? p.value ?? p.default;
          const set = (v: number) => setEdits((e) => {
            const n = { ...e };
            if (v === (p.value ?? p.default)) delete n[p.key]; else n[p.key] = v;
            return n;
          });
          return (
            <div key={p.key} className="rounded-lg bg-raised p-2.5">
              <div className="flex items-start gap-2">
                <div className="min-w-0 flex-1">
                  <div className="text-sm">{p.label}</div>
                  {p.description && <div className="mt-0.5 text-xs text-muted">{p.description}</div>}
                </div>
                <div className="shrink-0">
                  {p.readOnly ? (
                    <span className="text-sm">{p.options.find((o) => o.value === shown)?.label ?? shown}</span>
                  ) : p.options.length ? (
                    <select
                      value={shown}
                      onChange={(e) => set(Number(e.target.value))}
                      className="hair max-w-48 rounded-md border bg-panel px-2 py-1 text-sm outline-none focus:border-accent/50"
                    >
                      {p.options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
                    </select>
                  ) : (
                    <span className="flex items-center gap-1">
                      <input
                        type="number"
                        min={p.min}
                        max={p.max}
                        value={shown}
                        onChange={(e) => set(Math.max(p.min, Math.min(p.max, Number(e.target.value))))}
                        className="hair w-24 rounded-md border bg-panel px-2 py-1 text-right text-sm outline-none focus:border-accent/50"
                      />
                      {p.unit && <span className="text-xs text-muted">{p.unit}</span>}
                    </span>
                  )}
                </div>
              </div>
              <div className="mt-1 text-[10px] text-muted">
                #{p.key}{p.value === undefined ? ' · not read yet' : ''}{edits[p.key] !== undefined ? ' · changed' : ''}
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

/* ---------------- activity ---------------- */

function Activity({ proto }: { proto: Proto }) {
  const t = useTree(proto);
  const [open, setOpen] = useState(false);
  const lines = t.log
    .map((l) => ({ at: l.at, s: describe(proto, l.tail, l.data) }))
    .filter((l): l is { at: number; s: string } => !!l.s);
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
