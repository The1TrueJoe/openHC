import { useMemo, useState } from 'react';
import { Bluetooth, Search, Signal } from 'lucide-react';
import { useTree, parse } from './model';

/* Bluetooth LE: what bluetooth-bridge hears under <base>/bluetooth.
   Sensors are the advertisers it decodes (BTHome, Ruuvi, ...): `device/<ADDR>`
   holds their readings. Everything else in range is `scan/<ADDR>`, one message
   per advertisement after the bridge's duplicate filter. Broadcast sensors need
   no pairing, so there is no "add device" here: in range is joined. */

type Scan = {
  address?: string; rssi?: number; connectable?: boolean;
  advertisement?: { local_name?: { name?: string }; manufacturer_data?: { company_id: number; company?: string }[]; service_data?: { name?: string }[] };
  beacons?: { format?: string }[];
};

const ago = (ms?: number) => {
  if (!ms) return '';
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return s < 60 ? `${s}s ago` : s < 3600 ? `${Math.round(s / 60)}m ago` : `${Math.round(s / 3600)}h ago`;
};
const fmtAddr = (a: string) => a.match(/../g)?.join(':') ?? a;
const nameOf = (s?: Scan) => s?.advertisement?.local_name?.name || '';
const unitOf: Record<string, string> = { temperature: '°C', humidity: '%', battery: '%', pressure: 'hPa', illuminance: 'lx', voltage: 'V' };

export function BluetoothPanel() {
  const t = useTree('bluetooth');
  const [q, setQ] = useState('');
  const info = parse(t.topics.get('bridge/info'));
  const online = parse(t.topics.get('bridge/state'))?.state !== 'offline';

  const { sensors, nearby } = useMemo(() => {
    const scans = new Map<string, Scan>();
    const sensors: { addr: string; r: Record<string, unknown>; scan?: Scan }[] = [];
    for (const [tail, body] of t.topics) {
      if (tail.startsWith('scan/')) scans.set(tail.slice(5), parse(body));
    }
    for (const [tail, body] of t.topics) {
      if (tail.startsWith('device/') && !tail.slice(7).includes('/')) {
        const addr = tail.slice(7);
        sensors.push({ addr, r: parse(body) ?? {}, scan: scans.get(addr) });
        scans.delete(addr);
      }
    }
    const nearby = [...scans.entries()].sort((a, b) => (b[1]?.rssi ?? -200) - (a[1]?.rssi ?? -200));
    return { sensors, nearby };
  }, [t.topics]);

  const match = (addr: string, s?: Scan) => !q || `${addr} ${nameOf(s)}`.toLowerCase().includes(q.toLowerCase());

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-3">
        <Bluetooth size={20} className="text-accent" />
        <h1 className="text-lg font-semibold">Bluetooth</h1>
        <span className={`text-xs ${online ? 'text-live' : 'text-warm'}`}>{online ? 'scanning' : 'offline'}</span>
        <span className="text-xs text-muted">
          {[info?.address && fmtAddr(String(info.address).replace(/:/g, '')), info?.core_version?.replace(/^Bluetooth® Core Specification /, 'Bluetooth ').replace(/ \(Withdrawn\)$/, '')]
            .filter(Boolean).join(' · ')}
        </span>
        <label className="hair ml-auto flex items-center gap-2 rounded-lg border px-2.5 py-1.5 text-sm">
          <Search size={14} className="text-muted" />
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Find" className="w-32 bg-transparent outline-none" />
        </label>
      </div>

      <section className="hair rounded-xl border bg-panel p-4">
        <h2 className="mb-1 text-sm font-medium">Sensors</h2>
        <p className="mb-3 text-xs text-muted">Decoded from their broadcasts (BTHome, Ruuvi, Xiaomi with BTHome firmware, Shelly BLU). Nothing to pair: bring one in range.</p>
        {sensors.length ? (
          <div className="space-y-2">
            {sensors.filter((s) => match(s.addr, s.scan)).map(({ addr, r, scan }) => (
              <div key={addr} className="hair flex items-center gap-3 rounded-xl border bg-raised p-3">
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm">{nameOf(scan) || fmtAddr(addr)}</div>
                  <div className="mt-1 flex flex-wrap gap-1.5">
                    {Object.entries(r)
                      .filter(([k, v]) => k !== 'rssi' && k !== 'last_seen' && (typeof v === 'number' || typeof v === 'boolean'))
                      .map(([k, v]) => (
                        <span key={k} className="rounded-md shade px-1.5 py-0.5 text-xs">
                          <span className="text-muted">{k.replace(/_/g, ' ')} </span>
                          {typeof v === 'boolean' ? (v ? 'yes' : 'no') : `${v}${unitOf[k] ?? ''}`}
                        </span>
                      ))}
                  </div>
                </div>
                <Rssi v={r.rssi as number | undefined} />
                <span className="w-16 shrink-0 text-right text-xs text-muted">{ago(r.last_seen as number | undefined)}</span>
              </div>
            ))}
          </div>
        ) : (
          <p className="text-xs text-muted">No sensors heard yet.</p>
        )}
      </section>

      <section className="hair rounded-xl border bg-panel p-4">
        <h2 className="mb-3 text-sm font-medium">Nearby <span className="text-xs font-normal text-muted">{nearby.length}</span></h2>
        {nearby.length ? (
          <div className="divide-y divide-edge">
            {/* The 100 strongest; a busy site can have thousands. */}
            {nearby.filter(([a, s]) => match(a, s)).slice(0, 100).map(([addr, s]) => {
              const md = s?.advertisement?.manufacturer_data?.[0];
              const what = [
                s?.beacons?.map((b) => b.format?.replace(/_/g, ' ')).join(', '),
                md?.company ?? (md ? `company 0x${md.company_id.toString(16).padStart(4, '0')}` : ''),
                s?.advertisement?.service_data?.[0]?.name,
                s?.connectable ? 'connectable' : '',
              ].filter(Boolean).join(' · ');
              return (
                <div key={addr} className="flex items-center gap-3 py-2">
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm">{nameOf(s) || <span className="font-mono text-xs">{fmtAddr(addr)}</span>}</div>
                    <div className="truncate text-xs text-muted">{nameOf(s) ? `${fmtAddr(addr)} · ` : ''}{what}</div>
                  </div>
                  <Rssi v={s?.rssi} />
                </div>
              );
            })}
          </div>
        ) : (
          <p className="text-xs text-muted">Nothing heard yet.</p>
        )}
      </section>
    </div>
  );
}

function Rssi({ v }: { v?: number }) {
  if (typeof v !== 'number') return null;
  return (
    <span className={`flex w-20 shrink-0 items-center justify-end gap-1 text-xs tabular-nums ${v > -70 ? 'text-live' : v > -85 ? 'text-ink' : 'text-muted'}`}>
      <Signal size={12} /> {v} dBm
    </span>
  );
}
