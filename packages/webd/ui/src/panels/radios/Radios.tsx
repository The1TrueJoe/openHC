import { useEffect, useRef, useState } from 'react';
import {
  Plus, Minus, Lightbulb, ToggleRight, Plug, Thermometer, Droplets, DoorOpen, PersonStanding, Lock, Gauge, Battery,
  BatteryLow, Signal, ShieldCheck, ShieldAlert, Zap, Flame, Sun, Radio, Waves, Cpu, X, Pencil, RefreshCw, Trash2, Check,
  CircleHelp, Gamepad2, History, ChevronDown, ChevronRight, Save, Search, LayoutList, Share2, SlidersHorizontal, HardDriveDownload,
} from 'lucide-react';
import {
  type Control, type Device, type Kind, type Proto, type Reading, useTree, zigbeeDevices, zwaveDevices, info, describe,
  setOn, setLevel, setColorTemp, rename, reinterview, remove, trees, zwaveParams, configure,
} from './model';
import { AddDevice } from './AddDevice';
import { NetworkMap } from './NetworkMap';

/* Devices: everything on the box's Zigbee and Z-Wave networks, as things you
   can see and use. The networks themselves are run by zigbee-bridge and
   zwave-bridge (see model.ts); this page never shows their topics. */

export { startRadios, useTree, present } from './model';

type Tab = 'devices' | 'map' | 'activity' | 'network';

/** The last sub-tab per radio, kept in the browser (a convenience; it may be gone). */
function useStoredTab(proto: Proto): [Tab, (t: Tab) => void] {
  const key = `ohc.radios.${proto}.tab`;
  const [tab, set] = useState<Tab>(() => {
    try { return (localStorage.getItem(key) as Tab) || 'devices'; } catch { return 'devices'; }
  });
  return [tab, (t) => { set(t); try { localStorage.setItem(key, t); } catch { /* private mode */ } }];
}

