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
import { ask } from '../../dialog';

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
                      onClick={async () => (await ask(`Remove ${d.name} (node ${d.id})?`, { body: 'It no longer answers.', ok: 'Remove', danger: true })) && remove(d)}
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
              onClick={async () => {
                if (!pick || !(await ask(`Move the Zigbee network to channel ${pick}?`, { body: 'Every device follows; sleepy ones when they next wake.', ok: 'Move' }))) return;
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

type SheetTab = 'overview' | 'settings' | 'details';

/* A device opens as a centered dialog with its own tabs, the way the Z-Wave and
   Zigbee controller UIs people already know lay a device out: what it does
   (overview), how it is configured (settings, grouped by what each setting
   acts on), and what it is (details). Actions sit in the header. */
function DeviceSheet({ d, onClose, onExclude }: { d: Device; onClose: () => void; onExclude: () => void }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(d.name);
  const [tab, setTab] = useState<SheetTab>('overview');
  const t = useTree('zwave');
  const params = d.proto === 'zwave' ? zwaveParams(t, d.id) : [];
  const Icon = KIND_ICON[d.kind];
  useEffect(() => setName(d.name), [d.name]);
  const save = () => {
    const n = name.trim();
    if (n && n !== d.name) rename(d, n);
    setEditing(false);
  };
  const dead = d.proto === 'zwave' && d.status === 'Not responding';
  useEffect(() => {
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape' && !editing) onClose(); };
    window.addEventListener('keydown', esc);
    return () => window.removeEventListener('keydown', esc);
  }, [onClose, editing]);
  const removeIt = async () => {
    if (d.proto === 'zigbee') {
      if (!(await ask(`Remove ${d.name} from the network?`, { ok: 'Remove', danger: true }))) return;
      remove(d, !d.healthy);
      onClose();
    } else if (dead) {
      if (!(await ask(`Remove ${d.name} from the controller?`, { body: 'It does not answer.', ok: 'Remove', danger: true }))) return;
      remove(d);
      onClose();
    } else onExclude();
  };
  const tabs: [SheetTab, string, number?][] = [
    ['overview', 'Overview'],
    ...(d.proto === 'zwave' ? [['settings', 'Settings', params.length] as [SheetTab, string, number]] : []),
    ['details', 'Details'],
  ];
  const chip = 'inline-flex items-center gap-1 rounded-md bg-raised px-2 py-0.5 text-xs';
  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/50 p-4" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label={d.name}
        className="hair flex h-[min(44rem,90vh)] w-full max-w-3xl flex-col overflow-hidden rounded-2xl border bg-panel shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="hair border-b px-5 pt-4">
          <div className="flex items-start gap-3">
            <div className="grid h-11 w-11 shrink-0 place-items-center rounded-xl bg-accent/15 text-accent"><Icon size={22} /></div>
            <div className="min-w-0 flex-1">
              {editing ? (
                <div className="flex gap-1">
                  <input
                    autoFocus
                    value={name}
                    onChange={(e) => setName(e.target.value)}
                    onKeyDown={(e) => { if (e.key === 'Enter') save(); if (e.key === 'Escape') setEditing(false); }}
                    className="hair w-full rounded-md border bg-raised px-2 py-1 text-base outline-none focus:border-accent/50"
                  />
                  <button className={ghost} onClick={save} aria-label="Save name"><Check size={14} /></button>
                </div>
              ) : (
                <button className="group flex items-center gap-1.5 text-left text-base font-semibold" onClick={() => setEditing(true)}>
                  {d.name} <Pencil size={13} className="text-muted opacity-0 group-hover:opacity-100" />
                </button>
              )}
              <div className="truncate text-xs text-muted">{[d.vendor, d.model, label(d.proto)].filter(Boolean).join(' · ')}</div>
              <div className="mt-2 flex flex-wrap gap-1.5">
                <span className={chip}>
                  <span className={`h-1.5 w-1.5 rounded-full ${d.healthy ? 'bg-live' : d.interviewing ? 'bg-warm' : 'bg-alarm'}`} />
                  {d.status}
                </span>
                <span className={`${chip} ${d.security ? 'bg-live/15 text-live' : ''}`}>
                  {d.security ? <ShieldCheck size={12} /> : <ShieldAlert size={12} />}
                  {d.security ? d.security[0] : d.proto === 'zwave' ? 'Not secure' : 'Network key'}
                </span>
                {d.signal && <span className={chip}><Signal size={12} /> {d.signal}</span>}
                {d.battery !== undefined && <span className={chip}><Battery size={12} /> {d.battery}%</span>}
                {d.firmware && <span className={chip}><Cpu size={12} /> {d.firmware}</span>}
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <button className="rounded-lg p-2 text-muted hover:bg-raised hover:text-ink" title="Set up again" aria-label="Set up again" onClick={() => reinterview(d)}>
                <RefreshCw size={16} />
              </button>
              <button className="rounded-lg p-2 text-muted hover:bg-alarm/10 hover:text-alarm" title="Remove" aria-label="Remove" onClick={removeIt}>
                <Trash2 size={16} />
              </button>
              <button className="rounded-lg p-2 text-muted hover:bg-raised hover:text-ink" onClick={onClose} aria-label="Close"><X size={18} /></button>
            </div>
          </div>
          <nav className="mt-3 flex gap-1" role="tablist">
            {tabs.map(([k, l, n]) => (
              <button
                key={k}
                role="tab"
                aria-selected={tab === k}
                onClick={() => setTab(k)}
                className={`-mb-px border-b-2 px-3 py-2 text-sm transition ${tab === k ? 'border-accent text-ink' : 'border-transparent text-muted hover:text-ink'}`}
              >
                {l}{n ? <span className="ml-1.5 rounded bg-raised px-1.5 text-[11px] text-muted">{n}</span> : null}
              </button>
            ))}
          </nav>
        </header>

        <div className="min-h-0 flex-1 overflow-y-auto">
          {tab === 'overview' && <Overview d={d} />}
          {tab === 'settings' && <ZwaveSettings id={d.id} />}
          {tab === 'details' && <Details d={d} />}
        </div>
      </div>
    </div>
  );
}

function Overview({ d }: { d: Device }) {
  const heading = 'mb-2 text-xs font-medium uppercase tracking-wide text-muted';
  const nothing = !d.controls.length && !d.colorTemp && !d.readings.length;
  return (
    <div className="space-y-6 p-5">
      {d.controls.length > 0 && (
        <section>
          <h3 className={heading}>{d.controls.length > 1 ? 'Channels' : 'Control'}</h3>
          <div className={`grid gap-2 ${d.controls.length > 1 ? 'sm:grid-cols-2' : ''}`}>
            {d.controls.map((c) => (
              <div key={c.ep} className="rounded-xl bg-raised p-3.5">
                <div className="flex items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium">{d.controls.length > 1 ? c.label : d.name}</div>
                    <div className="text-xs text-muted">{c.on === undefined ? 'Not reported yet' : c.on ? 'On' : 'Off'}</div>
                  </div>
                  <OnOff d={d} c={c} />
                </div>
                {c.level !== undefined && <div className="mt-3"><LevelSlider d={d} c={c} /></div>}
              </div>
            ))}
          </div>
        </section>
      )}
      {d.colorTemp && (
        <section>
          <h3 className={heading}>Colour temperature</h3>
          <div className="flex items-center gap-2 rounded-xl bg-raised p-3.5">
            <span className="text-xs text-muted">Warm</span>
            <Committed value={d.colorTemp.mireds} min={d.colorTemp.min} max={d.colorTemp.max} onCommit={(v) => setColorTemp(d, v)} label="Colour temperature" />
            <span className="text-xs text-muted">Cool</span>
          </div>
        </section>
      )}
      {d.readings.length > 0 && (
        <section>
          <h3 className={heading}>Readings</h3>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
            {d.readings.map((r) => {
              const I = READING_ICON[r.icon] ?? Gauge;
              return (
                <div key={r.key} className={`rounded-xl p-3 ${r.alert ? 'bg-alarm/10' : 'bg-raised'}`}>
                  <div className="flex items-center gap-1 text-xs text-muted"><I size={12} /> {r.label}</div>
                  <div className={`mt-1 text-lg font-medium tabular-nums ${r.alert ? 'text-alarm' : ''}`}>{r.text}</div>
                </div>
              );
            })}
          </div>
        </section>
      )}
      {nothing && <p className="py-10 text-center text-sm text-muted">Nothing to control or read on this device yet.</p>}
    </div>
  );
}

function Details({ d }: { d: Device }) {
  const [raw, setRaw] = useState(false);
  return (
    <div className="space-y-4 p-5">
      <dl className="hair divide-y overflow-hidden rounded-xl border text-sm">
        {d.details.map(([k, v]) => (
          <div key={k} className="hair flex gap-4 px-3.5 py-2">
            <dt className="w-36 shrink-0 text-muted">{k}</dt>
            <dd className="min-w-0 break-words">{v}</dd>
          </div>
        ))}
      </dl>
      <button className="inline-flex items-center gap-1 text-xs text-muted hover:text-ink" onClick={() => setRaw(!raw)}>
        <Cpu size={12} /> {raw ? 'Hide' : 'Show'} technical record <ChevronDown size={12} className={raw ? 'rotate-180' : ''} />
      </button>
      {raw && <pre className="hair max-h-80 overflow-auto rounded-lg border bg-raised p-3 text-[11px]">{JSON.stringify(d.raw, null, 2)}</pre>}
    </div>
  );
}

/* ---------------- Z-Wave settings (configuration parameters) ---------------- */

/** "Relay 1: Auto Turn-Off Timer" files under "Relay 1"; anything else under General. */
const groupOf = (label: string) => (label.includes(':') ? label.slice(0, label.indexOf(':')).trim() : 'General');
const shortLabel = (label: string) => (label.includes(':') ? label.slice(label.indexOf(':') + 1).trim() : label);

function ZwaveSettings({ id }: { id: string }) {
  const t = useTree('zwave');
  const params = zwaveParams(t, id);
  const [edits, setEdits] = useState<Record<string, number>>({});
  const [help, setHelp] = useState<string | null>(null);
  if (!params.length) {
    return <p className="p-10 text-center text-sm text-muted">No settings known for this device: it is not in the device library, or it has not been identified yet.</p>;
  }
  const groups = new Map<string, typeof params>();
  for (const p of params) {
    const g = groupOf(p.label);
    groups.set(g, [...(groups.get(g) ?? []), p]);
  }
  const n = Object.keys(edits).length;
  return (
    <div className="flex min-h-full flex-col">
      <div className="flex-1 space-y-5 p-5">
        {[...groups].map(([g, ps]) => (
          <section key={g}>
            <h3 className="mb-2 text-xs font-medium uppercase tracking-wide text-muted">{g}</h3>
            <div className="hair divide-y overflow-hidden rounded-xl border">
              {ps.map((p) => {
                const shown = edits[p.key] ?? p.value ?? p.default;
                const changed = edits[p.key] !== undefined;
                const set = (v: number) => setEdits((e) => {
                  const x = { ...e };
                  if (v === (p.value ?? p.default)) delete x[p.key]; else x[p.key] = v;
                  return x;
                });
                return (
                  <div key={p.key} className={`hair px-3.5 py-2.5 ${changed ? 'bg-accent/5' : ''}`}>
                    <div className="flex items-center gap-3">
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-1.5 text-sm">
                          {changed && <span className="h-1.5 w-1.5 rounded-full bg-accent" />}
                          {groups.size > 1 ? shortLabel(p.label) : p.label}
                          {p.description && (
                            <button
                              className="text-muted hover:text-ink"
                              aria-label="About this setting"
                              onClick={() => setHelp(help === p.key ? null : p.key)}
                            >
                              <CircleHelp size={13} />
                            </button>
                          )}
                        </div>
                        <div className="text-[11px] text-muted">#{p.key}{p.value === undefined ? ' · not read yet' : ''}</div>
                      </div>
                      <div className="shrink-0">
                        {p.readOnly ? (
                          <span className="text-sm">{p.options.find((o) => o.value === shown)?.label ?? shown}</span>
                        ) : p.options.length ? (
                          <select
                            value={shown}
                            onChange={(e) => set(Number(e.target.value))}
                            className="hair w-56 rounded-lg border bg-raised px-2 py-1.5 text-sm outline-none focus:border-accent/50"
                          >
                            {p.options.map((o) => <option key={o.value} value={o.value}>{o.label}</option>)}
                          </select>
                        ) : (
                          <span className="flex items-center gap-1.5">
                            <input
                              type="number"
                              min={p.min}
                              max={p.max}
                              value={shown}
                              onChange={(e) => set(Math.max(p.min, Math.min(p.max, Number(e.target.value))))}
                              className="hair w-28 rounded-lg border bg-raised px-2 py-1.5 text-right text-sm tabular-nums outline-none focus:border-accent/50"
                            />
                            {p.unit && <span className="whitespace-nowrap text-xs text-muted">{p.unit}</span>}
                          </span>
                        )}
                      </div>
                    </div>
                    {help === p.key && p.description && (
                      <p className="mt-2 rounded-lg bg-raised p-2.5 text-xs text-muted">
                        {p.description} <span className="whitespace-nowrap">Range {p.min}–{p.max}, default {p.options.find((o) => o.value === p.default)?.label ?? p.default}.</span>
                      </p>
                    )}
                  </div>
                );
              })}
            </div>
          </section>
        ))}
      </div>
      {n > 0 && (
        <div className="hair sticky bottom-0 flex items-center gap-3 border-t bg-panel px-5 py-3">
          <span className="text-sm">{n} unsaved {n === 1 ? 'change' : 'changes'}</span>
          <button className={`${ghost} ml-auto`} onClick={() => setEdits({})}>Discard</button>
          <button className={primary} onClick={() => { configure(id, edits); setEdits({}); }}>
            <Save size={14} /> Save to device
          </button>
        </div>
      )}
    </div>
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
