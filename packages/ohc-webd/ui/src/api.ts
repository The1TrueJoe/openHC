// Typed client for the ohc-webd REST API. Shapes mirror the Rust serde structs.
export interface Radio { type: string; dev: string; }
export interface SerialPort { dev: string; label: string; baud: number; }
export interface Board {
  model: string;
  hostname: string;
  uplink_iface: string;
  wifi_iface: string;
  radios: Radio[];
  serials: SerialPort[];
}
export interface Iface { name: string; ip: string; carrier: boolean; }
export interface System {
  hostname: string;
  kernel: string;
  uptime: number;
  loadavg: string;
  interfaces: Iface[];
}

const j = (p: string, opt?: RequestInit) => fetch(p, opt).then((r) => r.json());

export const api = {
  board: (): Promise<Board> => j("/api/board"),
  system: (): Promise<System> => j("/api/system"),
  radioTx: (kind: string, hex: string): Promise<{ written?: number; error?: string }> =>
    j(`/api/radios/${kind}/tx`, { method: "POST", body: JSON.stringify({ hex }) }),
  radioRx: (kind: string, ms = 500): Promise<{ hex: string; text: string; error?: string }> =>
    j(`/api/radios/${kind}/rx?ms=${ms}`),
  radioReset: (kind: string): Promise<{ reset: boolean }> =>
    j(`/api/radios/${kind}/reset`, { method: "POST" }),
};

export function serialWsUrl(dev: string): string {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  return `${proto}://${location.host}/ws/serial/${dev.replace("/dev/", "")}`;
}

export function fmtUptime(s: number): string {
  s = Math.floor(s);
  const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
  return (d ? d + "d " : "") + (h ? h + "h " : "") + m + "m";
}
