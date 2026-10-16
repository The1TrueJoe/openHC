import { useEffect, useState } from 'react';
import { Check, AlertTriangle, Monitor } from 'lucide-react';
import { rest, type DisplayCfg } from '../api';
import { Field, Text, Toggle } from './Settings';

/** The controller's own screen: the opt-in full-screen web view (S95ohc-display
 *  on each board that has one — openHC's webview on the EAs and the HC-800), so the tab
 *  only appears where the API reports it `available`. Off by default (the screen
 *  shows the openHC splash). Toggling applies at once, the URL has its own Apply;
 *  a URL or theme change on a running view is live, without a restart. */
export function DisplayPanel() {
  const [cfg, setCfg] = useState<DisplayCfg | null>(null);
  const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    rest.display().then(setCfg).catch(() => setCfg({ enabled: false, url: 'http://localhost/', theme: 'auto' }));
  }, []);

  if (!cfg) return null;

  async function apply(next: DisplayCfg) {
    setSaving(true);
    setNote(null);
    try {
      const r = await rest.setDisplay(next);
      setCfg({ ...next, enabled: r.enabled, url: r.url, theme: r.theme });
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
            <Field label="Day / night" hint="The screen's own dark mode: every page sees it like an OS setting, and a change applies at once. Auto leaves pages at their default.">
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
            <button
              onClick={() => apply(cfg)}
              disabled={saving}
              className="rounded-lg bg-accent/20 px-3 py-1.5 text-xs transition hover:bg-accent/30 disabled:opacity-50"
            >
              {saving ? 'Applying…' : 'Apply URL'}
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
