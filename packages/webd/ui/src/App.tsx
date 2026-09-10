import { useEffect, useState, useSyncExternalStore } from 'react';
import { Cpu, ToggleLeft, AlertTriangle, Settings, BookOpen } from 'lucide-react';
import { io, rest, type Capabilities, type IoState } from './api';
import { IoPanel } from './panels/Io';
import { OverviewPanel } from './panels/Overview';
import { SettingsPanel } from './panels/Settings';
import { DocsPanel } from './panels/Docs';

/** Subscribe a component to the mirrored state.
 *  `useSyncExternalStore` rather than a context + effect because the socket is
 *  the source of truth and React is only rendering it — there is no second copy
 *  of the relay states to keep in step. */
export function useIoState(): IoState {
  return useSyncExternalStore(
    (cb) => io.watch(cb),
    () => io.state,
  );
}

type Dest = {
  id: string; label: string; icon: typeof Cpu;
  render: (c: Capabilities) => React.ReactNode;
  /** Take the whole pane: no padding, no outer scroll. For a panel that
   *  manages its own viewport — the API reference scrolls internally, and an
   *  outer scrollbar on top of that is two scrollbars for one document. */
  full?: boolean;
};

/** The rail is built FROM THE CAPABILITIES, not from a fixed list.
 *  A board with no relays, contacts or IR shows no IO destination at all —
 *  the same rule the house UI uses for rooms: a service with nothing behind it
 *  does not appear. */
function destinations(c: Capabilities): Dest[] {
  const d: Dest[] = [{ id: 'overview', label: 'Overview', icon: Cpu, render: (c) => <OverviewPanel caps={c} /> }];
  // Serial lives on the IO page — it IS IO, and a rail entry for it implied
  // otherwise. The terminal itself opens as a window over that page.
  if (c.relays || c.contacts || c.ir || c.serials?.length) {
    d.push({ id: 'io', label: 'IO', icon: ToggleLeft, render: (c) => <IoPanel caps={c} /> });
  }
  // Always present: this is where you point the controller at a house broker,
  // and it must be reachable even when the IO side is not working.
  d.push({ id: 'docs', label: 'API', icon: BookOpen, render: () => <DocsPanel />, full: true });
  d.push({ id: 'settings', label: 'Settings', icon: Settings, render: () => <SettingsPanel /> });
  return d;
}

export default function App() {
  const [caps, setCaps] = useState<Capabilities | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [at, setAt] = useState('overview');

  useEffect(() => {
    // REST first, for two things MQTT cannot supply: the board's shape, and
    // the topic root to subscribe under. Only then does the IO client connect.
    Promise.all([rest.capabilities(), rest.config()])
      .then(([caps, cfg]) => {
        setCaps(caps);
        io.connect(cfg.topics.base);
      })
      .catch((e: unknown) => setErr(String((e as Error).message ?? e)));
  }, []);

  if (err) {
    return (
      <div className="flex h-full items-center justify-center p-8">
        <div className="max-w-md rounded-xl border border-alarm/30 bg-alarm/5 p-6">
          <div className="mb-2 flex items-center gap-2 text-alarm">
            <AlertTriangle size={18} />
            <h1 className="font-semibold">Cannot reach iod</h1>
          </div>
          <p className="text-sm text-muted">
            The IO server owns every port on this controller and answers on <code>:7070</code>. This
            page loaded, so webd is running — iod is not, or is not reachable.
          </p>
          <p className="mt-3 font-mono text-xs text-alarm/80">{err}</p>
        </div>
      </div>
    );
  }
  if (!caps) return <div className="p-8 text-muted">Reading the board…</div>;

  const dests = destinations(caps);
  const here = dests.find((d) => d.id === at) ?? dests[0];

  return (
    <div className="flex h-full">
      <nav className="hair flex w-16 shrink-0 flex-col items-center gap-1 border-r py-4 sm:w-52 sm:items-stretch sm:px-3">
        <div className="mb-4 px-2">
          <div className="text-sm font-semibold tracking-tight">openHC</div>
          <div className="truncate text-xs text-muted" title={caps.hostname}>
            {caps.hostname}
          </div>
        </div>
        {dests.map((d) => {
          const on = d.id === here.id;
          return (
            <button
              key={d.id}
              onClick={() => setAt(d.id)}
              className={`flex items-center gap-3 rounded-lg px-3 py-2 text-sm transition ${
                on ? 'bg-accent/15 text-ink' : 'text-muted hover:shade hover:text-ink'
              }`}
            >
              <d.icon size={17} className={on ? 'text-accent' : ''} />
              <span className="hidden sm:inline">{d.label}</span>
            </button>
          );
        })}
      </nav>
      <main
        className={
          here.full
            ? 'min-w-0 flex-1 overflow-hidden'
            : 'min-w-0 flex-1 overflow-auto p-5 sm:p-7'
        }
      >
        {here.render(caps)}
      </main>
    </div>
  );
}
