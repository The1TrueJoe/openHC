import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ReactFlow, Background, Controls, Handle, Panel, Position, useNodesState, useReactFlow,
  type Connection, type Edge, type EdgeChange, type Node, type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {
  Music, Radio, Cable, Library, Speaker, Bell, Volume2, X, Plus, Trash2, Unplug, ChevronRight, RotateCcw,
  Play, Pause, SkipBack, SkipForward, Square, Folder, FileAudio, Search, Shuffle, Repeat, RefreshCw,
} from 'lucide-react';
import {
  io, rest, audioCover, type AudioEndpoint, type AudioEndpoints, type AudioMap, type AudioMeta, type IoState,
  type LibraryEntry, type LibraryState,
} from '../api';

/* The audio switcher: the whole Audio page on boards with named outputs.

   Sources (Spotify Connect and AirPlay endpoints, the library player, the
   hardware inputs) on the left, outputs on the right, and the routing is the
   lines between them — drag from a source to an output to connect it, select
   a line and press Delete (or use its panel) to disconnect. A Spotify, AirPlay
   or library source plays to one output, so a new line replaces its old one;
   an input can feed any number of outputs. Click any node for its settings:
   an output's level, tone and announcements; a source's name, what it is
   playing, and for the library its player and browser.

   Routing is configuration (REST PUT /audio/api/audio/endpoints — audiod
   restarts only what changed, so other outputs keep playing); everything live
   (levels, tone, now playing, which lines carry audio) is MQTT. Node positions
   are this browser's own (localStorage). The source kinds are a list, so new
   ones (network streams, more inputs) slot in as more node kinds. */

type Live = NonNullable<IoState['audio']>;
type Kind = 'spotify' | 'airplay' | 'library' | 'input';
const KIND_ICON = { spotify: Music, airplay: Radio, library: Library, input: Cable } as const;
const KIND_LABEL = { spotify: 'Spotify Connect', airplay: 'AirPlay', library: 'Library player', input: 'Input' } as const;

type SourceData = {
  kind: Kind;
  index: number;          // position in its endpoint list (or the input id's index)
  name: string;
  tag?: string;           // instance tag, for now-playing
  running: boolean;
  playing: boolean;
  meta?: AudioMeta;
  connected: boolean;
};
type OutputData = { id: string; label: string; level?: number; announcing: boolean; feeding: number };
type XY = { x: number; y: number };

const POS_KEY = 'ohc.audioflow.positions';
const loadPos = (): Record<string, XY> => {
  try { return JSON.parse(localStorage.getItem(POS_KEY) ?? '{}'); } catch { return {}; }
};
const savePos = (p: Record<string, XY>) => {
  try { localStorage.setItem(POS_KEY, JSON.stringify(p)); } catch { /* private window */ }
};

const srcId = (kind: Kind, index: number | string) => `src:${kind}:${index}`;
const outId = (id: string) => `out:${id}`;

const endpointsOf = (map: AudioMap): AudioEndpoints => ({
  spotify: map.spotify, airplay: map.airplay, routes: map.routes, library: map.library ?? [],
});

/* ── nodes ────────────────────────────────────────────────────────────────── */

function SourceNode({ data, selected }: NodeProps<Node<SourceData>>) {
  const Icon = KIND_ICON[data.kind];
  const m = data.meta;
  const track = m?.title ? [m.title, m.artist].filter(Boolean).join(' — ') : '';
  const [bad, setBad] = useState<string | null>(null);
  const cover = m?.cover && m.cover !== bad ? m.cover : null;
  return (
    <div className={`hair flex w-64 items-center gap-2.5 rounded-xl border bg-panel px-2.5 py-2 text-xs shadow-sm ${selected ? 'border-accent ring-1 ring-accent/40' : ''} ${data.running || data.kind === 'input' ? '' : 'opacity-60'}`}>
      {cover ? (
        <img src={audioCover(cover)} alt="" onError={() => setBad(cover)} className="size-9 shrink-0 rounded-md object-cover" />
      ) : (
        <div className="hair flex size-9 shrink-0 items-center justify-center rounded-md border bg-raised">
          <Icon size={15} className={data.playing ? 'text-live' : 'text-muted'} />
        </div>
      )}
      <div className="min-w-0 flex-1 leading-tight">
        <div className="truncate text-sm text-ink">{data.name}</div>
        <div className="truncate text-muted">
          {track || (data.playing ? 'playing' : data.kind === 'input' ? 'input' : !data.connected ? 'not connected' : data.running ? KIND_LABEL[data.kind] : 'not running')}
        </div>
      </div>
      {data.playing && <span className="size-2 shrink-0 rounded-full bg-live" title="playing" />}
      <Handle type="source" position={Position.Right} className="!size-3 !border-2 !border-[var(--panel)] !bg-[var(--accent)]" />
    </div>
  );
}

