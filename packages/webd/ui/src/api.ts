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
/* ohc-audiod's configuration REST, proxied by webd at /audio. */
const AUDIO = `${location.origin}/audio`;
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
  /** Audio, discovered at runtime (not board.env). Absent when the box has no
   *  sound card AND no receiver binaries — the same "nothing behind it" rule. */
  audio?: AudioStatus;
}

/** An ALSA playback device the receivers can be pointed at. `id` is what you
 *  pass to `aplay -D` / librespot `--device` (e.g. `hw:DSP`). */
export interface AudioOutput {
  id: string;
  name: string;
  card: number;
}

/** A network audio receiver. Spotify Connect and AirPlay are *receivers* —
 *  playback is driven from the phone — so `supports_transport` is honest about
 *  whether this build exposes play/pause at all (it does not, today). */
export interface AudioReceiver {
  id: 'librespot' | 'shairport';
  name: string;
  kind: 'spotify' | 'airplay';
  installed: boolean;
  running: boolean;
  supports_transport: boolean;
  supports_metadata: boolean;
  /** Whatever JSON a receiver's hook fed us; absent means nothing playing. */
  now_playing?: unknown;
}

/** A named jack on a board with an endpoint map (board.env OHC_AUDIO_OUTPUTS). */
export interface AudioPort {
  id: string;
  label: string;
  device: string;
  pcm: string;
}

/** A Spotify Connect or AirPlay endpoint: the name a phone shows, and the
 *  output it plays on. */
export interface AudioEndpoint {
  name: string;
  output: string;
}

/** A live route from an input to an output. */
export interface AudioRoute {
  input: string;
  output: string;
}

/** The configurable part of the map — what PUT /audio/api/audio/endpoints takes. */
export interface AudioEndpoints {
  spotify: AudioEndpoint[];
  airplay: AudioEndpoint[];
  routes: AudioRoute[];
  /** The music-library player (mpd over the drives on /media): 0 or 1. */
  library?: AudioEndpoint[];
}

/** One supervised process ohc-audiod runs for the map. */
export interface AudioInstance {
  tag: string;
  kind: 'spotify' | 'airplay' | 'route' | 'library' | 'helper';
  name: string | null;
  input: string | null;
  output: string | null;
  running: boolean;
  /** Actually playing audio right now (a route: whenever it runs). */
  playing?: boolean;
}

/** ohc-audiod's endpoint map: the board's outputs/inputs, the configured
 *  endpoints, and what is running. Arrives over REST once and over MQTT
 *  (`<base>/state/audio/map`) whenever it changes. */
export interface AudioMap extends AudioEndpoints {
  rate: number;
  outputs: AudioPort[];
  inputs: AudioPort[];
  instances: AudioInstance[];
}

/** Now playing on one Spotify/AirPlay endpoint (`<base>/state/audio/meta/<tag>`). */
export interface AudioMeta {
  state?: 'playing' | 'paused' | 'stopped' | '';
  title?: string;
  artist?: string;
  album?: string;
  /** An absolute image URL (Spotify), or a path under ohc-audiod's API (AirPlay). */
  cover?: string;
  /** The phone or app driving it. */
  client?: string;
  duration_ms?: number;
}

/** The music-library player (`<base>/state/audio/library`). */
export interface LibraryState {
  state: 'play' | 'pause' | 'stop' | 'offline';
  elapsed_s?: number;
  /** When elapsed_s was read (Unix ms): the page runs the clock from there. */
  elapsed_at_ms?: number;
  duration_s?: number;
  position?: number;
  queue_length: number;
  random: boolean;
  repeat: boolean;
  updating: boolean;
}

/** A folder or track in the library (REST browse/search). */
export interface LibraryEntry {
  kind: 'dir' | 'file';
  path: string;
  title?: string;
  artist?: string;
  album?: string;
  duration_s?: number;
}

