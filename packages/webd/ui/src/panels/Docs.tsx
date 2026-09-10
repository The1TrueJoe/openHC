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
/* Scalar, cut down to a reference.
   This box is not a place to try requests from: there is a live controller
   behind these endpoints, "Test Request" against /api/radios/{type}/reset is a
   real reset, and the client-library snippets are noise on a page whose reader
   is already holding a shell on the device. Search goes too — the whole
   surface is a few dozen operations in one sidebar.
   withDefaultFonts is off deliberately: a controller on a LAN cannot fetch
   webfonts, and leaving it on means a layout that waits for a font that never
   arrives. */
const SCALAR = {
  url: '/api/openapi.json',
  hideSearch: true,
  hiddenClients: true,
  hideClientButton: true,
  hideTestRequestButton: true,
  hideDownloadButton: true,
  hideDarkModeToggle: true,
  withDefaultFonts: false,
  showSidebar: true,
  defaultOpenAllTags: false,
  layout: 'modern',
} as const;

export function DocsPanel() {
  const [tab, setTab] = useState<'rest' | 'mqtt'>('rest');
  return (
    /* h-full + min-h-0 so the reference gets the whole pane and scrolls
       INSIDE itself. Without min-h-0 a flex child refuses to shrink below its
       content and the page grows a second scrollbar around a document that
       already has one. */
    <div className="flex h-full min-h-0 flex-col">
      <div className="hair flex shrink-0 items-center gap-2 border-b px-5 py-2.5">
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
        <span className="ml-auto text-xs text-muted">
          <a className="hover:text-ink" href="/api/openapi.json">openapi.json</a>
          {' · '}
          <a className="hover:text-ink" href="/api/asyncapi.json">asyncapi.json</a>
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {tab === 'rest' ? <ApiReferenceReact configuration={SCALAR} /> : <AsyncApiWrap />}
      </div>
    </div>
  );
}

/* Rendered here rather than with @asyncapi/react-component, which pulls the full
   AsyncAPI parser and with it avsc, node-fetch and Node's fs/stream/zlib — none
   of which belong in a bundle that ships inside a RAM rootfs, and which does not
   build for the browser without a pile of polyfills.
   The document is ours and its shape is simple, so this reads it directly. */
/* The specs are written in markdown because that is what the format says they
   are. Rendering the two constructs actually used — **bold** and `code` — is a
   dozen lines; pulling a markdown library for it would add more bundle than the
   whole AsyncAPI panel costs. Anything else degrades to plain text, which is
   the correct failure. */
function md(text: string) {
  return text.split(/\n\n+/).map((para, i) => (
    <p key={i} className="mb-2 last:mb-0">
      {para.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).map((bit, j) => {
        if (bit.startsWith('**') && bit.endsWith('**'))
          return <strong key={j} className="text-ink">{bit.slice(2, -2)}</strong>;
        if (bit.startsWith('`') && bit.endsWith('`'))
          return <code key={j} className="rounded bg-raised px-1 font-mono text-[0.9em]">{bit.slice(1, -1)}</code>;
        return <span key={j}>{bit}</span>;
      })}
    </p>
  ));
}

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
        <div className="mt-1 text-sm text-muted">{md(spec.info?.description ?? '')}</div>
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
                  {c.description && <div className="mt-1 text-xs text-muted">{md(c.description)}</div>}
                </div>
              ))}
            </div>
          </div>
        ) : null,
      )}
    </div>
  );
}
