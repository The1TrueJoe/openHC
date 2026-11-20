import { useEffect, useState } from 'react';
import { Check, Usb } from 'lucide-react';
import { rest, type UsbipCfg } from '../api';
import { Toggle } from './Settings';
import { toastError } from '../toast';

/** USB/IP: share this box's USB devices with another machine over the network.
 *  Only on boards with the 'usbip' feature (`available`); off by default. Every
 *  change applies at once. The box's own radios never appear in the list. */
export function UsbipSection() {
  const [cfg, setCfg] = useState<UsbipCfg | null>(null);
  const [note, setNote] = useState<{ text: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    rest.usbip().then(setCfg).catch(() => setCfg(null));
  }, []);

  if (!cfg?.available) return null;

  async function apply(enabled: boolean, selected: string[]) {
    setSaving(true);
    setNote(null);
    try {
      const r = await rest.setUsbip({ enabled, selected });
      setCfg(r);
      setNote({ text: r.enabled ? `sharing ${r.selected.length} device(s) on port 3240` : 'off' });
    } catch (e) {
      toastError(e, 'Saving USB/IP');
    } finally {
      setSaving(false);
    }
  }

  const host = location.hostname;
  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <div className="mb-3 flex items-center gap-2">
        <Usb size={16} className="text-accent" />
        <h2 className="text-sm font-medium">USB/IP</h2>
      </div>
      <Toggle
        label="Share USB devices over the network"
        hint="Off by default. Lets another machine use a USB device plugged into this controller (a Zigbee or Z-Wave stick, say) as if it were plugged into that machine. There is no password: anything on your network can attach a shared device, and while it is attached this controller cannot use it."
        checked={cfg.enabled}
        disabled={saving}
        onChange={(v) => apply(v, cfg.selected)}
      />
      {cfg.enabled && (
        <div className="mt-3 space-y-2 border-l-2 pl-4 hair">
          {cfg.devices.length === 0 && (
            <div className="text-xs text-muted">No USB devices plugged in. This controller's own radios are never listed.</div>
          )}
          {cfg.devices.map((d) => (
            <Toggle
              key={d.busid}
              label={
                <>
                  {d.product || 'USB device'}
                  <span className="ml-2 font-mono text-xs text-muted">{d.busid} · {d.id}</span>
                  {d.exported && <span className="ml-2 text-xs text-live">shared</span>}
                </>
              }
              hint={d.manufacturer || undefined}
              checked={cfg.selected.includes(d.busid)}
              disabled={saving}
              onChange={(v) =>
                apply(true, v ? [...cfg.selected, d.busid] : cfg.selected.filter((b) => b !== d.busid))
              }
            />
          ))}
          <div className="rounded-lg shade p-3 font-mono text-xs text-muted">
            <div>usbip list -r {host}</div>
            <div className="text-ink">sudo usbip attach -r {host} -b &lt;busid&gt;</div>
          </div>
        </div>
      )}
      {note && (
        <div className="mt-3 flex items-center gap-1.5 text-xs text-live">
          <Check size={13} />
          {note.text}
        </div>
      )}
    </section>
  );
}
