import { useEffect, useState } from 'react';
import { rest, type Capabilities, type McuInfo } from '../api';

export function OverviewPanel({ caps }: { caps: Capabilities }) {
  const [mcu, setMcu] = useState<McuInfo | null>(null);
  useEffect(() => {
    if (caps.backend === 'mcu' && caps.mcu_linked) rest.mcu().then(setMcu).catch(() => {});
  }, [caps]);

  return (
    <div className="space-y-6">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">{caps.board}</h1>
        <p className="text-sm text-muted">{caps.hostname}</p>
      </header>

      <section className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
        {caps.ir && <Stat label="IR outputs" value={caps.ir.total} sub={irSub(caps)} />}
        {caps.relays && <Stat label="Relays" value={caps.relays.count} />}
        {caps.contacts && <Stat label="Contacts" value={caps.contacts.count} />}
        {caps.serials?.length ? <Stat label="Serial ports" value={caps.serials.length} /> : null}
      </section>

      {mcu && (
        <section className="hair rounded-xl border bg-panel p-4">
          <h2 className="mb-3 text-sm font-medium">IO microcontroller</h2>
          <dl className="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-2">
            <Row k="Part" v={mcu.part} />
            <Row k="Firmware" v={mcu.version} />
            <Row k="Product" v={mcu.product} mono />
            <Row
              k="Link"
              v={`${mcu.baud} baud${mcu.measured_baud ? ` · reports ${mcu.measured_baud}` : ''}`}
            />
          </dl>
        </section>
      )}
    </div>
  );
}

/** Combo ports are a subset of the outputs, not extra ones — the same physical
 *  connectors as the user serial ports. Saying so here stops someone adding
 *  them to the total. */
function irSub(c: Capabilities) {
  const bits: string[] = [];
  if (c.ir?.blaster) bits.push(`${c.ir.blaster} blaster`);
  if (c.ir?.combo) bits.push(`${c.ir.combo} shared with serial`);
  if (c.ir?.receiver) bits.push(`${c.ir.receiver} receiver`);
  return bits.join(' · ');
}

function Stat({ label, value, sub }: { label: string; value: number; sub?: string }) {
  return (
    <div className="hair rounded-xl border bg-panel p-4">
      <div className="text-2xl font-semibold tabular-nums">{value}</div>
      <div className="text-sm text-ink/80">{label}</div>
      {sub && <div className="mt-1 text-xs text-muted">{sub}</div>}
    </div>
  );
}

function Row({ k, v, mono }: { k: string; v: string; mono?: boolean }) {
  return (
    <div className="hair flex justify-between gap-4 border-b pb-1">
      <dt className="text-muted">{k}</dt>
      <dd className={`truncate text-right ${mono ? 'font-mono text-xs' : ''}`} title={v}>{v}</dd>
    </div>
  );
}