/** A displayable URL for `AudioMeta.cover`. */
export const audioCover = (c: string) => (/^https?:\/\//.test(c) ? c : auth(`${AUDIO}/api/audio/${c}`));

/** `/api/audio`, and the shape inside `caps.audio`. `volume`/`now_playing` are
 *  only present when genuinely available, so the panel shows them conditionally. */
export interface AudioStatus {
  outputs: AudioOutput[];
  receivers: AudioReceiver[];
  /** The chosen output id, or null when the receivers follow the default PCM. */
  selected: string | null;
  volume?: number;
  /** Present on boards with named outputs: N endpoints mapped onto jacks. */
  map?: AudioMap | null;
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
  /** Live audio deltas: the selected output, the volume, and per-receiver
   *  running flags. The fuller picture (outputs, now-playing) comes over REST. */
  audio?: {
    output?: string;
    volume?: number;
    receiver?: Record<string, { running?: boolean }>;
    /** Boards with named outputs: the endpoint map, live. */
    map?: AudioMap;
    /** Output id → level 0..100: the one volume of that jack, which its
     *  AirPlay/Spotify endpoints' sliders also move. */
    level?: Record<string, number>;
    /** Output id → an announcement is playing (its music is ducked). */
    announcing?: Record<string, boolean>;
    /** Endpoint tag (`spotify-1`, `airplay-0`) → what it is playing. */
    meta?: Record<string, AudioMeta>;
    /** The library player's transport state. */
    library?: LibraryState;
    /** Input id → triggered on (playing to its default routes). */
    input?: Record<string, boolean>;
    /** Output id → tone (images with the tone stage): dB, dB, -100..100. */
    bass?: Record<string, number>;
    treble?: Record<string, number>;
    balance?: Record<string, number>;
  };
  /** sysmond telemetry, republished by iod: the latest sample, the ring at one
   *  point a minute, and the fan. */
  health?: { now?: Telemetry; history?: History; fan?: FanStatus };
  /** Return-to-stock availability (boards with the ohc-restore helper). */
  system?: { restore?: { available: boolean; openhc?: boolean; detail?: string } };
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
export interface Series {
  slug: string;
  label: string;
  chip: string;
  kind: 'temp' | 'fan' | 'pwm';
}

export interface Telemetry {
  at: number | null;
  series: Series[];
  values: (number | null)[] | null;
  cpu: number | null;
  load1: number | null;
  mem_used_pct: number | null;
  mem_total_kb: number | null;
  uptime_s: number | null;
}

/** sysmond's in-RAM ring. `series` is sent ONCE and each sample's `v` is
 *  positional against it — the same compaction sysmond uses internally, which is
 *  why the labels do not repeat per sample. `held`/`capacity` describe the ring,
 *  not the window actually returned. */
export interface History {
  period_s: number;
  held: number;
  /** The REST ring reports its capacity; the minute-resolution copy on MQTT does not. */
  capacity?: number;
  series: Series[];
  samples: {
    at: number;
    v: (number | null)[];
    cpu: number | null;
    load1: number | null;
    mem_used_pct: number | null;
  }[];
}

/** Fan presence and mode. `available` is false on boards with no fan helper
 *  (the Health panel then shows only readings); `mode` is auto|manual and `pct`
 *  is the fan's current duty when a controllable fan is present. */
export interface FanStatus {
  available: boolean;
  mode?: 'auto' | 'manual';
  pct?: number | null;
  error?: string;
}

export const rest = {
  /** Telemetry lives behind /sys, proxied by webd to sysmond on :7071. */
  telemetry: () => j<Telemetry>(`${SYS}/api/now`),
  /** The telemetry ring. `seconds` trims the window; absent/0 is everything held. */
  history: (seconds?: number) =>
    j<History>(`${SYS}/api/history${seconds ? `?seconds=${seconds}` : ''}`),
  /** Fan presence/mode/duty. `available:false` on boards with no fan helper. */
  fan: () => j<FanStatus>(`${SYS}/api/fan`),
  /** ohc-audiod: outputs, inputs, the endpoint map, the receivers. Loaded once;
   *  live changes arrive over MQTT. */
  audio: () => j<AudioStatus>(`${AUDIO}/api/audio`),
  /** Replace the endpoint map (configuration). */
  libraryBrowse: (path: string) =>
    j<LibraryEntry[]>(`${AUDIO}/api/audio/library/browse?path=${encodeURIComponent(path)}`),
  librarySearch: (q: string) =>
    j<LibraryEntry[]>(`${AUDIO}/api/audio/library/search?q=${encodeURIComponent(q)}`),
  saveEndpoints: (e: AudioEndpoints) =>
    j<AudioMap>(`${AUDIO}/api/audio/endpoints`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(e),
    }),
  /** Manual override, 0-100. Persists until released; fail-safe still applies. */
  fanSet: (pct: number) =>
    j<{ ok: boolean; mode?: string; pct?: number; error?: string }>(`${SYS}/api/fan/set`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ pct: Math.max(0, Math.min(100, Math.round(pct))) }),
    }),
  /** Release the fan back to its automatic curve. */
  fanAuto: () =>
    j<{ ok: boolean; mode?: string; error?: string }>(`${SYS}/api/fan/auto`, { method: 'POST' }),
  capabilities: () => j<Capabilities>(`${HTTP}/api/io`),
  mcu: () => j<McuInfo>(`${HTTP}/api/io/mcu`),
  config: () => j<ConfigDoc>(`${HTTP}/api/config`),
  saveConfig: (mqtt: MqttWrite) =>
    j<{ ok: boolean }>(`${HTTP}/api/config`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ mqtt }),
    }),

  /** Return-to-stock. `available` is false on boards with no CEFDK/MFH (the UI
   *  then shows nothing); `openhc` true means the install's MFH item is present
   *  and a restore would do something. */
  restoreStatus: () =>
    j<{ available: boolean; openhc?: boolean; detail?: string; reason?: string }>(
      `${HTTP}/api/system/restore`,
    ),
  /** DESTRUCTIVE, one-way: the controller reboots to stock. */
  restoreStock: () =>
    j<{ started: boolean; note?: string }>(`${HTTP}/api/system/restore/stock`, { method: 'POST' }),
};

