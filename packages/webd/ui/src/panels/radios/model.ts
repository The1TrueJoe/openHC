/* The radios' state, as one device model for the UI.

   Two bridges run the networks: zigbee-bridge under `<base>/zigbee` and
   zwave-bridge under `<base>/zwave` (both pulzar-net projects, each with its
   own documented MQTT format). This file follows both trees and turns them
   into Device objects, so the page deals in lights, plugs and sensors, not in
   clusters, command classes and topic paths. */
import { useSyncExternalStore } from 'react';
import { io } from '../../api';
import { toastError } from '../../toast';

export type Proto = 'zigbee' | 'zwave';

type Tree = {
  topics: Map<string, string>;
  /** Events and request responses, newest first, with when they arrived. */
  log: { at: number; tail: string; data: any }[];
};

const parse = (s: string | undefined): any => {
  if (s === undefined || s === '') return undefined;
  try { return JSON.parse(s); } catch { return s; }
};

function makeTree(proto: Proto) {
  let snap: Tree = { topics: new Map(), log: [] };
  const subs = new Set<() => void>();
  let started = false;
  return {
    start() {
      if (started) return;
      started = true;
      io.subscribeTree(proto, (tail, body) => {
        const topics = new Map(snap.topics);
        if (body === '') topics.delete(tail);
        else topics.set(tail, body);
        let log = snap.log;
        if (tail === 'bridge/event' || tail.startsWith('bridge/response/')) {
          const data = parse(body);
          log = [{ at: Date.now(), tail, data }, ...log].slice(0, 200);
          if (tail.startsWith('bridge/response/') && (data?.status === 'error' || data?.ok === false) && !quiet(tail, data)) {
            toastError(data.error ?? body, `${proto === 'zigbee' ? 'Zigbee' : 'Z-Wave'}: ${tail.slice(16).replace(/_/g, ' ')}`);
          }
        }
        snap = { topics, log };
        subs.forEach((f) => f());
      });
    },
    subscribe: (f: () => void) => (subs.add(f), () => { subs.delete(f); }),
    get: () => snap,
    /** One of the bridge's requests, or a device command. */
    send(tail: string, body: unknown = {}) {
      try {
        io.publishTree(proto, tail, typeof body === 'string' ? body : JSON.stringify(body));
      } catch (e) {
        toastError(e, proto === 'zigbee' ? 'Zigbee' : 'Z-Wave');
      }
    },
  };
}

/** Outcomes the wizard explains itself, so they need no toast. */
const quiet = (tail: string, data: any) => tail === 'bridge/response/exclude' && /no node/i.test(data?.error ?? '');

export const trees = { zigbee: makeTree('zigbee'), zwave: makeTree('zwave') };

/** Follow both bridges; called once the MQTT client is set up. */
export function startRadios() {
  trees.zigbee.start();
  trees.zwave.start();
}

export function useTree(p: Proto): Tree {
  return useSyncExternalStore(trees[p].subscribe, trees[p].get);
}

/* ---------------- the device model ---------------- */

export type Kind = 'light' | 'switch' | 'plug' | 'sensor' | 'contact' | 'motion' | 'leak' | 'lock' | 'thermostat' | 'remote' | 'unknown';

export type Reading = {
  key: string;
  label: string;
  text: string;
  /** An icon name the page knows: temperature, humidity, battery, power, ... */
  icon: string;
  /** Worth drawing attention to: open, motion, leak, low battery. */
  alert?: boolean;
};

