import { useEffect, useState } from 'react';
import { ApiReferenceReact } from '@scalar/api-reference-react';
import '@scalar/api-reference-react/style.css';

/* The two halves of the API, side by side, because "what can this box do" is one
   question and the answer is split across two protocols:

     REST  — configuration and telemetry (webd, iod, sysmond)
     MQTT  — IO control, where a relay closing is state to subscribe to rather
             than a request to make

   Both specs are served by webd from the same origin as this page, so the docs
   work on a controller with no internet — which is the normal case. */
export function DocsPanel() {
  const [tab, setTab] = useState<'rest' | 'mqtt'>('rest');
  return (
    <div className="space-y-4">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">API</h1>
        <p className="text-sm text-muted">
          Generated from the specs this box serves — {' '}
          <a className="text-accent hover:underline" href="/api/openapi.json">openapi.json</a>
          {' · '}
          <a className="text-accent hover:underline" href="/api/asyncapi.json">asyncapi.json</a>
        </p>
      </header>

      <div className="flex gap-2">
        {(['rest', 'mqtt'] as const).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`rounded-lg px-3 py-1.5 text-sm transition ${
              tab === t ? 'bg-accent/20 text-ink' : 'shade text-muted hover:text-ink'
            }`}
          >
            {t === 'rest' ? 'REST' : 'MQTT'}
          </button>
        ))}
      </div>

      <div className="hair overflow-hidden rounded-xl border bg-panel">
        {tab === 'rest' ? (
          <ApiReferenceReact configuration={{ url: '/api/openapi.json' }} />
        ) : (
          <AsyncApiWrap />
        )}
      </div>
    </div>
  );
}

/* Rendered here rather than with @asyncapi/react-component, which pulls the full
   AsyncAPI parser and with it avsc, node-fetch and Node's fs/stream/zlib — none
   of which belong in a bundle that ships inside a RAM rootfs, and which does not
   build for the browser without a pile of polyfills.
   The document is ours and its shape is simple, so this reads it directly. */
type Chan = { address: string; summary?: string; description?: string;
              messages?: Record<string, { $ref?: string }> };
type Spec = {
  info?: { title?: string; version?: string; description?: string };
  servers?: Record<string, { host?: string; protocol?: string; description?: string }>;
  channels?: Record<string, Chan>;
  components?: { messages?: Record<string, { title?: string; payload?: unknown }> };
};

function AsyncApiWrap() {
  const [spec, setSpec] = useState<Spec | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    fetch('/api/asyncapi.json')
      .then((r) => r.json())
      .then(setSpec)
      .catch((e) => setErr(String(e)));
  }, []);
  if (err) return <div className="p-6 text-sm text-alarm">Could not load asyncapi.json: {err}</div>;
  if (!spec) return <div className="p-6 text-sm text-muted">Loading…</div>;

  const chans = Object.entries(spec.channels ?? {});
  const group = (pred: (a: string) => boolean) => chans.filter(([, c]) => pred(c.address));
  const sections: [string, string, [string, Chan][]][] = [
    ['Commands', 'Publish these.', group((a) => a.includes('/cmd/'))],
    ['State', 'Retained — a late subscriber is told the truth immediately.', group((a) => a.includes('/state/'))],
    ['Events', 'Not retained — never replayed to whoever connects next.', group((a) => a.includes('/event/'))],
  ];
  const msgName = (c: Chan) => {
    const ref = Object.values(c.messages ?? {})[0]?.$ref ?? '';
    const key = ref.split('/').pop() ?? '';
    return spec.components?.messages?.[key]?.title ?? key;
  };

  return (
    <div className="space-y-6 p-5">
      <div>
        <h3 className="text-base font-semibold">{spec.info?.title}</h3>
        <p className="mt-1 whitespace-pre-line text-sm text-muted">{spec.info?.description}</p>
      </div>
      <div className="flex flex-wrap gap-3">
        {Object.entries(spec.servers ?? {}).map(([k, s]) => (
          <div key={k} className="hair rounded-lg border p-3 text-xs">
            <div className="font-medium text-ink">{k} · {s.protocol}</div>
            <code className="text-muted">{s.host}</code>
            {s.description && <div className="mt-1 max-w-sm text-muted">{s.description}</div>}
          </div>
        ))}
      </div>
      {sections.map(([title, blurb, list]) =>
        list.length ? (
          <div key={title}>
            <h4 className="text-sm font-medium">{title}</h4>
            <p className="mb-2 text-xs text-muted">{blurb}</p>
            <div className="space-y-2">
              {list.map(([id, c]) => (
                <div key={id} className="hair rounded-lg border p-3">
                  <div className="flex flex-wrap items-baseline gap-2">
                    <code className="font-mono text-xs text-accent">{c.address}</code>
                    <span className="text-[11px] text-muted">{msgName(c)}</span>
                  </div>
                  <div className="mt-1 text-sm text-ink">{c.summary}</div>
                  {c.description && <div className="mt-1 text-xs text-muted">{c.description}</div>}
                </div>
              ))}
            </div>
          </div>
        ) : null,
      )}
    </div>
  );
}