/** `ON`/`OFF` become booleans, digits become numbers, JSON objects/arrays are
 *  parsed, everything else stays a string. iod publishes scalars bare so a
 *  shell script can read them without a JSON parser, and anything structured as
 *  JSON (mqtt/topics.rs `payload`); this is the other half of that bargain.
 *  Without the JSON half, a structured value (the audio endpoint map) arrived
 *  as a string and the panel rendering it crashed. */
function decode(raw: string): unknown {
  if (raw === 'ON') return true;
  if (raw === 'OFF') return false;
  if (raw !== '' && !Number.isNaN(Number(raw))) return Number(raw);
  const c = raw[0];
  if (c === '{' || c === '[') {
    try { return JSON.parse(raw); } catch { /* not JSON after all: keep the text */ }
  }
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
  /** Point the network receivers at an ALSA output. `id` is a device string like
   *  `hw:DSP`; they apply it on their next restart. */
  setAudioOutput = (id: string) => this.#publish('audio/output', id);
  /** Output volume, 0..100. Clamped and read back by iod. */
  setAudioVolume = (percent: number) =>
    this.#publish('audio/volume', String(Math.max(0, Math.min(100, Math.round(percent)))));
  /** An output's level, 0..100 (boards with named outputs). */
  setAudioLevel = (output: string, percent: number) =>
    this.#publish(`audio/level/${output}`, String(Math.max(0, Math.min(100, Math.round(percent)))));
  /** Play an announcement over an output's music, which ducks under it:
   *  `chime`, an http(s) URL or an absolute path to a WAV on the box. */
  announce = (output: string, source = 'chime') => this.#publish(`audio/announce/${output}`, source);
  /** Trigger an input on or off: while on it plays to its default routes. */
  setInput = (input: string, on: boolean) => this.#publish(`audio/input/${input}`, on ? 'ON' : 'OFF');
  /** An output's tone: bass/treble in dB (-12..12), balance -100 (left) .. 100 (right). */
  setTone = (output: string, knob: 'bass' | 'treble' | 'balance', value: number) =>
    this.#publish(`audio/${knob}/${output}`, String(Math.round(value)));
  /** The library player: play|pause|toggle|stop|next|previous|seek <s>|clear|
   *  add <uri>|replace <uri>|random ON/OFF|repeat ON/OFF|update. */
  library = (verb: string, arg = '') => this.#publish(`audio/library/${verb}`, arg);
  /** Fan: a percent holds it there, 'auto' hands it back to the curve. */
  setFan = (v: number | 'auto') =>
    this.#publish('health/fan', v === 'auto' ? 'auto' : String(Math.max(0, Math.min(100, Math.round(v)))));
  /** Return to stock. One-way; iod acts only on the literal "confirm". */
  restoreStock = () => this.#publish('system/restore', 'confirm');

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