export type Device = {
  key: string;
  proto: Proto;
  /** IEEE address (Zigbee) or node id (Z-Wave). */
  id: string;
  name: string;
  vendor?: string;
  model?: string;
  firmware?: string;
  kind: Kind;
  /** Short, human: "Ready", "Interviewing: endpoints", "Not responding". */
  status: string;
  healthy: boolean;
  interviewing: boolean;
  /** It has an on/off control, whether or not it has reported its state yet. */
  switchable: boolean;
  on?: boolean;
  /** 0..100. */
  level?: number;
  colorTemp?: { mireds: number; min: number; max: number };
  readings: Reading[];
  battery?: number;
  /** Link quality (Zigbee LQI) or route RSSI, already as text. */
  signal?: string;
  /** Granted Z-Wave security classes; Zigbee: whether it has a unique link key. */
  security?: string[];
  details: [string, string][];
  raw: any;
};

const hex4 = (v: unknown) => `0x${Number(v ?? 0).toString(16).padStart(4, '0')}`;
const num = (v: unknown) => (typeof v === 'number' ? v : undefined);
const fmt = (v: number, d = 1) => (Number.isInteger(v) ? String(v) : v.toFixed(d));

/* Zigbee */

function zigbeeKind(d: any, st: any): Kind {
  const cl = new Set<string>((d.endpoints ?? []).flatMap((e: any) => e.input_clusters ?? []));
  if (cl.has('door_lock')) return 'lock';
  if (cl.has('thermostat')) return 'thermostat';
  if (cl.has('color_control') || (cl.has('level_control') && cl.has('on_off'))) return 'light';
  if (cl.has('on_off') && (cl.has('electrical_measurement') || cl.has('metering'))) return 'plug';
  if (cl.has('on_off')) return 'switch';
  if ('water_leak' in st) return 'leak';
  if ('contact' in st) return 'contact';
  if ('occupancy' in st || cl.has('occupancy_sensing')) return 'motion';
  if (cl.has('ias_zone') || cl.has('temperature_measurement') || cl.has('relative_humidity_measurement') || cl.has('illuminance_measurement')) return 'sensor';
  if ((d.endpoints ?? []).some((e: any) => (e.output_clusters ?? []).includes('on_off'))) return 'remote';
  return 'unknown';
}

const ZB_READINGS: [string, string, string, (v: any) => string | null, ((v: any) => boolean)?][] = [
  ['temperature', 'Temperature', 'temperature', (v) => `${fmt(v)} °C`],
  ['local_temperature', 'Temperature', 'temperature', (v) => `${fmt(v)} °C`],
  ['humidity', 'Humidity', 'humidity', (v) => `${fmt(v, 0)} %`],
  ['pressure', 'Pressure', 'gauge', (v) => `${fmt(v, 0)} hPa`],
  ['illuminance', 'Light', 'light', (v) => `${fmt(v, 0)} lx`],
  ['co2', 'CO₂', 'gauge', (v) => `${fmt(v, 0)} ppm`],
  ['pm25', 'PM2.5', 'gauge', (v) => `${fmt(v, 0)} µg/m³`],
  ['contact', 'Contact', 'contact', (v) => (v ? 'Closed' : 'Open'), (v) => v === false],
  ['occupancy', 'Motion', 'motion', (v) => (v ? 'Motion' : 'Clear'), (v) => !!v],
  ['water_leak', 'Leak', 'leak', (v) => (v ? 'Leak!' : 'Dry'), (v) => !!v],
  ['smoke', 'Smoke', 'alarm', (v) => (v ? 'Smoke!' : 'Clear'), (v) => !!v],
  ['carbon_monoxide', 'CO', 'alarm', (v) => (v ? 'CO!' : 'Clear'), (v) => !!v],
  ['vibration', 'Vibration', 'motion', (v) => (v ? 'Vibration' : 'Still'), (v) => !!v],
  ['alarm', 'Alarm', 'alarm', (v) => (v ? 'Alarm' : 'Clear'), (v) => !!v],
  ['tamper', 'Tamper', 'alarm', (v) => (v ? 'Tampered' : null), (v) => !!v],
  ['power', 'Power', 'power', (v) => `${fmt(v)} W`],
  ['energy', 'Energy', 'power', (v) => `${fmt(v, 2)} kWh`],
  ['voltage', 'Voltage', 'power', (v) => `${fmt(v)} V`],
  ['current', 'Current', 'power', (v) => `${fmt(v, 2)} A`],
];