export function RadiosPanel({ proto }: { proto: Proto }) {
  const t = useTree(proto);
  const i = info(t) ?? {};
  const devices = proto === 'zigbee' ? zigbeeDevices(t) : zwaveDevices(t);
  const [tab, setTab] = useStoredTab(proto);
  const [open, setOpen] = useState<string | null>(null);
  const [wizard, setWizard] = useState<null | 'add' | 'remove'>(null);
  const selected = devices.find((d) => d.key === open);
  const online = /online/.test(t.topics.get('bridge/state') ?? '');
  const pairing = proto === 'zigbee' && !!i.permit_join;
  const attention = devices.filter((d) => !d.healthy && !d.interviewing).length;
  const Icon = proto === 'zigbee' ? Radio : Waves;
  const tabs: { id: Tab; label: string; icon: typeof Radio; badge?: number }[] = [
    { id: 'devices', label: 'Devices', icon: LayoutList, badge: devices.length },
    { id: 'map', label: 'Network map', icon: Share2 },
    { id: 'activity', label: 'Activity', icon: History },
    { id: 'network', label: proto === 'zigbee' ? 'Network' : 'Controller', icon: SlidersHorizontal },
  ];

  return (
    <div className="flex min-h-full flex-col gap-4">
      <header className="flex flex-wrap items-center gap-3">
        <div className="grid h-10 w-10 place-items-center rounded-xl bg-accent/15 text-accent"><Icon size={20} /></div>
        <div className="min-w-0">
          <h1 className="text-lg font-semibold leading-tight tracking-tight">{label(proto)}</h1>
          <div className="flex flex-wrap items-center gap-x-2 text-xs text-muted">
            <span className={`inline-flex items-center gap-1 ${online ? (pairing ? 'text-accent' : 'text-live') : 'text-alarm'}`}>
              <span className={`h-1.5 w-1.5 rounded-full ${online ? (pairing ? 'animate-pulse bg-accent' : 'bg-live') : 'bg-alarm'}`} />
              {online ? (pairing ? 'Pairing open' : 'Online') : 'Offline'}
            </span>
            <span>·</span>
            <span>{devices.length} device{devices.length === 1 ? '' : 's'}</span>
            {attention > 0 && <><span>·</span><span className="text-alarm">{attention} not responding</span></>}
            <span>·</span>
            <span className="font-mono">{proto === 'zigbee' ? `EZSP ${i.coordinator?.firmware ?? '?'} · ch ${i.network?.channel ?? '?'}` : `${i.library_version ?? '?'} · home ${i.home_id ?? '?'}`}</span>
          </div>
        </div>
        <div className="ml-auto flex gap-2">
          {proto === 'zwave' && (
            <button className={ghost} onClick={() => setWizard('remove')}><Minus size={15} /> Remove device</button>
          )}
          <button className={primary} onClick={() => setWizard('add')}><Plus size={15} /> Add device</button>
        </div>
      </header>

      <nav className="hair flex gap-1 overflow-x-auto border-b [scrollbar-width:none] [&::-webkit-scrollbar]:hidden" role="tablist">
        {tabs.map((x) => (
          <button
            key={x.id}
            role="tab"
            aria-selected={tab === x.id}
            onClick={() => setTab(x.id)}
            className={`-mb-px inline-flex items-center gap-1.5 whitespace-nowrap border-b-2 px-3 py-2 text-sm transition ${
              tab === x.id ? 'border-accent text-ink' : 'border-transparent text-muted hover:text-ink'
            }`}
          >
            <x.icon size={14} /> {x.label}
            {x.badge !== undefined && <span className="rounded-full bg-raised px-1.5 text-[10px] tabular-nums text-muted">{x.badge}</span>}
          </button>
        ))}
      </nav>

      {tab === 'devices' && <DeviceList proto={proto} devices={devices} onOpen={setOpen} onAdd={() => setWizard('add')} />}
      {tab === 'map' && <NetworkMap proto={proto} embedded />}
      {tab === 'activity' && <ActivityList proto={proto} />}
      {tab === 'network' && <NetworkTab proto={proto} devices={devices} onExclude={() => setWizard('remove')} />}

      {selected && <DeviceSheet d={selected} onClose={() => setOpen(null)} onExclude={() => { setOpen(null); setWizard('remove'); }} />}
      {wizard && <AddDevice protos={[proto]} mode={wizard} onClose={() => setWizard(null)} />}
    </div>
  );
}

export const label = (p: Proto) => (p === 'zigbee' ? 'Zigbee' : 'Z-Wave');
export const primary = 'inline-flex items-center gap-1.5 rounded-lg bg-accent px-3 py-1.5 text-sm font-medium text-white hover:opacity-90 disabled:opacity-40';
export const ghost = 'hair inline-flex items-center gap-1.5 rounded-lg border px-3 py-1.5 text-sm text-ink hover:border-accent/50 disabled:opacity-40';
const card = 'hair rounded-xl border bg-panel p-4';
const h2 = 'mb-3 flex items-center gap-2 text-sm font-medium';

/* ---------------- network / controller tab ---------------- */