function OutputNode({ data, selected }: NodeProps<Node<OutputData>>) {
  return (
    <div className={`hair w-56 rounded-xl border bg-panel px-3 py-2.5 shadow-sm ${selected ? 'border-accent ring-1 ring-accent/40' : ''} ${data.announcing ? 'border-warm/70' : ''}`}>
      <Handle type="target" position={Position.Left} className="!size-3 !border-2 !border-[var(--panel)] !bg-[var(--accent)]" />
      <div className="flex items-center gap-2 text-sm">
        <Speaker size={15} className="shrink-0 text-muted" />
        <span className="truncate">{data.label}</span>
        {data.announcing && <Bell size={13} className="ml-auto shrink-0 text-warm" />}
      </div>
      <div className="mt-2 flex items-center gap-2">
        <Volume2 size={12} className="shrink-0 text-muted" />
        <div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-edge">
          <div className="h-full rounded-full bg-accent" style={{ width: `${data.level ?? 0}%` }} />
        </div>
        <span className="w-9 shrink-0 text-right text-xs tabular-nums text-muted">{data.level === undefined ? '–' : `${data.level}%`}</span>
      </div>
    </div>
  );
}

// Not `input`/`output`: those are React Flow's built-in node types, and their
// default styling (a box behind the node) applies by class name.
/* Re-frame the chart when the side panel opens or closes (the canvas changes
   width under it). Rendered inside <ReactFlow>, whose context it uses. */
function Refit({ when }: { when: boolean }) {
  const { fitView } = useReactFlow();
  useEffect(() => {
    const t = setTimeout(() => fitView({ padding: 0.15, duration: 200 }), 50);
    return () => clearTimeout(t);
  }, [when, fitView]);
  return null;
}

const nodeTypes = { ohcSource: SourceNode, ohcOutput: OutputNode };

/* ── the page ─────────────────────────────────────────────────────────────── */