function zigbeeDevice(d: any, st: any): Device {
  st = st && typeof st === 'object' ? st : {};
  const readings: Reading[] = [];
  for (const [k, label, icon, text, alert] of ZB_READINGS) {
    if (!(k in st) || st[k] === null) continue;
    const t = text(st[k]);
    if (t) readings.push({ key: k, label, text: t, icon, alert: alert?.(st[k]) });
  }
  const battery = num(st.battery);
  if (battery !== undefined) readings.push({ key: 'battery', label: 'Battery', text: `${battery} %`, icon: 'battery', alert: battery < 15 || !!st.battery_low });
  const done = !!d.interview?.done;
  const stage = d.interview?.stage;
  const kind = zigbeeKind(d, st);
  const on = typeof st.state === 'string' ? st.state.toUpperCase() === 'ON' : undefined;
  const bri = num(st.brightness);
  return {
    key: `zigbee:${d.ieee_address}`,
    proto: 'zigbee',
    id: d.ieee_address,
    name: d.friendly_name ?? d.ieee_address,
    vendor: d.manufacturer ?? undefined,
    model: d.model ?? undefined,
    firmware: d.software_build_id ?? undefined,
    kind,
    status: done ? 'Ready' : d.interview?.error ? `Interview stopped: ${d.interview.error}` : `Setting up${stage ? `: ${stage.replace(/_/g, ' ')}` : '…'}`,
    healthy: done && !d.interview?.error,
    interviewing: !done,
    switchable: on !== undefined || kind === 'light' || kind === 'switch' || kind === 'plug',
    on,
    level: bri !== undefined ? Math.round((bri / 254) * 100) : undefined,
    colorTemp: num(st.color_temp) !== undefined ? { mireds: st.color_temp, min: 153, max: 500 } : undefined,
    readings,
    battery,
    signal: num(st.linkquality) !== undefined ? `LQI ${st.linkquality}` : undefined,
    security: d.unique_link_key ? ['unique link key'] : undefined,
    details: [
      ['IEEE address', d.ieee_address],
      ['Network address', d.network_address !== undefined ? `0x${Number(d.network_address).toString(16).padStart(4, '0')}` : '—'],
      ['Type', d.type?.replace(/_/g, ' ') ?? '—'],
      ['Power', ({ 1: 'Mains', 3: 'Battery', 4: 'DC' } as Record<number, string>)[d.power_source] ?? '—'],
      ['Manufacturer', d.manufacturer ?? '—'],
      ['Model', d.model ?? '—'],
      ['Firmware', d.software_build_id ?? '—'],
      ['Date code', d.date_code ?? '—'],
      ['Security', d.unique_link_key ? 'Zigbee 3.0 unique link key' : 'Network key only'],
      ['Clusters', (d.endpoints ?? []).map((e: any) => `${e.endpoint}: ${(e.input_clusters ?? []).join(', ')}`).join(' · ') || '—'],
    ],
    raw: d,
  };
}

export function zigbeeDevices(t: Tree): Device[] {
  const list: any[] = parse(t.topics.get('bridge/devices')) ?? [];
  return list.filter((d) => d.type !== 'coordinator').map((d) => zigbeeDevice(d, parse(t.topics.get(d.friendly_name))));
}

/* Z-Wave */

/** Generic device classes (SDS13740), for the kind of node before its values say more. */
const ZW_GENERIC: Record<number, Kind> = {
  0x08: 'thermostat', 0x10: 'switch', 0x11: 'light', 0x12: 'switch', 0x18: 'remote', 0x20: 'sensor',
  0x21: 'sensor', 0x31: 'plug', 0x40: 'lock', 0x07: 'sensor', 0x01: 'remote',
};

