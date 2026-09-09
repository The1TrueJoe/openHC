import { useEffect, useState } from 'react';
import { Check, Lock, AlertTriangle, Server, Share2 } from 'lucide-react';
import { rest, type ConfigDoc, type MqttWrite } from '../api';

/** Fields the environment has pinned cannot be edited here: a restart would
 *  discard the change, and a form that silently ignores you is worse than one
 *  that says why it will not take the edit. */
function Pinned() {
  return (
    <span className="ml-2 inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted shade">
      <Lock size={9} /> set in iod.conf
    </span>
  );
}

export function SettingsPanel() {
  const [doc, setDoc] = useState<ConfigDoc | null>(null);
  const [form, setForm] = useState<MqttWrite>({});
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    rest.config().then((d) => { setDoc(d); setForm(d.mqtt); }).catch((e) =>
      setNote({ ok: false, text: String(e.message ?? e) }));
  }, []);

  if (!doc) return <div className="text-muted">Reading settings…</div>;
  const pinned = (f: string) => doc.pinned.includes(f);
  const set = <K extends keyof MqttWrite>(k: K, v: MqttWrite[K]) => setForm((p) => ({ ...p, [k]: v }));

  async function save() {
    setSaving(true);
    setNote(null);
    try {
      // Only send what changed, so an untouched password field keeps the
      // stored secret rather than blanking it.
      const patch: MqttWrite = {};
      for (const [k, v] of Object.entries(form)) {
        if (k === 'password_set') continue;
        if (k === 'password' && !v) continue;
        if (!pinned(k) && v !== (doc!.mqtt as any)[k]) (patch as any)[k] = v;
      }
      if (Object.keys(patch).length === 0) {
        setNote({ ok: true, text: 'nothing changed' });
        return;
      }
      await rest.saveConfig(patch);
      const fresh = await rest.config();
      setDoc(fresh);
      setForm(fresh.mqtt);
      setNote({ ok: true, text: 'saved — the broker connection restarted' });
    } catch (e) {
      setNote({ ok: false, text: String((e as Error).message) });
    } finally {
      setSaving(false);
    }
  }

  const base = `${form.prefix ?? doc.mqtt.prefix}/${form.client_id ?? doc.mqtt.client_id}`;

  return (
    <div className="max-w-2xl space-y-6">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">Settings</h1>
        <p className="text-sm text-muted">
          IO control is MQTT. These are the same topics Home Assistant or a script would use.
        </p>
      </header>

      {/* Serving is what the config GUI itself talks to, so it is first and it
          is explicit about what turning it off costs. */}
      <section className="hair rounded-xl border bg-panel p-4">
        <div className="mb-3 flex items-center gap-2">
          <Server size={16} className="text-accent" />
          <h2 className="text-sm font-medium">This controller</h2>
        </div>
        <Toggle
          label="Serve MQTT from this controller"
          hint="The IO panels in this page talk to it. Turning it off leaves them with nothing to speak to."
          checked={form.serve ?? true}
          onChange={(v) => set('serve', v)}
        />
        <Field label="Plain MQTT port" hint="For things on the LAN that are not a browser. 0 disables it.">
          <input
            type="number"
            value={form.listen_port ?? 1883}
            onChange={(e) => set('listen_port', Number(e.target.value))}
            className="w-28 rounded-md border bg-raised px-2 py-1 text-sm hair outline-none focus:border-accent/50"
          />
        </Field>
      </section>

      <section className="hair rounded-xl border bg-panel p-4">
        <div className="mb-3 flex items-center gap-2">
          <Share2 size={16} className="text-accent" />
          <h2 className="text-sm font-medium">Another broker</h2>
        </div>
        <Toggle
          label={<>Also publish to an external broker {pinned('bridge') && <Pinned />}</>}
          hint="Independent of the above. This page keeps talking to the controller either way, so a broker that is down never costs you the settings screen."
          checked={form.bridge ?? false}
          disabled={pinned('bridge')}
          onChange={(v) => set('bridge', v)}
        />
        {(form.bridge ?? false) && (
          <div className="mt-3 space-y-3 border-l-2 pl-4 hair">
            <Field label={<>Broker URL {pinned('url') && <Pinned />}</>} hint="mqtt:// is plaintext, mqtts:// is TLS.">
              <Text value={form.url ?? ''} disabled={pinned('url')} placeholder="mqtt://10.0.0.5:1883"
                    onChange={(v) => set('url', v)} />
            </Field>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label={<>Username {pinned('username') && <Pinned />}</>}>
                <Text value={form.username ?? ''} disabled={pinned('username')} onChange={(v) => set('username', v)} />
              </Field>
              <Field
                label={<>Password {pinned('password') && <Pinned />}</>}
                hint={doc.mqtt.password_set ? 'A password is set. Leave blank to keep it.' : undefined}
              >
                <Text type="password" value={form.password ?? ''} disabled={pinned('password')}
                      placeholder={doc.mqtt.password_set ? '••••••••' : ''}
                      onChange={(v) => set('password', v)} />
              </Field>
            </div>
            <Field
              label={<>CA certificate {pinned('ca_path') && <Pinned />}</>}
              hint="Required for mqtts:// with a private or self-signed broker — this image ships no trust store, and iod refuses to connect rather than skipping verification."
            >
              <Text value={form.ca_path ?? ''} disabled={pinned('ca_path')}
                    placeholder="/etc/openhc/broker-ca.pem" onChange={(v) => set('ca_path', v)} />
            </Field>
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Client certificate" hint="Only if the broker wants mutual TLS.">
                <Text value={form.client_cert_path ?? ''} disabled={pinned('client_cert_path')}
                      onChange={(v) => set('client_cert_path', v)} />
              </Field>
              <Field label="Client key">
                <Text value={form.client_key_path ?? ''} disabled={pinned('client_key_path')}
                      onChange={(v) => set('client_key_path', v)} />
              </Field>
            </div>
          </div>
        )}
      </section>

      <section className="hair rounded-xl border bg-panel p-4">
        <h2 className="mb-3 text-sm font-medium">Topics</h2>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={<>Prefix {pinned('prefix') && <Pinned />}</>}>
            <Text value={form.prefix ?? ''} disabled={pinned('prefix')} onChange={(v) => set('prefix', v)} />
          </Field>
          <Field label={<>Controller id {pinned('client_id') && <Pinned />}</>} hint="Defaults to the hostname, which is already unique.">
            <Text value={form.client_id ?? ''} disabled={pinned('client_id')} onChange={(v) => set('client_id', v)} />
          </Field>
        </div>
        <div className="mt-3 rounded-lg shade p-3 font-mono text-xs text-muted">
          <div className="text-ink">{base}/state/relay/1</div>
          <div>{base}/cmd/relay/1/set &nbsp;←&nbsp; ON | OFF | TOGGLE</div>
          <div>{base}/event/ir/rx</div>
        </div>
        <Field
          label={<>Home Assistant discovery {pinned('discovery') && <Pinned />}</>}
          hint="Relays appear as switches and contacts as binary sensors, with no YAML. Empty disables it."
        >
          <Text value={form.discovery ?? ''} disabled={pinned('discovery')} placeholder="homeassistant"
                onChange={(v) => set('discovery', v)} />
        </Field>
      </section>

      <div className="flex items-center gap-3">
        <button
          onClick={save}
          disabled={saving}
          className="rounded-lg bg-accent/20 px-4 py-2 text-sm transition hover:bg-accent/30 disabled:opacity-50"
        >
          {saving ? 'Saving…' : 'Save'}
        </button>
        {note && (
          <span className={`flex items-center gap-1.5 text-xs ${note.ok ? 'text-live' : 'text-alarm'}`}>
            {note.ok ? <Check size={13} /> : <AlertTriangle size={13} />}
            {note.text}
          </span>
        )}
      </div>
    </div>
  );
}

