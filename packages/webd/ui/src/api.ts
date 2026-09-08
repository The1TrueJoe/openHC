// The client for iod's control socket.
//
// One socket carries three things: a mirror of everything the box currently IS,
// a stream of things that HAPPEN, and commands with correlation ids. That is
// deliberately not three connections — a UI that polls `/api/io/relays` shows a
// relay somebody else just closed as still open until the next poll, and two
// installers on the same page then disagree about the hardware.
//
// webd serves this page on :80 and answers system/network questions; iod owns
// every IO on the controller and answers on :7070. Separate processes on
// purpose — one owner for a UART that can only answer one question at a time.

export const IOD = `${location.protocol}//${location.hostname}:7070`;
const WS = `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.hostname}:7070`;

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
}

/** Exactly what the board has. A section is ABSENT when the count is zero —
 *  that is the contract, and it is why the UI can render without knowing what
 *  model it is talking to. */
export interface Capabilities {
  board: string;
  hostname: string;
  backend: Backend;
  mcu_linked: boolean;
  ir?: { out: number; blaster: number; total: number; combo: number; receiver: number };
  relays?: { count: number };
  contacts?: { count: number };
  serials?: SerialPort[];
  bauds?: number[];
}

export interface McuInfo {
  part: string;
  baud: number;
  product: string;
  version: string;
  /** What the MCU says it measured, which is a useful link check: an HC-800
   *  reports ~115207 against a nominal 115200. */
  measured_baud: number | null;
}

/** The mirrored state tree, exactly as iod publishes it. */
export interface IoState {
  relay?: Record<string, boolean>;
  contact?: Record<string, boolean>;
  mcu?: { link?: boolean };
  serial?: Record<string, { baud?: number; viewers?: number }>;
}

type Pending = { resolve: (v: any) => void; reject: (e: Error) => void };

export class Control {
  state: IoState = {};
  connected = false;

  #ws: WebSocket | null = null;
  #id = 0;
  #pending = new Map<number, Pending>();
  #topics = new Set<string>();
  #onChange = new Set<() => void>();
  #onEvent = new Set<(topic: string, data: any) => void>();
  #retry: number | null = null;

  /** Re-render hook. Every state delta calls these. */
  watch(fn: () => void) {
    this.#onChange.add(fn);
    return () => this.#onChange.delete(fn);
  }
  /** Events, which are not state: serial bytes, IR received. */
  onEvent(fn: (topic: string, data: any) => void) {
    this.#onEvent.add(fn);
    return () => this.#onEvent.delete(fn);
  }

  open() {
    if (this.#ws) return;
    const ws = new WebSocket(auth(`${WS}/ws/control`));
    this.#ws = ws;

    ws.onopen = () => {
      this.connected = true;
      // Re-subscribe: a reconnected socket is a NEW subscription state on the
      // server, and silently losing the serial feed after a blip is the kind of
      // bug that looks like broken hardware.
      if (this.#topics.size) ws.send(JSON.stringify({ op: 'subscribe', topics: [...this.#topics] }));
      this.#changed();
    };

    ws.onmessage = (m) => {
      let msg: any;
      try { msg = JSON.parse(m.data); } catch { return; }

      // A command reply, matched by the id we sent.
      if (msg.id != null && this.#pending.has(msg.id)) {
        const p = this.#pending.get(msg.id)!;
        this.#pending.delete(msg.id);
        msg.ok ? p.resolve(msg.result) : p.reject(new Error(msg.error?.message ?? 'command failed'));
        return;
      }

      switch (msg.type) {
        case 'snapshot':
          this.state = msg.state ?? {};
          this.#changed();
          break;
        case 'state':
          this.#apply(msg.path, msg.value);
          this.#changed();
          break;
        case 'event':
          this.#onEvent.forEach((f) => f(msg.topic, msg.data));
          break;
        // We fell behind and the bus dropped messages for us. iod resends the
        // snapshot straight after, so there is nothing to do but not pretend
        // the mirror was continuous.
        case 'lagged':
          break;
      }
    };

    const down = () => {
      this.connected = false;
      this.#ws = null;
      this.#pending.forEach((p) => p.reject(new Error('socket closed')));
      this.#pending.clear();
      this.#changed();
      // Reconnect: a controller reboot, or webd restarting, should not require
      // the installer to reload the page from a ladder.
      if (this.#retry == null) this.#retry = window.setTimeout(() => { this.#retry = null; this.open(); }, 1500);
    };
    ws.onclose = down;
    ws.onerror = () => ws.close();
  }

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

  /** Issue a command and wait for its reply. */
  send<T = any>(body: Record<string, unknown>): Promise<T> {
    return new Promise((resolve, reject) => {
      const ws = this.#ws;
      if (!ws || ws.readyState !== WebSocket.OPEN) return reject(new Error('not connected'));
      const id = ++this.#id;
      this.#pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, ...body }));
    });
  }

  /** Events are opt-in; state always flows. Subscribing to a chatty serial
   *  port should be a decision, not the default for every open page. */
  subscribe(...topics: string[]) {
    topics.forEach((t) => this.#topics.add(t));
    if (this.#ws?.readyState === WebSocket.OPEN) this.#ws.send(JSON.stringify({ op: 'subscribe', topics }));
    return () => this.unsubscribe(...topics);
  }
  unsubscribe(...topics: string[]) {
    topics.forEach((t) => this.#topics.delete(t));
    if (this.#ws?.readyState === WebSocket.OPEN) this.#ws.send(JSON.stringify({ op: 'unsubscribe', topics }));
  }

  // Typed shorthands for what the panels actually do.
  capabilities = () => this.send<Capabilities>({ op: 'capabilities' });
  mcu = () => this.send<McuInfo>({ op: 'mcu.info' });
  relaySet = (index: number, on: boolean) => this.send({ op: 'relay.set', index, on });
  relayToggle = (index: number) => this.send({ op: 'relay.toggle', index });
  sendIr = (port: number, pronto: string, repeat = 1) => this.send({ op: 'ir.send', port, pronto, repeat });
  setBaud = (index: number, baud: number) => this.send({ op: 'serial.baud', index, baud });
}

export const control = new Control();

/** The terminal socket: raw bytes, because a console stream is not JSON.
 *  It attaches to the same shared session the control socket reports on, so
 *  every viewer of a port sees the same stream. */
export const serialSocket = (index: number) => new WebSocket(auth(`${WS}/ws/serial/${index}`));

/** REST, for the one thing that happens before the socket is up. */
export async function fetchCapabilities(): Promise<Capabilities> {
  const r = await fetch(auth(`${IOD}/api/io`));
  if (!r.ok) {
    let detail = `${r.status}`;
    try { const b = await r.json(); if (b?.error) detail = b.error; } catch { /* not json */ }
    throw new Error(detail);
  }
  return r.json();
}