export function AudioFlow({ map, live }: { map: AudioMap; live: Live }) {
  // Edits show at once; the next map from MQTT replaces this.
  const [pending, setPending] = useState<AudioEndpoints | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const mapKey = JSON.stringify(endpointsOf(map));
  useEffect(() => setPending(null), [mapKey]);
  const eps = pending ?? endpointsOf(map);

  const save = useCallback(async (next: AudioEndpoints) => {
    setPending(next);
    setErr(null);
    try {
      await rest.saveEndpoints(next);
    } catch (e) {
      setPending(null);
      setErr(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const inst = (kind: string, name: string | null, output: string) =>
    map.instances.find((i) => i.kind === kind && (i.name ?? i.input) === name && i.output === output);

  // What to draw, from the map and the live state.
  const sources: Array<{ id: string; data: SourceData }> = [];
  (['spotify', 'airplay', 'library'] as const).forEach((kind) =>
    (eps[kind] ?? []).forEach((e, n) => {
      const i = e.output ? inst(kind, e.name, e.output) : undefined;
      sources.push({ id: srcId(kind, n), data: {
        kind, index: n, name: e.name, tag: i?.tag, running: !!i?.running, playing: !!i?.running && !!i?.playing,
        meta: i?.tag ? live.meta?.[i.tag] : undefined, connected: !!e.output,
      } });
    }));
  map.inputs.forEach((p, n) => {
    const routes = eps.routes.filter((r) => r.input === p.id);
    sources.push({ id: srcId('input', p.id), data: {
      kind: 'input', index: n, name: p.label, running: true,
      playing: routes.some((r) => inst('route', p.id, r.output)?.running), connected: routes.length > 0,
    } });
  });
  const feeding = (o: string) =>
    (['spotify', 'airplay', 'library'] as const).reduce((a, k) => a + (eps[k] ?? []).filter((e) => e.output === o).length, 0) +
    eps.routes.filter((r) => r.output === o).length;
  const outputs = map.outputs.map((o) => ({ id: outId(o.id), data: {
    id: o.id, label: o.label, level: live.level?.[o.id], announcing: !!live.announcing?.[o.id], feeding: feeding(o.id),
  } as OutputData }));

  // Default layout: sources in a column, outputs in a column beside them,
  // centred on each other. Dragged positions win.
  const ROW_S = 72, ROW_O = 96, X_OUT = 460;
  const hS = sources.length * ROW_S, hO = outputs.length * ROW_O, h = Math.max(hS, hO);
  const derived: Node[] = [
    ...sources.map((s, i) => ({ id: s.id, type: 'ohcSource', position: { x: 0, y: (h - hS) / 2 + i * ROW_S }, data: s.data, deletable: false })),
    ...outputs.map((o, i) => ({ id: o.id, type: 'ohcOutput', position: { x: X_OUT, y: (h - hO) / 2 + i * ROW_O }, data: o.data, deletable: false })),
  ];
  const derivedKey = JSON.stringify(derived.map((d) => [d.id, d.data]));

  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const pos = useRef<Record<string, XY>>(loadPos());
  useEffect(() => {
    setNodes((prev) =>
      derived.map((d) => {
        const p = prev.find((x) => x.id === d.id);
        return p ? { ...p, type: d.type, data: d.data } : { ...d, position: pos.current[d.id] ?? d.position };
      }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [derivedKey]);

  const edges: Edge[] = [];
  (['spotify', 'airplay', 'library'] as const).forEach((kind) =>
    (eps[kind] ?? []).forEach((e, n) => {
      if (!e.output) return;
      const playing = sources.find((s) => s.id === srcId(kind, n))?.data.playing;
      const id = `edge:${kind}:${n}`;
      edges.push({ id, source: srcId(kind, n), target: outId(e.output), animated: !!playing, selected: sel === id,
        style: { stroke: playing ? 'var(--live)' : 'var(--muted)', strokeWidth: playing ? 2.5 : 1.5, opacity: playing ? 1 : 0.6 } });
    }));
  eps.routes.forEach((r, n) => {
    const playing = !!inst('route', r.input, r.output)?.running;
    const id = `edge:route:${n}`;
    edges.push({ id, source: srcId('input', r.input), target: outId(r.output), animated: playing, selected: sel === id,
      style: { stroke: playing ? 'var(--live)' : 'var(--muted)', strokeWidth: playing ? 2.5 : 1.5 } });
  });

  // Routing edits.
  const connect = (source: string, target: string) => {
    if (!target.startsWith('out:') || !source.startsWith('src:')) return;
    const out = target.slice(4);
    const [, kind, idx] = source.split(':');
    const next: AudioEndpoints = JSON.parse(JSON.stringify(eps));
    if (kind === 'input') {
      if (next.routes.some((r) => r.input === idx && r.output === out)) return;
      next.routes.push({ input: idx, output: out });
    } else {
      const list = next[kind as 'spotify' | 'airplay' | 'library'] as AudioEndpoint[];
      list[Number(idx)].output = out; // one output per endpoint: replaces
    }
    save(next);
  };
  const disconnect = (edgeId: string) => {
    const [, kind, idx] = edgeId.split(':');
    const next: AudioEndpoints = JSON.parse(JSON.stringify(eps));
    if (kind === 'route') next.routes.splice(Number(idx), 1);
    else (next[kind as 'spotify' | 'airplay' | 'library'] as AudioEndpoint[])[Number(idx)].output = '';
    setSel(null);
    save(next);
  };
  const addSource = (kind: 'spotify' | 'airplay' | 'library') => {
    const next: AudioEndpoints = JSON.parse(JSON.stringify(eps));
    const list = (next[kind] ??= []) as AudioEndpoint[];
    const base = kind === 'library' ? 'Library' : KIND_LABEL[kind];
    let name = base, n = 2;
    while (list.some((e) => e.name === name)) name = `${base} ${n++}`;
    list.push({ name, output: '' });
    save(next);
    setSel(srcId(kind, list.length - 1));
  };

  const onEdgesChange = (changes: EdgeChange[]) => {
    for (const c of changes) {
      if (c.type === 'remove') disconnect(c.id);
      if (c.type === 'select' && c.selected) setSel(c.id);
    }
  };

  const selected = sel?.startsWith('edge:') ? null : sel;
  const selSource = sources.find((s) => s.id === selected);
  const selOutput = map.outputs.find((o) => outId(o.id) === selected);
  const selEdge = sel?.startsWith('edge:') ? edges.find((e) => e.id === sel) : undefined;
  const nameOf = (nodeId: string) =>
    sources.find((s) => s.id === nodeId)?.data.name ?? map.outputs.find((o) => outId(o.id) === nodeId)?.label ?? nodeId;
  const hasLibrary = (eps.library ?? []).length > 0;

  return (
    <div className="flex h-full min-h-0">
      <div className="relative min-w-0 flex-1">
        <ReactFlow
          nodes={nodes.map((n) => ({ ...n, selected: n.id === sel }))}
          edges={edges}
          nodeTypes={nodeTypes}
          onNodesChange={(chs) => {
            onNodesChange(chs.filter((c) => c.type !== 'remove' && c.type !== 'select'));
            for (const c of chs) {
              if (c.type === 'position' && c.position && !c.dragging) {
                pos.current = { ...pos.current, [c.id]: c.position };
                savePos(pos.current);
              }
            }
          }}
          onEdgesChange={onEdgesChange}
          onConnect={(c: Connection) => connect(c.source, c.target)}
          isValidConnection={(c) => c.source.startsWith('src:') && c.target.startsWith('out:')}
          onNodeClick={(_, n) => setSel(n.id)}
          onEdgeClick={(_, e) => setSel(e.id)}
          onPaneClick={() => setSel(null)}
          colorMode="system"
          style={{ background: 'var(--ground)' }}
          fitView
          fitViewOptions={{ padding: 0.15 }}
          proOptions={{ hideAttribution: true }}
          deleteKeyCode={['Backspace', 'Delete']}
        >
          <Refit when={!!(selSource || selOutput || selEdge)} />
          <Background gap={20} size={1} />
          <Controls showInteractive={false} />
          <Panel position="top-left">
            <AddSource onAdd={addSource} hasLibrary={hasLibrary} />
          </Panel>
          {err && (
            <Panel position="bottom-center">
              <div className="rounded-lg border border-alarm/40 bg-panel px-3 py-1.5 text-xs text-alarm">{err}</div>
            </Panel>
          )}
        </ReactFlow>
      </div>

      {(selSource || selOutput || selEdge) && (
        <aside className="hair absolute inset-y-0 right-0 z-10 w-full max-w-sm overflow-y-auto border-l bg-panel p-4 shadow-xl sm:static sm:w-96 sm:shadow-none">
          <button onClick={() => setSel(null)} className="float-right rounded-md p-1 text-muted hover:text-ink" aria-label="Close">
            <X size={16} />
          </button>
          {selSource && (
            <SourceInspector key={selSource.id} s={selSource.data} eps={eps} map={map} live={live}
              onSave={save} onClose={() => setSel(null)} />
          )}
          {selOutput && (
            <OutputInspector key={selOutput.id} id={selOutput.id} label={selOutput.label} device={selOutput.device} live={live}
              feeding={sources.filter((s) => edges.some((e) => e.source === s.id && e.target === outId(selOutput.id))).map((s) => s.data)} />
          )}
          {selEdge && (
            <div className="space-y-3">
              <h2 className="text-sm font-medium">Connection</h2>
              <p className="text-sm">
                {nameOf(selEdge.source)} <ChevronRight size={13} className="inline text-muted" /> {nameOf(selEdge.target)}
              </p>
              <button onClick={() => disconnect(selEdge.id)}
                className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-3 py-1.5 text-xs hover:border-alarm/60 hover:text-alarm">
                <Unplug size={13} /> Disconnect
              </button>
              <p className="text-xs text-muted">Or select the line and press Delete.</p>
            </div>
          )}
        </aside>
      )}
    </div>
  );
}

function AddSource({ onAdd, hasLibrary }: { onAdd: (k: 'spotify' | 'airplay' | 'library') => void; hasLibrary: boolean }) {
  const [open, setOpen] = useState(false);
  const item = 'flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-xs hover:bg-raised disabled:opacity-40';
  return (
    <div className="relative">
      <button onClick={() => setOpen(!open)}
        className="hair flex items-center gap-1.5 rounded-lg border bg-panel px-3 py-1.5 text-xs shadow-sm hover:border-accent/50">
        <Plus size={13} /> Add source
      </button>
      {open && (
        <div className="hair absolute left-0 top-9 z-10 w-52 rounded-lg border bg-panel p-1 shadow-lg">
          <button className={item} onClick={() => { onAdd('spotify'); setOpen(false); }}><Music size={13} /> Spotify Connect</button>
          <button className={item} onClick={() => { onAdd('airplay'); setOpen(false); }}><Radio size={13} /> AirPlay</button>
          <button className={item} disabled={hasLibrary} title={hasLibrary ? 'One library player per box' : undefined}
            onClick={() => { onAdd('library'); setOpen(false); }}><Library size={13} /> Library player</button>
        </div>
      )}
    </div>
  );
}

/* ── inspectors ───────────────────────────────────────────────────────────── */

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs text-muted">{label}</span>
      {children}
    </label>
  );
}
const inputCls = 'hair w-full rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50';

function SourceInspector({
  s, eps, map, live, onSave, onClose,
}: {
  s: SourceData;
  eps: AudioEndpoints;
  map: AudioMap;
  live: Live;
  onSave: (e: AudioEndpoints) => void;
  onClose: () => void;
}) {
  const Icon = KIND_ICON[s.kind];
  const [name, setName] = useState(s.name);
  const m = s.meta;
  const clone = (): AudioEndpoints => JSON.parse(JSON.stringify(eps));
  const list = (e: AudioEndpoints) => (s.kind === 'input' ? null : (e[s.kind] ?? []) as AudioEndpoint[]);
  const ep = list(eps)?.[s.index];

  const rename = () => {
    const n = name.trim();
    if (!n || n === s.name || s.kind === 'input') return;
    const next = clone();
    list(next)![s.index].name = n;
    onSave(next);
  };

  if (s.kind === 'input') {
    const id = map.inputs[s.index]?.id;
    const routes = eps.routes.map((r, n) => ({ ...r, n })).filter((r) => r.input === id);
    return (
      <div className="space-y-4">
        <h2 className="flex items-center gap-2 text-sm font-medium"><Icon size={15} className="text-muted" /> {s.name}</h2>
        <p className="text-xs text-muted">A hardware input. Drag it to any number of outputs to play it there live.</p>
        <div className="space-y-1.5">
          {routes.map((r) => (
            <div key={r.n} className="hair flex items-center gap-2 rounded-lg border bg-raised px-2.5 py-1.5 text-sm">
              <ChevronRight size={13} className="text-muted" />
              {map.outputs.find((o) => o.id === r.output)?.label ?? r.output}
              <button className="ml-auto rounded-md p-1 text-muted hover:text-alarm" title="Disconnect"
                onClick={() => { const next = clone(); next.routes.splice(r.n, 1); onSave(next); }}><Unplug size={13} /></button>
            </div>
          ))}
          {!routes.length && <p className="text-xs text-muted">Not playing anywhere.</p>}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-4">
      <h2 className="flex items-center gap-2 text-sm font-medium">
        <Icon size={15} className="text-muted" /> {KIND_LABEL[s.kind]}
        <span className={`ml-2 text-xs font-normal ${s.playing ? 'text-live' : 'text-muted'}`}>
          {s.playing ? 'playing' : !s.connected ? 'not connected' : s.running ? 'idle' : 'not running'}
        </span>
      </h2>

      {m?.title && (
        <div className="hair flex gap-3 rounded-xl border bg-raised p-2.5">
          {m.cover && <img src={audioCover(m.cover)} alt="" className="size-14 shrink-0 rounded-lg object-cover" />}
          <div className="min-w-0 text-sm leading-tight">
            <div className="truncate">{m.title}</div>
            {m.artist && <div className="truncate text-xs text-muted">{m.artist}</div>}
            {m.album && <div className="truncate text-xs text-muted">{m.album}</div>}
            {m.client && <div className="mt-1 truncate text-xs text-muted">from {m.client}</div>}
          </div>
        </div>
      )}

      <Field label={s.kind === 'spotify' ? 'Name in the Spotify app' : s.kind === 'airplay' ? 'Name on Apple devices' : 'Name'}>
        <input value={name} onChange={(e) => setName(e.target.value)} onBlur={rename} maxLength={64}
          onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()} className={inputCls} />
      </Field>
      <Field label="Plays on">
        <select value={ep?.output ?? ''} className={inputCls}
          onChange={(e) => { const next = clone(); list(next)![s.index].output = e.target.value; onSave(next); }}>
          <option value="">Not connected</option>
          {map.outputs.map((o) => <option key={o.id} value={o.id}>{o.label}</option>)}
        </select>
      </Field>

      {s.kind === 'library' && <LibraryPlayer state={live.library} meta={m} />}

      <button onClick={() => {
        if (!confirm(`Remove "${s.name}"?`)) return;
        const next = clone(); list(next)!.splice(s.index, 1); onSave(next); onClose();
      }}
        className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-3 py-1.5 text-xs hover:border-alarm/60 hover:text-alarm">
        <Trash2 size={13} /> Remove source
      </button>
    </div>
  );
}

function OutputInspector({
  id, label, device, live, feeding,
}: {
  id: string;
  label: string;
  device: string;
  live: Live;
  feeding: SourceData[];
}) {
  const level = live.level?.[id];
  const hasTone = live.bass !== undefined;
  const tone = { bass: live.bass?.[id] ?? 0, treble: live.treble?.[id] ?? 0, balance: live.balance?.[id] ?? 0 };
  const flat = tone.bass === 0 && tone.treble === 0 && tone.balance === 0;
  const db = (v: number) => (v > 0 ? `+${v} dB` : `${v} dB`);
  const bal = (v: number) => (v === 0 ? 'centre' : v < 0 ? `L ${-v}` : `R ${v}`);
  return (
    <div className="space-y-4">
      <h2 className="flex items-center gap-2 text-sm font-medium" title={device}>
        <Speaker size={15} className="text-muted" /> {label}
        {live.announcing?.[id] && <span className="text-xs font-normal text-warm">announcing</span>}
      </h2>

      <div>
        <div className="mb-1 text-xs text-muted">Level</div>
        <LiveSlider value={level ?? 0} min={0} max={100} disabled={level === undefined} label={`${label} level`}
          fmt={(v) => `${v}%`} send={(v) => io.setAudioLevel(id, v)} />
        <p className="mt-1 text-xs text-muted">The AirPlay and Spotify volume sliders move this too.</p>
      </div>

      {hasTone && (
        <div>
          <div className="mb-1 flex items-center text-xs text-muted">
            Tone
            <button disabled={flat} title="Flat and centred" aria-label="Reset tone"
              onClick={() => { try { (['bass', 'treble', 'balance'] as const).forEach((k) => io.setTone(id, k, 0)); } catch { /* not connected */ } }}
              className="ml-auto rounded-md p-1 text-muted hover:text-ink disabled:opacity-30"><RotateCcw size={12} /></button>
          </div>
          <div className="space-y-1.5">
            <ToneSlider id={id} knob="bass" label="Bass" value={tone.bass} min={-12} max={12} fmt={db} />
            <ToneSlider id={id} knob="treble" label="Treble" value={tone.treble} min={-12} max={12} fmt={db} />
            <ToneSlider id={id} knob="balance" label="Balance" value={tone.balance} min={-100} max={100} fmt={bal} />
          </div>
        </div>
      )}

      <button onClick={() => { try { io.announce(id); } catch { /* not connected */ } }}
        className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-3 py-1.5 text-xs hover:border-accent/50">
        <Bell size={13} /> Play a chime (ducks the music)
      </button>

      <div>
        <div className="mb-1 text-xs text-muted">Playing here</div>
        {feeding.length ? (
          <ul className="space-y-1 text-sm">
            {feeding.map((f) => {
              const Icon = KIND_ICON[f.kind];
              return (
                <li key={`${f.kind}:${f.index}`} className="flex items-center gap-2">
                  <Icon size={13} className={f.playing ? 'shrink-0 text-live' : 'shrink-0 text-muted'} />
                  <span className="truncate">{f.name}</span>
                  <span className="ml-auto shrink-0 text-xs text-muted">{KIND_LABEL[f.kind]}</span>
                </li>
              );
            })}
          </ul>
        ) : (
          <p className="text-xs text-muted">Nothing connected — drag a source here.</p>
        )}
      </div>
    </div>
  );
}

/* A slider that follows the thumb while dragging, sends (throttled) as it
   moves and once on release, and otherwise shows the live value. */
function LiveSlider({
  value, min, max, disabled, label, fmt, send,
}: {
  value: number;
  min: number;
  max: number;
  disabled?: boolean;
  label: string;
  fmt: (v: number) => string;
  send: (v: number) => void;
}) {
  const [drag, setDrag] = useState<number | null>(null);
  const sent = useRef(0);
  const shown = drag ?? value;
  const go = (v: number, force = false) => {
    const now = Date.now();
    if (!force && now - sent.current < 120) return;
    sent.current = now;
    try { send(v); } catch { /* not connected */ }
  };
  const release = () => { if (drag !== null) go(drag, true); setDrag(null); };
  return (
    <div className="flex items-center gap-2">
      <input type="range" min={min} max={max} value={shown} disabled={disabled} aria-label={label}
        onChange={(e) => { const v = Number(e.target.value); setDrag(v); go(v); }}
        onPointerUp={release} onKeyUp={release} onBlur={release}
        className="min-w-0 flex-1 accent-accent" />
      <span className="w-12 shrink-0 text-right text-xs tabular-nums">{disabled ? '–' : fmt(shown)}</span>
    </div>
  );
}

function ToneSlider({
  id, knob, label, value, min, max, fmt,
}: {
  id: string;
  knob: 'bass' | 'treble' | 'balance';
  label: string;
  value: number;
  min: number;
  max: number;
  fmt: (v: number) => string;
}) {
  const [drag, setDrag] = useState<number | null>(null);
  const sent = useRef(0);
  const shown = drag ?? value;
  const send = (v: number, force = false) => {
    const now = Date.now();
    if (!force && now - sent.current < 120) return;
    sent.current = now;
    try { io.setTone(id, knob, v); } catch { /* not connected */ }
  };
  const release = () => {
    if (drag !== null) send(drag, true);
    setDrag(null);
  };
  return (
    <label className="flex min-w-0 flex-1 items-center gap-2 text-xs">
      <span className="w-12 shrink-0 text-muted">{label}</span>
      <input type="range" min={min} max={max} value={shown} aria-label={label}
        onChange={(e) => { const v = Number(e.target.value); setDrag(v); send(v); }}
        onPointerUp={release} onKeyUp={release} onBlur={release}
        onDoubleClick={() => { setDrag(null); send(0, true); }}
        className="min-w-0 flex-1 accent-accent" />
      <span className="w-12 shrink-0 text-right tabular-nums">{fmt(shown)}</span>
    </label>
  );
}


const clock = (s: number) => {
  const t = Math.max(0, Math.floor(s));
  return `${Math.floor(t / 60)}:${String(t % 60).padStart(2, '0')}`;
};

function useElapsed(st: LibraryState | undefined): number | undefined {
  const [, tick] = useState(0);
  useEffect(() => {
    if (st?.state !== 'play') return;
    const t = setInterval(() => tick((n) => n + 1), 500);
    return () => clearInterval(t);
  }, [st?.state]);
  if (st?.elapsed_s === undefined) return undefined;
  if (st.state !== 'play' || !st.elapsed_at_ms) return st.elapsed_s;
  return Math.min(st.duration_s ?? Infinity, st.elapsed_s + (Date.now() - st.elapsed_at_ms) / 1000);
}

function LibraryPlayer({ state, meta }: { state: LibraryState | undefined; meta: AudioMeta | undefined }) {
  const send = (verb: string, arg = '') => { try { io.library(verb, arg); } catch { /* not connected */ } };
  const elapsed = useElapsed(state);
  const playing = state?.state === 'play';
  const offline = !state || state.state === 'offline';
  const [badCover, setBadCover] = useState<string | null>(null);
  const cover = meta?.cover && meta.cover !== badCover ? meta.cover : null;
  const btn = 'rounded-lg p-2 text-muted hover:text-ink disabled:opacity-30';

  return (
    <div className="hair rounded-xl border bg-raised p-3">
      <div className="mb-2 flex items-center gap-2 text-xs text-muted">
        Player
        {state?.updating && (
          <span className="ml-auto flex items-center gap-1">
            <RefreshCw size={12} className="animate-spin" /> indexing drives
          </span>
        )}
      </div>

      {offline ? (
        <p className="text-xs text-muted">The library player is starting (or not installed on this image).</p>
      ) : (
        <>
          <div className="flex items-center gap-3">
            {cover ? (
              <img src={audioCover(cover)} alt="" onError={() => setBadCover(cover)} className="size-14 shrink-0 rounded-lg object-cover" />
            ) : (
              <div className="hair flex size-14 shrink-0 items-center justify-center rounded-lg border bg-raised">
                <Music size={20} className="text-muted" />
              </div>
            )}
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm">{meta?.title || (state.queue_length ? 'Stopped' : 'Nothing queued')}</div>
              <div className="truncate text-xs text-muted">{[meta?.artist, meta?.album].filter(Boolean).join(' — ')}</div>
              {state.duration_s !== undefined && elapsed !== undefined && (
                <div className="mt-1.5 flex items-center gap-2 text-xs tabular-nums text-muted">
                  <span>{clock(elapsed)}</span>
                  <input type="range" min={0} max={Math.max(1, Math.floor(state.duration_s))} value={Math.floor(elapsed)}
                    onChange={(e) => send('seek', e.target.value)} aria-label="Position"
                    className="min-w-0 flex-1 accent-accent" />
                  <span>{clock(state.duration_s)}</span>
                </div>
              )}
            </div>
          </div>
          <div className="mt-2 flex items-center gap-1">
            <button className={btn} onClick={() => send('previous')} disabled={!state.queue_length} aria-label="Previous"><SkipBack size={16} /></button>
            <button className={`${btn} text-ink`} onClick={() => send('toggle')} disabled={!state.queue_length} aria-label={playing ? 'Pause' : 'Play'}>
              {playing ? <Pause size={18} /> : <Play size={18} />}
            </button>
            <button className={btn} onClick={() => send('next')} disabled={!state.queue_length} aria-label="Next"><SkipForward size={16} /></button>
            <button className={btn} onClick={() => send('stop')} disabled={state.state === 'stop'} aria-label="Stop"><Square size={14} /></button>
            <button className={`${btn} ${state.random ? 'text-accent' : ''}`} onClick={() => send('random', state.random ? 'OFF' : 'ON')} aria-label="Shuffle" title="Shuffle"><Shuffle size={15} /></button>
            <button className={`${btn} ${state.repeat ? 'text-accent' : ''}`} onClick={() => send('repeat', state.repeat ? 'OFF' : 'ON')} aria-label="Repeat" title="Repeat"><Repeat size={15} /></button>
            <span className="ml-auto text-xs text-muted">{state.queue_length} in queue</span>
          </div>
          <Browser onPlay={(uri) => send('replace', uri)} onQueue={(uri) => send('add', uri)} />
        </>
      )}
    </div>
  );
}

/* Folders and tracks on the drives. Clicking a folder opens it, clicking a
   track plays it; ▶ plays the row (a folder: everything under it), + queues it. */
function Browser({ onPlay, onQueue }: { onPlay: (uri: string) => void; onQueue: (uri: string) => void }) {
  const [path, setPath] = useState('');
  const [q, setQ] = useState('');
  const [items, setItems] = useState<LibraryEntry[] | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setErr(null);
    const load = q.trim() ? rest.librarySearch(q.trim()) : rest.libraryBrowse(path);
    const t = setTimeout(() => {
      load.then((v) => live && setItems(v)).catch((e) => live && setErr(e instanceof Error ? e.message : String(e)));
    }, q.trim() ? 250 : 0);
    return () => { live = false; clearTimeout(t); };
  }, [path, q]);

  const crumbs = path ? path.split('/') : [];
  const base = (p: string) => p.split('/').pop() ?? p;
  return (
    <div className="mt-3 border-t pt-3" style={{ borderColor: 'var(--edge)' }}>
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1 text-xs">
          <button onClick={() => { setPath(''); setQ(''); }} className="text-muted hover:text-ink">Drives</button>
          {!q && crumbs.map((c, i) => (
            <span key={i} className="flex items-center gap-1">
              <ChevronRight size={12} className="text-muted" />
              <button onClick={() => setPath(crumbs.slice(0, i + 1).join('/'))} className="max-w-40 truncate hover:text-ink">{c}</button>
            </span>
          ))}
        </div>
        <label className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-2 py-1">
          <Search size={13} className="text-muted" />
          <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search" aria-label="Search the library"
            className="w-32 bg-transparent text-xs text-ink outline-none sm:w-44" />
        </label>
      </div>
      {err && <p className="text-xs text-alarm">{err}</p>}
      {items && !items.length && (
        <p className="text-xs text-muted">{q ? 'No matches.' : path ? 'Empty folder.' : 'No drives with music yet — plug one in (it is indexed automatically).'}</p>
      )}
      <div className="max-h-80 space-y-0.5 overflow-auto">
        {items?.map((e) => (
          <div key={e.path} className="group flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm hover:bg-raised">
            {e.kind === 'dir' ? <Folder size={14} className="shrink-0 text-muted" /> : <FileAudio size={14} className="shrink-0 text-muted" />}
            <button className="min-w-0 flex-1 truncate text-left" title={e.path}
              onClick={() => (e.kind === 'dir' ? (setQ(''), setPath(e.path)) : onPlay(e.path))}>
              {e.kind === 'dir' ? base(e.path) : e.title || base(e.path)}
              {e.kind === 'file' && e.artist && <span className="text-xs text-muted"> — {e.artist}</span>}
            </button>
            {e.duration_s !== undefined && <span className="shrink-0 text-xs tabular-nums text-muted">{clock(e.duration_s)}</span>}
            <button onClick={() => onPlay(e.path)} title="Play" aria-label={`Play ${base(e.path)}`}
              className="rounded-md p-1 text-muted opacity-60 hover:text-ink group-hover:opacity-100"><Play size={13} /></button>
            <button onClick={() => onQueue(e.path)} title="Add to queue" aria-label={`Queue ${base(e.path)}`}
              className="rounded-md p-1 text-muted opacity-60 hover:text-ink group-hover:opacity-100"><Plus size={13} /></button>
          </div>
        ))}
      </div>
    </div>
  );
}

