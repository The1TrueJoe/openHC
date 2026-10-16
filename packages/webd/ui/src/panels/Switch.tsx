import { useState } from 'react';
import { Network, Save, AlertTriangle, CheckCircle2, Cable } from 'lucide-react';
import {
  io, rest,
  type Capabilities, type SwitchConfig, type SwitchMode, type Addressing,
  type SwitchPortStatus, type ApplyResult,
} from '../api';
import { useIoState } from '../App';

/* The managed switch (switchd, on boards with the BCM53125 DSA switch).
 *
 * Default is a plain managed switch: every port bridged into one L2 domain, one
 * address on the bridge. From there the ports can be split into separate IP
 * interfaces (Isolated) or carved into 802.1Q VLANs (Custom). Live per-port link
 * and counters arrive over MQTT; the topology is configuration, saved over REST
 * and re-applied on boot — the owner's MQTT-vs-REST split.
 *
 * Nothing here polls: the port cards redraw from the retained `state/switch/...`
 * topics, and the form is seeded once from the REST overview the app loaded. */
export function SwitchPanel({ caps }: { caps: Capabilities }) {
  const sw = caps.switch;
  const live = useIoState().switch?.port;
  const [cfg, setCfg] = useState<SwitchConfig>(() => structuredClone(caps.switch!.config));
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ApplyResult | null>(null);
  const [errs, setErrs] = useState<string[] | null>(null);

  if (!sw) return null;

  // Canonical port set is what the board actually has; live MQTT status wins
  // over the slower REST snapshot where present.
  const ports: SwitchPortStatus[] = sw.ports.map((p) => ({ ...p, ...(live?.[p.name] ?? {}) }));
  const names = ports.map((p) => p.name);

  const setMode = (mode: SwitchMode) => {
    setCfg((c) => {
      const next = structuredClone(c);
      next.mode = mode;
      // Managed means every port in the bridge; the other modes leave membership
      // to the per-port controls below.
      if (mode === 'managed') next.bridge.members = [...names];
      return next;
    });
  };

  const patch = (fn: (c: SwitchConfig) => void) =>
    setCfg((c) => { const n = structuredClone(c); fn(n); return n; });

  const portCfg = (name: string) =>
    cfg.ports[name] ?? { enabled: true };

  const save = async () => {
    setBusy(true); setResult(null); setErrs(null);
    try {
      setResult(await rest.saveSwitchConfig(cfg));
    } catch (e) {
      // switchd returns {errors:[…]} on a refused config; j() surfaces the first.
      setErrs([String((e as Error).message ?? e)]);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <header className="flex items-center gap-2">
        <Network size={20} className="text-accent" />
        <h1 className="text-lg font-semibold tracking-tight">Managed switch</h1>
        <span className="ml-auto text-xs text-muted">
          {sw.bridge.present
            ? <>bridge <code className="text-ink">{sw.bridge.name}</code> up</>
            : <span className="text-warm">bridge not yet built</span>}
        </span>
      </header>

      {/* Live ports — always shown, independent of the chosen mode. */}
      <section className="hair rounded-xl border bg-panel p-4">
        <h2 className="mb-3 text-sm font-semibold text-ink">Ports</h2>
        <div className="grid gap-2 sm:grid-cols-2">
          {ports.map((p) => <PortCard key={p.name} p={p} />)}
        </div>
      </section>

      {/* Mode */}
      <section className="hair rounded-xl border bg-panel p-4">
        <h2 className="mb-1 text-sm font-semibold text-ink">Mode</h2>
        <p className="mb-3 text-xs text-muted">
          How the two jacks behave. Changing this and saving reconfigures the switch live.
        </p>
        <div className="grid gap-2 sm:grid-cols-3">
          <ModeCard id="managed" cur={cfg.mode} onPick={setMode}
            title="Managed switch" desc="Both ports one L2 segment, one address. The default." />
          <ModeCard id="isolated" cur={cfg.mode} onPick={setMode}
            title="Separate interfaces" desc="Each port its own routed IP interface." />
          <ModeCard id="custom" cur={cfg.mode} onPick={setMode}
            title="VLANs / custom" desc="802.1Q VLANs and explicit bridge membership." />
        </div>
      </section>

      {/* Mode-specific settings */}
      {cfg.mode === 'managed' && (
        <section className="hair rounded-xl border bg-panel p-4 space-y-3">
          <h2 className="text-sm font-semibold text-ink">Bridge</h2>
          <Toggle label="Spanning tree (STP)"
            hint="Keeps both jacks on one network from forming a loop. Leave on unless you know the topology is loop-free."
            value={cfg.bridge.stp}
            onChange={(v) => patch((c) => { c.bridge.stp = v; })} />
          <div>
            <div className="mb-1 text-xs text-muted">Bridge address</div>
            <AddressingEditor value={cfg.bridge.addressing}
              onChange={(a) => patch((c) => { c.bridge.addressing = a; })} />
          </div>
        </section>
      )}

      {cfg.mode === 'isolated' && (
        <section className="hair rounded-xl border bg-panel p-4 space-y-3">
          <h2 className="text-sm font-semibold text-ink">Per-port addressing</h2>
          {names.map((n) => (
            <div key={n} className="hair rounded-lg border bg-raised p-3">
              <div className="mb-2 flex items-center gap-2">
                <Cable size={15} className="text-muted" />
                <span className="font-mono text-sm text-ink">{n}</span>
                <label className="ml-auto flex items-center gap-1.5 text-xs text-muted">
                  <input type="checkbox" checked={portCfg(n).enabled}
                    onChange={(e) => patch((c) => { (c.ports[n] ??= { enabled: true }).enabled = e.target.checked; })} />
                  enabled
                </label>
              </div>
              <AddressingEditor
                value={portCfg(n).addressing ?? { mode: 'none' }}
                onChange={(a) => patch((c) => { (c.ports[n] ??= { enabled: true }).addressing = a; })} />
            </div>
          ))}
        </section>
      )}

      {cfg.mode === 'custom' && (
        <CustomSettings cfg={cfg} names={names} patch={patch} portCfg={portCfg} />
      )}

      {/* Save */}
      <div className="flex items-center gap-3">
        <button onClick={save} disabled={busy}
          className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-3 py-1.5 text-sm text-ink hover:border-accent/50 disabled:opacity-50">
          <Save size={15} /> {busy ? 'Applying…' : 'Apply & save'}
        </button>
        {result && (result.ok
          ? <span className="flex items-center gap-1.5 text-sm text-live"><CheckCircle2 size={15} /> Applied{result.l3_iface ? ` — address on ${result.l3_iface}` : ''}</span>
          : <span className="flex items-center gap-1.5 text-sm text-warm"><AlertTriangle size={15} /> Applied with {result.errors.length} problem(s)</span>)}
        {errs && <span className="flex items-center gap-1.5 text-sm text-alarm"><AlertTriangle size={15} /> {errs[0]}</span>}
      </div>
      {result && result.errors.length > 0 && (
        <ul className="hair rounded-lg border bg-raised p-3 font-mono text-xs text-alarm/90 space-y-0.5">
          {result.errors.map((e, i) => <li key={i}>{e}</li>)}
        </ul>
      )}
    </div>
  );
}

function PortCard({ p }: { p: SwitchPortStatus }) {
  const up = p.carrier;
  return (
    <div className="hair rounded-lg border bg-raised p-3">
      <div className="flex items-center gap-2">
        <Cable size={15} className={up ? 'text-live' : 'text-muted'} />
        <span className="font-mono text-sm text-ink">{p.name}</span>
        {!p.present && <span className="ml-auto text-xs text-alarm">absent</span>}
        {p.present && (
          <span className="ml-auto flex items-center gap-2 text-xs">
            <span className={up ? 'text-live' : 'text-muted'}>{up ? 'link up' : 'no link'}</span>
            <button
              onClick={() => io.setSwitchPort(p.name, !p.admin_up)}
              title="Admin up/down (live)"
              className={`hair rounded border px-1.5 py-0.5 ${p.admin_up ? 'text-ink' : 'text-muted'} hover:border-accent/50`}>
              {p.admin_up ? 'on' : 'off'}
            </button>
          </span>
        )}
      </div>
      <div className="mt-2 grid grid-cols-2 gap-x-3 gap-y-0.5 text-xs text-muted">
        {up && p.speed != null && <div>speed <span className="text-ink">{p.speed} Mb/s {p.duplex ?? ''}</span></div>}
        {p.master && <div>bridge <span className="text-ink">{p.master}</span></div>}
        <div>rx <span className="text-ink">{fmtBytes(p.stats.rx_bytes)}</span></div>
        <div>tx <span className="text-ink">{fmtBytes(p.stats.tx_bytes)}</span></div>
        {(p.stats.rx_errors > 0 || p.stats.tx_errors > 0) &&
          <div className="text-warm">errors {p.stats.rx_errors}/{p.stats.tx_errors}</div>}
      </div>
    </div>
  );
}

function CustomSettings({
  cfg, names, patch, portCfg,
}: {
  cfg: SwitchConfig;
  names: string[];
  patch: (fn: (c: SwitchConfig) => void) => void;
  portCfg: (n: string) => { enabled: boolean; addressing?: Addressing; pvid?: number; tagged?: number[]; untagged?: number };
}) {
  const inBridge = (n: string) => cfg.bridge.members.includes(n);
  const toggleMember = (n: string) =>
    patch((c) => {
      const i = c.bridge.members.indexOf(n);
      if (i >= 0) c.bridge.members.splice(i, 1); else c.bridge.members.push(n);
    });

  return (
    <section className="hair rounded-xl border bg-panel p-4 space-y-3">
      <h2 className="text-sm font-semibold text-ink">VLANs & membership</h2>
      <Toggle label="802.1Q VLAN filtering"
        hint="Off is a plain bridge. On applies the per-port PVID / tagged VLANs below — the b53 offloads it in hardware."
        value={cfg.bridge.vlan_filtering}
        onChange={(v) => patch((c) => { c.bridge.vlan_filtering = v; })} />

      {names.map((n) => {
        const pc = portCfg(n);
        const member = inBridge(n);
        return (
          <div key={n} className="hair rounded-lg border bg-raised p-3 space-y-2">
            <div className="flex items-center gap-2">
              <Cable size={15} className="text-muted" />
              <span className="font-mono text-sm text-ink">{n}</span>
              <label className="ml-auto flex items-center gap-1.5 text-xs text-muted">
                <input type="checkbox" checked={member} onChange={() => toggleMember(n)} />
                in bridge
              </label>
            </div>
            {member && cfg.bridge.vlan_filtering && (
              <div className="grid gap-2 sm:grid-cols-2">
                <NumField label="PVID (untagged VLAN)" value={pc.pvid}
                  onChange={(v) => patch((c) => { (c.ports[n] ??= { enabled: true }).pvid = v; })} />
                <VidListField label="Tagged VLANs" value={pc.tagged ?? []}
                  onChange={(v) => patch((c) => { (c.ports[n] ??= { enabled: true }).tagged = v; })} />
              </div>
            )}
            {!member && (
              <div>
                <div className="mb-1 text-xs text-muted">Routed address (port not in the bridge)</div>
                <AddressingEditor value={pc.addressing ?? { mode: 'none' }}
                  onChange={(a) => patch((c) => { (c.ports[n] ??= { enabled: true }).addressing = a; })} />
              </div>
            )}
          </div>
        );
      })}

      <VlanList cfg={cfg} patch={patch} />

      <div>
        <div className="mb-1 text-xs text-muted">Bridge address</div>
        <AddressingEditor value={cfg.bridge.addressing}
          onChange={(a) => patch((c) => { c.bridge.addressing = a; })} />
      </div>
    </section>
  );
}

function VlanList({ cfg, patch }: { cfg: SwitchConfig; patch: (fn: (c: SwitchConfig) => void) => void }) {
  const [id, setId] = useState('');
  const [name, setName] = useState('');
  return (
    <div className="hair rounded-lg border bg-raised p-3">
      <div className="mb-2 text-xs text-muted">Named VLANs (for your own reference)</div>
      <div className="mb-2 flex flex-wrap gap-1.5">
        {cfg.vlans.length === 0 && <span className="text-xs text-muted">none</span>}
        {cfg.vlans.map((v) => (
          <span key={v.id} className="hair flex items-center gap-1 rounded border px-2 py-0.5 text-xs text-ink">
            {v.id}{v.name ? ` · ${v.name}` : ''}
            <button className="text-muted hover:text-alarm"
              onClick={() => patch((c) => { c.vlans = c.vlans.filter((x) => x.id !== v.id); })}>×</button>
          </span>
        ))}
      </div>
      <div className="flex gap-2">
        <input value={id} onChange={(e) => setId(e.target.value)} placeholder="id"
          className="hair w-20 rounded-lg border bg-panel p-1.5 text-sm text-ink outline-none focus:border-accent/50" />
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="name (optional)"
          className="hair min-w-0 flex-1 rounded-lg border bg-panel p-1.5 text-sm text-ink outline-none focus:border-accent/50" />
        <button
          onClick={() => {
            const n = parseInt(id, 10);
            if (!Number.isFinite(n) || n < 1 || n > 4094) return;
            patch((c) => { if (!c.vlans.some((x) => x.id === n)) c.vlans.push({ id: n, name: name || undefined }); });
            setId(''); setName('');
          }}
          className="hair rounded-lg border bg-panel px-3 text-sm text-ink hover:border-accent/50">add</button>
      </div>
    </div>
  );
}

function AddressingEditor({ value, onChange }: { value: Addressing; onChange: (a: Addressing) => void }) {
  const input = 'hair rounded-lg border bg-panel p-1.5 text-sm text-ink outline-none focus:border-accent/50';
  return (
    <div className="flex flex-wrap items-center gap-2">
      <select value={value.mode} className={input}
        onChange={(e) => {
          const m = e.target.value as Addressing['mode'];
          onChange(m === 'static' ? { mode: 'static', addr: '', gw: '' } : { mode: m });
        }}>
        <option value="dhcp">DHCP</option>
        <option value="static">Static</option>
        <option value="none">No address</option>
      </select>
      {value.mode === 'static' && (
        <>
          <input className={`${input} w-40`} placeholder="10.0.0.5/24" value={value.addr}
            onChange={(e) => onChange({ ...value, addr: e.target.value })} />
          <input className={`${input} w-40`} placeholder="gateway (optional)" value={value.gw ?? ''}
            onChange={(e) => onChange({ ...value, gw: e.target.value || undefined })} />
        </>
      )}
    </div>
  );
}

function ModeCard({ id, cur, onPick, title, desc }: {
  id: SwitchMode; cur: SwitchMode; onPick: (m: SwitchMode) => void; title: string; desc: string;
}) {
  const on = id === cur;
  return (
    <button onClick={() => onPick(id)}
      className={`hair rounded-lg border p-3 text-left transition ${on ? 'border-accent/60 bg-accent/10' : 'bg-raised hover:border-accent/40'}`}>
      <div className={`text-sm font-medium ${on ? 'text-ink' : 'text-ink'}`}>{title}</div>
      <div className="mt-0.5 text-xs text-muted">{desc}</div>
    </button>
  );
}

function Toggle({ label, hint, value, onChange }: {
  label: string; hint?: string; value: boolean; onChange: (v: boolean) => void;
}) {
  return (
    <label className="flex items-start gap-2">
      <input type="checkbox" checked={value} onChange={(e) => onChange(e.target.checked)} className="mt-0.5" />
      <span>
        <span className="text-sm text-ink">{label}</span>
        {hint && <span className="block text-xs text-muted">{hint}</span>}
      </span>
    </label>
  );
}

function NumField({ label, value, onChange }: { label: string; value?: number; onChange: (v: number | undefined) => void }) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs text-muted">{label}</span>
      <input type="number" min={1} max={4094} value={value ?? ''}
        onChange={(e) => onChange(e.target.value === '' ? undefined : parseInt(e.target.value, 10))}
        className="hair w-full rounded-lg border bg-panel p-1.5 text-sm text-ink outline-none focus:border-accent/50" />
    </label>
  );
}

function VidListField({ label, value, onChange }: { label: string; value: number[]; onChange: (v: number[]) => void }) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs text-muted">{label}</span>
      <input value={value.join(',')} placeholder="e.g. 10,20"
        onChange={(e) => onChange(
          e.target.value.split(',').map((s) => parseInt(s.trim(), 10)).filter((n) => Number.isFinite(n) && n >= 1 && n <= 4094),
        )}
        className="hair w-full rounded-lg border bg-panel p-1.5 text-sm text-ink outline-none focus:border-accent/50" />
    </label>
  );
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const u = ['KiB', 'MiB', 'GiB', 'TiB'];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(1)} ${u[i]}`;
}