function Field({ label, hint, children }: { label: React.ReactNode; hint?: string; children: React.ReactNode }) {
  return (
    <label className="mt-3 block">
      <div className="mb-1 flex items-center text-xs font-medium">{label}</div>
      {children}
      {hint && <div className="mt-1 text-xs text-muted">{hint}</div>}
    </label>
  );
}

function Text({
  value, onChange, disabled, placeholder, type = 'text',
}: {
  value: string; onChange: (v: string) => void; disabled?: boolean; placeholder?: string; type?: string;
}) {
  return (
    <input
      type={type}
      value={value}
      placeholder={placeholder}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value)}
      spellCheck={false}
      className="hair w-full rounded-md border bg-raised px-2 py-1.5 text-sm text-ink outline-none focus:border-accent/50 disabled:opacity-50"
    />
  );
}

function Toggle({
  label, hint, checked, onChange, disabled,
}: {
  label: React.ReactNode; hint?: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean;
}) {
  return (
    <label className="flex cursor-pointer items-start gap-3">
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5 h-4 w-4 accent-[var(--accent)] disabled:opacity-50"
      />
      <span>
        <span className="flex items-center text-sm">{label}</span>
        {hint && <span className="mt-0.5 block text-xs text-muted">{hint}</span>}
      </span>
    </label>
  );
}