const ZW_CONTACT = /door|window|access_control/i;
const ZW_MOTION = /motion|occupancy|home_security/i;
const ZW_LEAK = /water/i;

function zwaveDevice(id: string, t: Tree): Device {
  const n = parse(t.topics.get(`nodes/${id}`)) ?? {};
  const status = t.topics.get(`nodes/${id}/status`) ?? 'unknown';
  const pre = `nodes/${id}/`;
  const values = new Map<string, any>();
  for (const [k, v] of t.topics) {
    if (!k.startsWith(pre)) continue;
    const path = k.slice(pre.length);
    if (['status', 'route', 'parameters', 'command'].includes(path)) continue;
    const pv = parse(v);
    if (pv && typeof pv === 'object' && 'value' in pv) values.set(path, pv);
  }
  const readings: Reading[] = [];
  let kind: Kind = ZW_GENERIC[n.protocol?.generic] ?? 'unknown';
  let on: boolean | undefined;
  let level: number | undefined;
  let battery: number | undefined;
  for (const [path, v] of values) {
    const [cc, prop] = path.split('/');
    const label = v.label ?? prop;
    if (cc === 'binary_switch' && prop === 'current') { on = !!v.value; continue; }
    if (cc === 'multilevel_switch' && prop === 'current') {
      level = Math.max(0, Math.min(100, Math.round((Number(v.value) / 99) * 100)));
      on = Number(v.value) > 0;
      if (kind === 'switch' || kind === 'unknown') kind = 'light';
      continue;
    }
    if (cc === 'battery' && prop === 'level') {
      battery = Number(v.value);
      readings.push({ key: path, label: 'Battery', text: `${battery} %`, icon: 'battery', alert: battery < 15 });
      continue;
    }
    if (cc === 'notification') {
      const idle = v.value === 0 || /idle/i.test(v.state ?? '');
      const icon = ZW_LEAK.test(prop) ? 'leak' : ZW_CONTACT.test(prop) ? 'contact' : ZW_MOTION.test(prop) ? 'motion' : 'alarm';
      if (icon === 'contact') kind = 'contact'; else if (icon === 'leak') kind = 'leak'; else if (icon === 'motion' && kind !== 'contact') kind = 'motion';
      readings.push({ key: path, label, text: idle ? 'Clear' : v.state ?? String(v.value), icon, alert: !idle });
      continue;
    }
    if (cc === 'meter' || /power|energy|voltage|current/i.test(prop)) {
      if (kind === 'switch') kind = 'plug';
      readings.push({ key: path, label, text: `${typeof v.value === 'number' ? fmt(v.value, 2) : v.value}${v.unit ? ` ${v.unit}` : ''}`, icon: 'power' });
      continue;
    }
    const icon = /temperature/i.test(prop) ? 'temperature' : /humidity/i.test(prop) ? 'humidity' : /illuminance|luminance/i.test(prop) ? 'light' : 'gauge';
    readings.push({ key: path, label, text: `${typeof v.value === 'number' ? fmt(v.value) : v.state ?? String(v.value)}${v.unit ? ` ${v.unit}` : ''}`, icon });
  }
  const stage = n.stage as string | undefined;
  const interviewing = !!stage && stage !== 'complete';
  const dead = status === 'dead';
  const name = n.name || n.device?.label || `Node ${id}`;
  const sec: string[] = n.security ?? [];
  const route = parse(t.topics.get(`nodes/${id}/route`));
  return {
    key: `zwave:${id}`,
    proto: 'zwave',
    id,
    name,
    vendor: n.device?.manufacturer ?? (n.manufacturer ? `Manufacturer ${hex4(n.manufacturer.manufacturer_id)}` : undefined),
    model: n.device?.label ?? n.device?.description,
    firmware: typeof n.firmware === 'string' ? n.firmware : undefined,
    kind,
    status: dead ? 'Not responding' : interviewing ? `Setting up: ${stage!.replace(/_/g, ' ')}` : status === 'asleep' ? 'Asleep' : status === 'awake' ? 'Awake' : 'Ready',
    healthy: !dead && !interviewing,
    interviewing,
    switchable: on !== undefined || kind === 'light' || kind === 'switch' || kind === 'plug',
    on,
    level,
    readings,
    battery,
    signal: num(route?.rssi) !== undefined ? `${route.rssi} dBm` : undefined,
    security: sec.length ? sec.map(secName) : undefined,
    details: [
      ['Node', id],
      ['Status', status],
      ['Listening', n.protocol ? (n.protocol.listening ? 'Always on' : n.protocol.flirs ? 'Wakes on beam' : 'Sleeps') : '—'],
      ['Manufacturer', n.device?.manufacturer ?? (n.manufacturer ? hex4(n.manufacturer.manufacturer_id) : '—')],
      ['Product', n.device?.label ?? (n.manufacturer ? `${hex4(n.manufacturer.product_type)}:${hex4(n.manufacturer.product_id)}` : '—')],
      ['Firmware', typeof n.firmware === 'string' ? n.firmware : '—'],
      ['Security', sec.length ? sec.map(secName).join(', ') : 'None'],
      ['Location', n.location || '—'],
      ['Interview', stage ?? '—'],
      ['Command classes', Object.keys(n.endpoints?.['0']?.ccs ?? {}).map((c) => CC_NAMES[Number(c)] ?? `0x${Number(c).toString(16)}`).join(', ') || '—'],
    ],
    raw: n,
  };
}

