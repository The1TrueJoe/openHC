// The client: MQTT for IO, REST for everything else.
//
// IO control is MQTT and only MQTT. The topics this page publishes and
// subscribes to are byte-for-byte the ones Home Assistant or a Node-RED flow
// would use, so there is one set of semantics to document and one to get right
// — rather than a bespoke browser protocol kept in step with the MQTT one
// forever.
//
// REST keeps what MQTT is bad at: what the board IS (needed before you know
// which topics exist) and how it is CONFIGURED (editing your own transport over
// that transport is a good way to lose a controller).
//
// Everything is same-origin. webd proxies both `/iod/*` and `/mqtt` through to
// the IO server, so the whole GUI needs exactly one port reachable — the one it
// was served from.
import mqtt, { type MqttClient } from 'mqtt';

const HTTP = `${location.origin}/iod`;
/* Telemetry is a DIFFERENT daemon behind a different mount. Deriving it from
   HTTP gave /iod/sys/api/now, which 404s — webd proxies /sys to sysmond and
   /iod to iod, and they are not nested. */
const SYS = `${location.origin}/sys`;
const WS = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/mqtt`;

/** Set when the daemon runs with IOD_TOKEN. Read from the page URL so a
 *  protected controller can be opened with ?token=… without a login screen the
 *  firmware has nowhere to store an account for. */
const token = new URLSearchParams(location.search).get('token') ?? '';
const auth = (u: string) => (token ? `${u}${u.includes('?') ? '&' : '?'}token=${encodeURIComponent(token)}` : u);

export type Backend = 'mcu' | 'gpio' | 'none';

export interface SerialPort {
  dev: string | null;
  label: string;
  baud: number;
  /** 'host' — a real tty. 'mcu' — routed through the IO protocol, no device node. */
  transport: 'host' | 'mcu';
  /** What THIS port will actually run at. Per port, not one global list: a
   *  host 16550A and a UART reached over the IO microcontroller's protocol do
   *  not have the same ceiling. */
  bauds: number[];
}

/** Exactly what the board has. A section is ABSENT when the count is zero —
 *  that is the contract, and it is why the UI can render without knowing what
 *  model it is talking to. */
export interface Capabilities {
  board: string;
  hostname: string;
  backend: Backend;
  mcu_linked: boolean;
  ir?: { out: number; blaster: number; total: number; combo: number; receiver: number;
         front?: { send: boolean; receive: boolean };
         /** Always `lirc`: each emitter is its own kernel device. */
         via?: 'lirc';
         /** One entry per kernel IR node, named so a port can be matched to the
          *  device an operator would open with ir-ctl. */
         devices?: { name: string; device: string }[] };
  /** Front-panel LEDs, as the kernel registered them — not board.env geometry,
   *  so a board with no panel simply has no key here. */
  leds?: { slug: string; name: string; colour: string; function: string;
           max: number; trigger: string }[];
  relays?: { count: number };
  contacts?: { count: number };
  serials?: SerialPort[];
}

/** What is carrying the IO. The kernel driver owns the link, so iod cannot ask
 *  the part who it is — these come from board.env and from the chip's presence. */
export interface McuInfo {
  chip: string;
  present: boolean;
  part: string | null;
  tty: string | null;
  baud: number | null;
}

export interface MqttConfig {
  serve: boolean;
  listen_port: number;
  bridge: boolean;
  url: string;
  username: string;
  password_set: boolean;
  ca_path: string;
  client_cert_path: string;
  client_key_path: string;
  prefix: string;
  client_id: string;
  discovery: string;
}

/** What the settings form may WRITE. `password_set` is read-only status, and
 *  `password` only ever travels in this direction — the daemon never sends a
 *  stored secret back to a page load. */
export type MqttWrite = Partial<Omit<MqttConfig, 'password_set'>> & { password?: string };

export interface ConfigDoc {
  mqtt: MqttConfig;
  /** Fields pinned by the environment. The UI shows these read-only rather
   *  than accepting an edit that a restart would silently discard. */
  pinned: string[];
  topics: { base: string };
}

/** Panel number for a zero-based array position.
 *
 *  Relays, contacts, IR ports and serial ports are labelled from 1 on the
 *  hardware, and the MQTT topics carry that number. Arrays here are indexed
 *  from 0, so this is the one place the two meet — the same boundary iod draws
 *  in mqtt::topics. */
export const panel = (i: number) => i + 1;

/** The mirrored state, keyed by PANEL NUMBER, exactly as the retained topics
 *  describe it — `state.relay["1"]` is the terminal marked 1. */
export interface IoState {
  relay?: Record<string, boolean>;
  contact?: Record<string, boolean>;
  led?: Record<string, number>;
  mcu?: { link?: boolean };
  serial?: Record<string, { baud?: number; viewers?: number }>;
}

async function j<T>(url: string, init?: RequestInit): Promise<T> {
  const r = await fetch(auth(url), init);
  if (!r.ok) {
    let detail = `${r.status}`;
    try { const b = await r.json(); if (b?.error) detail = b.error; } catch { /* not json */ }
    throw new Error(detail);
  }
  return r.json();
}

/** One telemetry sample from sysmond. `values` is positional against `series`,
 *  which is why the series list comes with it rather than being repeated per
 *  reading. */
export interface Telemetry {
  at: number | null;
  series: { slug: string; label: string; chip: string; kind: 'temp' | 'fan' | 'pwm' }[];
  values: (number | null)[] | null;
  cpu: number | null;
  load1: number | null;
  mem_used_pct: number | null;
  mem_total_kb: number | null;
  uptime_s: number | null;
}

export const rest = {
  /** Telemetry lives behind /sys, proxied by webd to sysmond on :7071. */
  telemetry: () => j<Telemetry>(`${SYS}/api/now`),
  capabilities: () => j<Capabilities>(`${HTTP}/api/io`),
  mcu: () => j<McuInfo>(`${HTTP}/api/io/mcu`),
  config: () => j<ConfigDoc>(`${HTTP}/api/config`),
  saveConfig: (mqtt: MqttWrite) =>
    j<{ ok: boolean }>(`${HTTP}/api/config`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ mqtt }),
    }),
};

/** `ON`/`OFF` become booleans, digits become numbers, everything else stays a
 *  string. iod publishes scalars bare so a shell script can read them without
 *  a JSON parser; this is the other half of that bargain. */
function decode(raw: string): boolean | number | string {
  if (raw === 'ON') return true;
  if (raw === 'OFF') return false;
  if (raw !== '' && !Number.isNaN(Number(raw))) return Number(raw);
  return raw;
}

export class Io {
  state: IoState = {};
  connected = false;
  base = '';
  lastError: string | null = null;

  #c: MqttClient | null = null;
  #onChange = new Set<() => void>();
  #onEvent = new Set<(topic: string, data: any) => void>();

  /** Re-render hook; every retained or live state message calls these. */
  watch(fn: () => void) {
    this.#onChange.add(fn);
    return () => this.#onChange.delete(fn);
  }
  /** Events, which are not state: IR received, serial bytes. `topic` is the
   *  part after `<base>/event/`. */
  onEvent(fn: (topic: string, data: any) => void) {
    this.#onEvent.add(fn);
    return () => this.#onEvent.delete(fn);
  }

  connect(base: string) {
    if (this.#c) return;
    this.base = base;
    const c = mqtt.connect(WS, {
      protocolVersion: 4,
      // The API token doubles as the MQTT password: one secret for the box.
      password: token || undefined,
      username: token ? 'openhc' : undefined,
      reconnectPeriod: 2000,
      clean: true,
    });
    this.#c = c;

    c.on('connect', () => {
      this.connected = true;
      // State is retained, so subscribing IS the snapshot — no separate
      // "give me everything" round trip, and a page loaded an hour after the
      // last change still renders the truth.
      c.subscribe([`${base}/state/#`, `${base}/status`, `${base}/error`]);
      this.#changed();
    });
    c.on('close', () => { this.connected = false; this.#changed(); });
    c.on('error', (e) => { this.lastError = String(e?.message ?? e); this.#changed(); });

    c.on('message', (topic, payload) => {
      const body = payload.toString();
      if (topic === `${base}/error`) {
        try { this.lastError = JSON.parse(body).message ?? body; } catch { this.lastError = body; }
        this.#changed();
        return;
      }
      const state = topic.startsWith(`${base}/state/`) && topic.slice(base.length + 7);
      if (state) {
        this.#apply(state, decode(body));
        this.#changed();
        return;
      }
      const ev = topic.startsWith(`${base}/event/`) && topic.slice(base.length + 7);
      if (ev) {
        let data: any = body;
        try { data = JSON.parse(body); } catch { /* not json */ }
        this.#onEvent.forEach((f) => f(ev, data));
      }
    });
  }

  /** Events are opt-in. Subscribing every open page to a chatty serial port's
   *  byte stream would be a waste; state is small and always wanted. */
  subscribeEvents(...topics: string[]) {
    const full = topics.map((t) => `${this.base}/event/${t}`);
    this.#c?.subscribe(full);
    return () => this.#c?.unsubscribe(full);
  }

  #publish(tail: string, body: string) {
    if (!this.#c?.connected) throw new Error('not connected to the IO server');
    this.lastError = null;
    this.#c.publish(`${this.base}/cmd/${tail}`, body);
  }

  // Commands. Fire-and-forget by design: the answer is not a return value, it
  // is the retained state topic changing — which every other open page sees too.
  relaySet = (i: number, on: boolean) => this.#publish(`relay/${panel(i)}/set`, on ? 'ON' : 'OFF');
  relayToggle = (i: number) => this.#publish(`relay/${panel(i)}/set`, 'TOGGLE');
  /** `null` targets the front blaster, which is not a jack and has no number.
   *  A rear jack is addressed by the number printed on the case. */
  sendIr = (port: number | null, pronto: string) =>
    this.#publish(`ir/${port === null ? 'front' : panel(port)}/send`, pronto);
  /** LEDs are addressed by NAME, not index — they are not a numbered row on
   *  the panel, and their names come from the kernel. */
  setLed = (slug: string, on: boolean) =>
    this.#publish(`led/${slug}/set`, on ? 'ON' : 'OFF');
  setBaud = (i: number, baud: number) => this.#publish(`serial/${panel(i)}/baud`, String(baud));
  serialWrite = (i: number, data: string) => this.#publish(`serial/${panel(i)}/write`, data);

  /** Nested-set `relay/1` → state.relay['1']. */
  #apply(path: string, value: unknown) {
    const parts = path.split('/');
    let cur: any = this.state;
    for (const p of parts.slice(0, -1)) cur = cur[p] ??= {};
    cur[parts[parts.length - 1]] = value;
  }
  #changed() {
    // Replace the object so React sees a new reference.
    this.state = { ...this.state };
    this.#onChange.forEach((f) => f());
  }
}

export const io = new Io();

/** The terminal socket: raw bytes, because a console is a byte stream with
 *  backpressure and scrollback, none of which pub/sub does well. It attaches to
 *  the shared session iod reports on, so every viewer sees the same stream. */
export const serialSocket = (index: number) =>
  new WebSocket(auth(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/iod/ws/serial/${panel(index)}`));
