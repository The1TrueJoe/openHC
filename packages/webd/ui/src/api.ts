// Clients for the two daemons behind this UI.
//
// iod owns every IO on the controller and answers on :7070; webd serves this
// page and answers system/network questions on :80. They are separate processes
// on purpose — one owner for a UART that can only answer one question at a time
// — so the UI simply talks to both rather than pretending it is one service.

/** iod's base URL: same host as this page, its own port. */
export const IOD = `${location.protocol}//${location.hostname}:7070`;
const wsBase = () => `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.hostname}:7070`;

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
}

export interface McuInfo {
  part: string;
  baud: number;
  product: string;
  version: string;
  measured_baud: number | null;
}

export type IodEvent =
  | { type: 'contact'; index: number; closed: boolean }
  | { type: 'contact_snapshot'; mask: number; closed: boolean[] }
  | { type: 'mcu_link'; up: boolean };

async function j<T>(url: string, init?: RequestInit): Promise<T> {
  const r = await fetch(url, init);
  if (!r.ok) {
    // iod answers errors as {"error": "..."} — surface that, not "500".
    let detail = `${r.status}`;
    try {
      const b = await r.json();
      if (b?.error) detail = b.error;
    } catch { /* body was not json */ }
    throw new Error(detail);
  }
  return r.json() as Promise<T>;
}

export const iod = {
  capabilities: () => j<Capabilities>(`${IOD}/api/io`),
  mcu: () => j<McuInfo>(`${IOD}/api/io/mcu`),
  contacts: () => j<{ mask: number; closed: boolean[] }>(`${IOD}/api/io/contacts`),
  relays: () => j<{ count: number; raw: number[]; decoded: boolean }>(`${IOD}/api/io/relays`),
  toggleRelays: (mask: number) =>
    j<{ raw: number[] }>(`${IOD}/api/io/relays/toggle`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ mask }),
    }),
  sendIr: (port: number, pronto: string, repeat = 1) =>
    j<unknown>(`${IOD}/api/io/ir/${port}/send`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ pronto, repeat }),
    }),
  events: () => new WebSocket(`${wsBase()}/ws/events`),
  serial: (index: number) => new WebSocket(`${wsBase()}/ws/serial/${index}`),
};