const secName = (s: string) => s.replace(/_/g, ' ').replace(/^s2 /, 'S2 ').replace(/^s0/, 'S0');

/** The command classes devices commonly have, for the detail sheet. */
const CC_NAMES: Record<number, string> = {
  0x20: 'Basic', 0x25: 'Binary Switch', 0x26: 'Multilevel Switch', 0x27: 'All Switch', 0x2b: 'Scene Activation',
  0x30: 'Binary Sensor', 0x31: 'Multilevel Sensor', 0x32: 'Meter', 0x33: 'Color Switch', 0x40: 'Thermostat Mode',
  0x43: 'Thermostat Setpoint', 0x55: 'Transport Service', 0x56: 'CRC-16', 0x59: 'Association Group Info',
  0x5a: 'Device Reset Locally', 0x5b: 'Central Scene', 0x5e: 'Z-Wave Plus Info', 0x60: 'Multi Channel',
  0x62: 'Door Lock', 0x6c: 'Supervision', 0x70: 'Configuration', 0x71: 'Notification', 0x72: 'Manufacturer Specific',
  0x73: 'Powerlevel', 0x75: 'Protection', 0x7a: 'Firmware Update', 0x80: 'Battery', 0x84: 'Wake Up', 0x85: 'Association',
  0x86: 'Version', 0x87: 'Indicator', 0x8e: 'Multi Channel Association', 0x98: 'Security S0', 0x9f: 'Security S2',
};

export function zwaveDevices(t: Tree): Device[] {
  const info = parse(t.topics.get('bridge/info')) ?? {};
  const ids = new Set<string>();
  for (const k of t.topics.keys()) {
    const m = /^nodes\/(\d+)$/.exec(k);
    if (m && m[1] !== String(info.node_id)) ids.add(m[1]);
  }
  return [...ids].sort((a, b) => Number(a) - Number(b)).map((id) => zwaveDevice(id, t));
}

export const info = (t: Tree) => parse(t.topics.get('bridge/info'));
export const present = (t: Tree) => t.topics.has('bridge/info');
export { parse };

/* ---------------- commands ---------------- */

export function setOn(d: Device, on: boolean) {
  if (d.proto === 'zigbee') return trees.zigbee.send(`${d.name}/set`, { state: on ? 'ON' : 'OFF' });
  const path = d.level !== undefined ? 'multilevel_switch/current' : 'binary_switch/current';
  trees.zwave.send(`nodes/${d.id}/${path}/set`, on ? 'ON' : 'OFF');
}

