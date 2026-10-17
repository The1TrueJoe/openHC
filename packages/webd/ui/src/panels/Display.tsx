import { useEffect, useState } from 'react';
import { Check, AlertTriangle, Monitor } from 'lucide-react';
import { rest, type DisplayCfg } from '../api';
import { Field, Text, Toggle } from './Settings';

/** The controller's own screen: the opt-in full-screen web view (S95ohc-display
 *  on each board that has one — openHC's webview on the EAs and the HC-800), so the tab
 *  only appears where the API reports it `available`. Off by default (the screen
 *  shows the openHC splash). Toggling applies at once, the URL has its own Apply;
 *  a URL or theme change on a running view is live, without a restart. Auto is
 *  dark from sunset to sunrise at the box's location, which webd works out from a
 *  time zone (prefilled from this browser's) or exact coordinates. */

/** This browser's IANA zone: the likeliest place the box is. */
const browserZone = () => Intl.DateTimeFormat().resolvedOptions().timeZone ?? '';
/** Every zone the browser knows, for the picker (ES2022 typings lack it). */
const allZones = (): string[] =>
  (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('timeZone') ?? [];

export function DisplayPanel() {
  const [cfg, setCfg] = useState<DisplayCfg | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);
  // Coordinates edit as text: half-typed numbers ("-7", "") are not numbers yet.
  const [lat, setLat] = useState('');
  const [lon, setLon] = useState('');

  const load = () =>
    rest
      .display()
      .then((d) => {
        setCfg(d);
        setLat(d.latitude != null ? String(d.latitude) : '');
        setLon(d.longitude != null ? String(d.longitude) : '');
      })
      .catch(() => setCfg({ enabled: false, url: 'http://localhost/', theme: 'auto' }));
  useEffect(() => {
    load();
  }, []);

  if (!cfg) return null;

  async function apply(next: DisplayCfg) {
    setSaving(true);
    setNote(null);
    const num = (v: string) => (v.trim() === '' ? null : Number(v));
    const body = {
      ...next,
      // Auto needs a place: default to this browser's zone the first time.
      timezone: next.timezone || (next.theme === 'auto' ? browserZone() : ''),
      latitude: num(lat),
      longitude: num(lon),
    };
    try {
      const r = await rest.setDisplay(body);
      await load(); // the resolved scheme and next sunrise/sunset come back from GET
      setNote({
        ok: true,
        text: r.enabled
          ? (r.applied ? 'on — the screen is showing this page' : 'saved — takes effect on the next boot')
          : 'off — the screen shows the splash',
      });
    } catch (e) {
      setNote({ ok: false, text: String((e as Error).message) });
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="max-w-2xl space-y-4">
      <section className="hair rounded-xl border bg-panel p-4">
        <div className="mb-3 flex items-center gap-2">
          <Monitor size={16} className="text-accent" />
          <h2 className="text-sm font-medium">On-screen display</h2>
        </div>
        <Toggle
          label="Show a web page on the controller's own screen"
          hint="Off by default: the screen shows the openHC splash. Turn this on to run a full-screen browser on the controller's video output — this dashboard, or any page you point it at."
          checked={cfg.enabled}
          disabled={saving}
          onChange={(v) => apply({ ...cfg, enabled: v })}
        />
        {cfg.enabled && (
          <div className="mt-3 space-y-3 border-l-2 pl-4 hair">
            <Field label="Page URL" hint="Defaults to this controller's own dashboard. Must be http:// or https://.">
              <Text
                value={cfg.url}
                placeholder="http://localhost/"
                onChange={(v) => setCfg({ ...cfg, url: v })}
              />
            </Field>
            <Field label="Day / night" hint="The screen's own dark mode: every page sees it like an OS setting, and a change applies at once. Auto is dark from sunset to sunrise.">
              <div className="flex gap-1.5">
                {(['auto', 'light', 'dark'] as const).map((t) => (
                  <button
                    key={t}
                    onClick={() => apply({ ...cfg, theme: t })}
                    disabled={saving}
                    className={`rounded-lg px-3 py-1.5 text-xs capitalize transition disabled:opacity-50 ${
                      cfg.theme === t ? 'bg-accent/30 text-ink' : 'shade text-muted hover:bg-accent/20'
                    }`}
                  >
                    {t}
                  </button>
                ))}
              </div>
            </Field>
            {cfg.theme === 'auto' && (
              <Field
                label="Location"
                hint="Where the sun is reckoned: the time zone's main city (usually within half an hour), or exact coordinates if you add them."
              >
                <div className="space-y-2">
                  <select
                    value={cfg.timezone || browserZone()}
                    onChange={(e) => setCfg({ ...cfg, timezone: e.target.value })}
                    disabled={saving}
                    className="hair w-full rounded-md border bg-raised px-2 py-1.5 text-sm text-ink outline-none focus:border-accent/50 disabled:opacity-50"
                  >
                    {Array.from(new Set([cfg.timezone || browserZone(), ...allZones()]))
                      .filter(Boolean)
                      .map((z) => (
                        <option key={z} value={z}>
                          {z}
                        </option>
                      ))}
                  </select>
                  <div className="flex gap-2">
                    <Text value={lat} onChange={setLat} placeholder="latitude (optional)" disabled={saving} />
                    <Text value={lon} onChange={setLon} placeholder="longitude (optional)" disabled={saving} />
                  </div>
                  <SunStatus cfg={cfg} />
                </div>
              </Field>
            )}
            <button
              onClick={() => apply(cfg)}
              disabled={saving}
              className="rounded-lg bg-accent/20 px-3 py-1.5 text-xs transition hover:bg-accent/30 disabled:opacity-50"
            >
              {saving ? 'Applying…' : 'Apply'}
            </button>
          </div>
        )}
        {note && (
          <div className={`mt-3 flex items-center gap-1.5 text-xs ${note.ok ? 'text-live' : 'text-alarm'}`}>
            {note.ok ? <Check size={13} /> : <AlertTriangle size={13} />}
            {note.text}
          </div>
        )}
      </section>
    </div>
  );
}

/** "Dark now · light at 07:13": what Auto is doing and when it next flips, in
 *  the box's own zone. Nothing to say until a location is saved. */
function SunStatus({ cfg }: { cfg: DisplayCfg }) {
  if (cfg.next_change == null) {
    return <div className="text-xs text-muted">{cfg.scheme ? `${cfg.scheme} now` : null}</div>;
  }
  let at: string;
  try {
    at = new Date(cfg.next_change * 1000).toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
      timeZone: cfg.timezone || undefined,
    });
  } catch {
    at = new Date(cfg.next_change * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  }
  const dark = cfg.scheme === 'dark';
  return (
    <div className="text-xs text-muted">
      {dark ? 'Dark' : 'Light'} now · {dark ? 'light at sunrise' : 'dark at sunset'}, {at}
    </div>
  );
}