function Facts({ rows }: { rows: [string, React.ReactNode][] }) {
  return (
    <dl className="divide-y text-sm hair">
      {rows.map(([k, v]) => (
        <div key={k} className="hair flex gap-3 py-2">
          <dt className="w-36 shrink-0 text-muted">{k}</dt>
          <dd className="min-w-0 break-words font-mono text-xs leading-5">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

function NetworkTab({ proto, devices, onExclude }: { proto: Proto; devices: Device[]; onExclude: () => void }) {
  const t = useTree(proto);
  const i = info(t) ?? {};
  const backup = t.log.find((l) => l.tail === 'bridge/response/backup');
  const hex = (n: unknown, w = 4) => (typeof n === 'number' ? `0x${n.toString(16).padStart(w, '0')}` : '—');
  const rows: [string, React.ReactNode][] = proto === 'zigbee'
    ? [
        ['Coordinator', `${(i.coordinator?.type ?? '').toUpperCase()} · EmberZNet ${i.coordinator?.firmware ?? '?'} · protocol ${i.coordinator?.protocol ?? '?'}`],
        ['IEEE address', i.coordinator?.ieee_address ?? '—'],
        ['Channel', String(i.network?.channel ?? '—')],
        ['PAN ID', hex(i.network?.pan_id)],
        ['Extended PAN ID', i.network?.extended_pan_id ?? '—'],
        ['Network update ID', String(i.network?.update_id ?? '—')],
        ['Pairing', i.permit_join ? `open (${i.permit_join_timeout ?? '?'} s)` : 'closed'],
        ['Bridge', `zigbee-bridge ${i.version ?? ''}`],
      ]
    : [
        ['Controller', `${String(i.library_type ?? '?').replace(/^./, (c: string) => c.toUpperCase())} · ${i.library_version ?? '?'}`],
        ['Firmware', i.firmware ?? '—'],
        ['Home ID', i.home_id ?? '—'],
        ['Node ID', String(i.node_id ?? '—')],
        ['SIS', i.sis ? 'yes (this controller)' : 'no'],
        ['Region', i.region ?? 'set by firmware'],
        ['Chip', i.chip_type !== undefined ? `type ${i.chip_type}` : '—'],
        ['Bridge', `zwave-bridge ${i.version ?? ''}`],
      ];
  const dead = devices.filter((d) => proto === 'zwave' && d.status === 'Not responding');
  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <section className={card}>
        <h2 className={h2}><Cpu size={15} className="text-muted" /> {proto === 'zigbee' ? 'Coordinator' : 'Controller'}</h2>
        <Facts rows={rows} />
      </section>
      <div className="space-y-4">
        {proto === 'zigbee' && (
          <section className={card}>
            <h2 className={h2}><Signal size={15} className="text-muted" /> Channel</h2>
            <ChannelSettings current={i.network?.channel} />
          </section>
        )}
        <section className={card}>
          <h2 className={h2}><HardDriveDownload size={15} className="text-muted" /> Backup</h2>
          <p className="mb-3 text-xs text-muted">
            {proto === 'zigbee'
              ? 'The network key, PAN and every device\'s link key, saved on the box. A restore brings the same network back onto a new coordinator, with nothing paired again. The bridge also backs up after changes.'
              : 'The controller\'s memory (its node table, routes and keys), saved on the box, to bring the same network back onto a replacement controller.'}
          </p>
          <div className="flex items-center gap-3">
            <button className={ghost} onClick={() => trees[proto].send('bridge/request/backup')}><Save size={14} /> Back up now</button>
            {backup && (
              <span className="text-xs text-muted">
                {backup.data?.status === 'ok' || backup.data?.ok
                  ? `Saved ${new Date(backup.at).toLocaleTimeString()}${backup.data?.data?.path ?? backup.data?.file ? ` to ${backup.data?.data?.path ?? backup.data?.file}` : ''}`
                  : `Failed: ${backup.data?.error ?? 'unknown'}`}
              </span>
            )}
          </div>
        </section>
        {proto === 'zwave' && (
          <section className={card}>
            <h2 className={h2}><Trash2 size={15} className="text-muted" /> Remove devices</h2>
            <p className="mb-3 text-xs text-muted">
              A working device leaves by exclusion: start it here, then press the device's button. Any device can be
              excluded this way, even one from another controller.
            </p>
            <button className={ghost} onClick={onExclude}><Minus size={14} /> Remove a device</button>
            {dead.length > 0 && (
              <div className="mt-4 space-y-2">
                <div className="text-xs text-muted">Not responding: these can be removed without the device.</div>
                {dead.map((d) => (
                  <div key={d.key} className="flex items-center gap-2 rounded-lg bg-raised px-3 py-2 text-sm">
                    <span className="min-w-0 flex-1 truncate">{d.name} <span className="text-xs text-muted">node {d.id}</span></span>
                    <button
                      className="text-xs text-alarm hover:underline"
                      onClick={() => confirm(`Remove ${d.name} (node ${d.id})? It no longer answers.`) && remove(d)}
                    >
                      Remove
                    </button>
                  </div>
                ))}
              </div>
            )}
          </section>
        )}
      </div>
    </div>
  );
}

/** Channels 15, 20, 25 and 26 sit between Wi-Fi's 1, 6 and 11. */
const BETWEEN_WIFI = [15, 20, 25, 26];

function ChannelSettings({ current }: { current?: number }) {
  const t = useTree('zigbee');
  const [asked, setAsked] = useState(0);
  const [pick, setPick] = useState<number | null>(null);
  const scan = t.log.find((l) => l.tail === 'bridge/response/energy_scan' && l.at >= asked);
  const moved = t.log.find((l) => l.tail === 'bridge/response/change_channel' && l.at >= asked);
  const channels: { channel: number; max_rssi: number }[] = scan?.data?.data?.channels ?? [];
  const scanning = asked > 0 && !scan;
  const quietest = [...channels].filter((c) => BETWEEN_WIFI.includes(c.channel)).sort((a, b) => a.max_rssi - b.max_rssi)[0]?.channel;
  const run = () => { setAsked(Date.now()); trees.zigbee.send('bridge/request/energy_scan'); };
  return (
    <div>
      {(
        <div className="space-y-3">
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

const KIND_NAME: Record<Kind, string> = {
  light: 'Light', switch: 'Switch', plug: 'Plug', sensor: 'Sensor', contact: 'Contact sensor', motion: 'Motion sensor',
  leak: 'Leak sensor', lock: 'Lock', thermostat: 'Thermostat', remote: 'Remote', unknown: 'Device',
};

function DeviceList({ proto, devices, onOpen, onAdd }: { proto: Proto; devices: Device[]; onOpen: (key: string) => void; onAdd: () => void }) {
  const [q, setQ] = useState('');
  const shown = devices.filter((d) => !q || `${d.name} ${d.vendor ?? ''} ${d.model ?? ''} ${d.id}`.toLowerCase().includes(q.toLowerCase()));
  if (!devices.length) {
    return (
      <div className="hair grid place-items-center rounded-xl border border-dashed bg-panel px-6 py-14 text-center">
        <div>
          {proto === 'zigbee' ? <Radio size={26} className="mx-auto mb-3 text-muted" /> : <Waves size={26} className="mx-auto mb-3 text-muted" />}
          <p className="text-sm font-medium">No {label(proto)} devices yet</p>
          <p className="mx-auto mt-1 max-w-sm text-xs text-muted">
            {proto === 'zigbee'
              ? 'Lights, plugs, switches and sensors from Hue, IKEA, Aqara, Sonoff and other Zigbee 3.0 makers.'
              : 'Switches, dimmers, relays, locks and sensors, secured with S2 as they join.'}
          </p>
          <button className={`${primary} mt-5`} onClick={onAdd}><Plus size={15} /> Add a device</button>
        </div>
      </div>
    );
  }
  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <label className="hair flex w-full max-w-xs items-center gap-2 rounded-lg border bg-panel px-2.5 py-1.5 focus-within:border-accent/50">
          <Search size={14} className="text-muted" />
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search devices" className="w-full bg-transparent text-sm outline-none" />
        </label>
        <span className="ml-auto text-xs text-muted">{shown.length} of {devices.length}</span>
      </div>
      <div className="hair overflow-hidden rounded-xl border bg-panel">
        <div className="hair hidden grid-cols-[minmax(0,2.2fr)_minmax(0,1.6fr)_7rem_minmax(0,1.4fr)_1.25rem] gap-3 border-b bg-raised px-4 py-2 text-[11px] font-medium uppercase tracking-wide text-muted md:grid">
          <span>Device</span><span>Readings</span><span>Status</span><span>Control</span><span />
        </div>
        <ul className="divide-y hair">
          {shown.map((d) => <DeviceRow key={d.key} d={d} onOpen={() => onOpen(d.key)} />)}
        </ul>
      </div>
    </div>
  );
}

function DeviceRow({ d, onOpen }: { d: Device; onOpen: () => void }) {
  const Icon = KIND_ICON[d.kind];
  const lit = d.controls.some((c) => c.on);
  return (
    <li
      className="hair grid cursor-pointer grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-4 py-3 transition hover:bg-raised md:grid-cols-[minmax(0,2.2fr)_minmax(0,1.6fr)_7rem_minmax(0,1.4fr)_1.25rem]"
      onClick={onOpen}
    >
      <div className="flex min-w-0 items-center gap-3">
        <div className={`grid h-9 w-9 shrink-0 place-items-center rounded-lg ${lit ? 'bg-warm/20 text-warm' : 'bg-raised text-muted'}`}><Icon size={17} /></div>
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">{d.name}</div>
          <div className="truncate text-xs text-muted">{[d.vendor, d.model].filter(Boolean).join(' · ') || KIND_NAME[d.kind]}</div>
        </div>
      </div>
      <div className="hidden min-w-0 flex-wrap gap-1 md:flex">
        {d.readings.filter((r) => r.icon !== 'battery').slice(0, 3).map((r) => <ReadingChip key={r.key} r={r} />)}
        {!d.readings.length && <span className="text-xs text-muted">—</span>}
      </div>
      <div className="hidden flex-col gap-1 text-xs md:flex">
        <span className="inline-flex items-center gap-1.5">
          <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${d.healthy ? 'bg-live' : d.interviewing ? 'bg-warm' : 'bg-alarm'}`} />
          <span className="truncate">{d.healthy ? 'Ready' : d.status}</span>
        </span>
        <span className="flex items-center gap-2 text-muted">
          {d.proto === 'zwave'
            ? (d.security ? <span className="inline-flex items-center gap-0.5 text-live" title={d.security.join(', ')}><ShieldCheck size={12} /> S2</span>
              : <span className="inline-flex items-center gap-0.5" title="Not secure"><ShieldAlert size={12} /> Insecure</span>)
            : d.signal && <span className="inline-flex items-center gap-0.5" title={d.signal}><Signal size={12} /> {d.signal.replace('LQI ', '')}</span>}
          {d.battery !== undefined && (
            <span className={`inline-flex items-center gap-0.5 ${d.battery < 15 ? 'text-alarm' : ''}`}>
              {d.battery < 15 ? <BatteryLow size={12} /> : <Battery size={12} />}{d.battery}%
            </span>
          )}
        </span>
      </div>
      <div className="flex min-w-0 flex-wrap items-center justify-end gap-2 md:justify-start" onClick={(e) => e.stopPropagation()}>
        {d.controls.length === 1 && <OnOff d={d} c={d.controls[0]} />}
        {d.controls.length > 1 && d.controls.map((c) => (
          <span key={c.ep} className="inline-flex items-center gap-1.5 rounded-lg bg-raised py-1 pl-2 pr-1 text-xs">
            <span className="max-w-24 truncate">{c.label}</span>
            <OnOff d={d} c={c} />
          </span>
        ))}
      </div>
      <ChevronRight size={16} className="hidden text-muted md:block" />
    </li>
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

function ActivityList({ proto }: { proto: Proto }) {
  const t = useTree(proto);
  const lines = t.log
    .map((l) => ({ at: l.at, s: describe(proto, l.tail, l.data) }))
    .filter((l): l is { at: number; s: string } => !!l.s);
  return (
    <section className="hair overflow-hidden rounded-xl border bg-panel">
      {lines.length ? (
        <ul className="divide-y hair">
          {lines.map((l, i) => (
            <li key={i} className="hair flex gap-4 px-4 py-2.5 text-sm">
              <span className="w-20 shrink-0 tabular-nums text-xs leading-5 text-muted">
                {new Date(l.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}
              </span>
              <span>{l.s.replace(/^(Zigbee|Z-Wave): /, '')}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="px-4 py-10 text-center text-xs text-muted">
          Nothing yet. Pairing, devices joining and leaving, and network changes show up here as they happen.
        </p>
      )}
    </section>
  );
}
