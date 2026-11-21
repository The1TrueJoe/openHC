import { useState } from 'react';
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

      {/* Scalar reads AsyncAPI 3 as well as OpenAPI; keyed so switching
          remounts it on the other document. */}
      <div className="min-h-0 flex-1 overflow-auto">
        <ApiReferenceReact key={tab} configuration={{ ...SCALAR, url: tab === 'rest' ? '/api/openapi.json' : '/api/asyncapi.json' }} />
      </div>
    </div>
  );
}
