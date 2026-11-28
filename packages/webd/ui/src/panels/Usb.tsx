import { useEffect, useState } from 'react';
import { Bluetooth, Cable, ChevronRight, HardDrive, Keyboard, Network, Radio, Share2, Speaker, Usb, Waves } from 'lucide-react';
import { rest, type Capabilities, type UsbDevice, type UsbUse } from '../api';
import { StoragePanel } from './Storage';
import { UsbipSection } from './Usbip';

/* USB: what is in the ports, and what the box does with each. A drive is
   shared by storaged (its volumes and the share are below); one of the box's
   radios links to its page; anything else is listed for what it is. USB/IP
   sharing is the same devices seen from another machine. */
export function UsbPanel({ caps, go }: { caps: Capabilities; go: (id: string) => void }) {
  const [devices, setDevices] = useState<UsbDevice[] | null>(null);
  useEffect(() => {
    // Polls; a uevent stream from webd if 3 s ever feels slow.
    const load = () => rest.usb().then((s) => setDevices(s.devices)).catch(() => {});
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, []);
  return (
    <div className="space-y-4">
      <section className="hair rounded-xl border bg-panel p-4">
        <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
          <Usb size={15} className="text-muted" />
          Plugged in
        </h2>
        {devices === null ? (
          <p className="text-xs text-muted">Reading the ports…</p>
        ) : devices.length ? (
          <div className="space-y-2">
            {devices.map((d) => <DeviceRow key={d.busid} d={d} go={go} />)}
          </div>
        ) : (
          <p className="text-xs text-muted">Nothing plugged in.</p>
        )}
      </section>
      {caps.storage && <StoragePanel caps={caps} />}
      <UsbipSection />
    </div>
  );
}

const RADIO: Record<string, { label: string; icon: typeof Usb }> = {
  zigbee: { label: 'Zigbee', icon: Radio },
  zwave: { label: 'Z-Wave', icon: Waves },
  bluetooth: { label: 'Bluetooth', icon: Bluetooth },
};

function iconFor(u?: UsbUse): typeof Usb {
  switch (u?.kind) {
    case 'storage': return HardDrive;
    case 'radio': return RADIO[u.radio]?.icon ?? Radio;
    case 'serial': return Cable;
    case 'network': return Network;
    case 'bluetooth': return Bluetooth;
    case 'input': return Keyboard;
    case 'audio': return Speaker;
    default: return Usb;
  }
}

function describe(u: UsbUse): string {
  switch (u.kind) {
    case 'storage': return `Drive (${u.block}) · shared below`;
    case 'radio': return `This box's ${RADIO[u.radio]?.label ?? u.radio} radio`;
    case 'serial': return `Serial port /dev/${u.tty}`;
    case 'network': return `Network interface ${u.iface}`;
    case 'bluetooth': return `Bluetooth adapter ${u.hci}`;
    case 'input': return u.name || 'Input device';
    case 'audio': return `Sound card ${u.card}`;
  }
}

function DeviceRow({ d, go }: { d: UsbDevice; go: (id: string) => void }) {
  const Icon = iconFor(d.uses[0]);
  const r = d.uses.find((u) => u.kind === 'radio');
  const radio = r ?? (d.uses.some((u) => u.kind === 'bluetooth') ? { radio: 'bluetooth' } : undefined);
  const speed = Number(d.speed);
  return (
    <div className="hair flex items-center gap-3 rounded-xl border bg-raised p-3">
      <Icon size={18} className="shrink-0 text-muted" />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-sm">
          <span className="truncate">{d.product || d.class}</span>
          {d.exported && <span className="flex items-center gap-1 text-xs text-live"><Share2 size={11} /> shared over USB/IP</span>}
        </div>
        <div className="truncate text-xs text-muted">
          {[d.manufacturer, d.class, ...d.uses.map(describe)].filter(Boolean).join(' · ')}
        </div>
        <div className="truncate font-mono text-[11px] text-muted">
          port {d.busid} · {d.id}{speed ? ` · ${speed >= 1000 ? `${speed / 1000} Gb/s` : `${speed} Mb/s`}` : ''}
        </div>
      </div>
      {radio && (
        <button
          onClick={() => go(radio.radio)}
          className="hair flex shrink-0 items-center gap-1 rounded-lg border px-2.5 py-1 text-xs hover:border-accent/50"
        >
          Open {RADIO[radio.radio]?.label ?? radio.radio}
          <ChevronRight size={13} />
        </button>
      )}
    </div>
  );
}