export function setLevel(d: Device, pct: number) {
  if (d.proto === 'zigbee') return trees.zigbee.send(`${d.name}/set`, { brightness: Math.round((pct / 100) * 254) });
  trees.zwave.send(`nodes/${d.id}/multilevel_switch/current/set`, String(Math.round((pct / 100) * 99)));
}

export function setColorTemp(d: Device, mireds: number) {
  trees.zigbee.send(`${d.name}/set`, { color_temp: Math.round(mireds) });
}

export function rename(d: Device, name: string) {
  if (d.proto === 'zigbee') return trees.zigbee.send('bridge/request/rename', { from: d.name, to: name });
  trees.zwave.send('bridge/request/rename', { node: Number(d.id), name, location: d.raw.location ?? '' });
}

export function reinterview(d: Device) {
  if (d.proto === 'zigbee') return trees.zigbee.send('bridge/request/interview', { device: d.id });
  trees.zwave.send('bridge/request/reinterview', { node: Number(d.id) });
}

/** Zigbee: ask it to leave (force: forget it even if it does not answer).
 *  Z-Wave: only a node that no longer answers can be removed this way; a
 *  working one leaves by exclusion (the wizard). */
export function remove(d: Device, force = false) {
  if (d.proto === 'zigbee') return trees.zigbee.send('bridge/request/remove', { device: d.id, force });
  trees.zwave.send('bridge/request/remove_failed', { node: Number(d.id) });
}

/* ---------------- activity, in words ---------------- */

export function describe(proto: Proto, tail: string, m: any): string | null {
  if (proto === 'zigbee') {
    if (tail !== 'bridge/event' || !m?.type) return null;
    const d = m.data ?? {};
    switch (m.type) {
      case 'permit_join': return d.time ? `Zigbee: pairing open for ${d.time} s` : 'Zigbee: pairing closed';
      case 'device_joined': return `Zigbee: a device joined (${d.ieee_address})${d.rejoin ? ', rejoining' : ''}`;
      case 'device_interview':
        return d.status === 'successful'
          ? `Zigbee: set up ${[d.manufacturer, d.model].filter(Boolean).join(' ') || d.ieee_address}`
          : d.status === 'failed' ? `Zigbee: setting up ${d.ieee_address} stopped: ${d.error ?? d.stage}` : `Zigbee: setting up ${d.ieee_address}`;
      case 'device_leave': return `Zigbee: ${d.ieee_address} left`;
      case 'device_removed': return `Zigbee: ${d.ieee_address} removed`;
      case 'channel_changed': return `Zigbee: moved to channel ${d.channel}`;
      default: return null;
    }
  }
  if (tail === 'bridge/response/include' && m?.ok) {
    return `Z-Wave: node ${m.node} added${m.security?.length ? `, secured (${m.security.join(', ')})` : ', without security'}${m.security_error ? ` — ${m.security_error}` : ''}`;
  }
  if (tail !== 'bridge/event' || !m?.event) return null;
  switch (m.event) {
    case 'inclusion_started': return 'Z-Wave: adding a device';
    case 'exclusion_started': return 'Z-Wave: removing a device';
    case 'node_added': return `Z-Wave: node ${m.node} joined`;
    case 'node_removed': return `Z-Wave: node ${m.node} removed`;
    case 'security_bootstrapped': return `Z-Wave: node ${m.node} secured (${(m.classes ?? []).join(', ')})`;
    case 'security_failed': return `Z-Wave: securing node ${m.node} failed: ${m.reason}`;
    case 'interview_completed': return `Z-Wave: node ${m.node} set up`;
    case 'interview_failed': return `Z-Wave: setting up node ${m.node} failed: ${m.reason}`;
    default: return null;
  }
}
