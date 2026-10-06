import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react';
import {
  ReactFlow, Background, Controls, Handle, Panel, Position, useNodesState, NodeToolbar, EdgeToolbar, BaseEdge,
  getBezierPath, type Connection, type Edge, type EdgeChange, type EdgeProps, type Node, type NodeProps,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {
  Music, Radio, Cable, Library, Speaker, Bell, Volume2, VolumeX, X, Plus, Trash2, Unplug, ChevronRight, RotateCcw,
  Play, Pause, SkipBack, SkipForward, Square, Folder, FileAudio, Search, Shuffle, Repeat, RefreshCw,
} from 'lucide-react';
import {
  io, rest, audioCover, type AudioEndpoint, type AudioEndpoints, type AudioMap, type AudioMeta, type IoState,
  type LibraryEntry, type LibraryState,
} from '../api';

/* The audio switcher: the whole Audio page on boards with named outputs.

   Sources (Spotify Connect and AirPlay endpoints, the library player, the
   hardware inputs) on the left, outputs on the right. Two kinds of line:
   a DEFAULT route (dashed, quiet) is where a source plays when its trigger
   fires — today, when it starts playing — and is configuration; a LIVE line
   (solid, lit) is audio actually flowing now. Drag from a source to an output
   to set its default; select a line and press Delete (or use its popup) to
   remove it. A Spotify, AirPlay
   or library source plays to one output, so a new line replaces its old one;
   an input can feed any number of outputs. Click any node for its settings:
   an output's level, tone and announcements; a source's name, what it is
   playing, and for the library its player and browser — in a popup on the
   chart itself (React Flow's NodeToolbar / EdgeToolbar), attached to what was
   clicked.

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
  /** Labels of the outputs it is assigned to (lines are drawn only while
   *  audio flows, so the node says where it would play). */
  targets: string[];
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

/* What the popups need: the map, the live state, and how to save or close. */
type Flow = {
  eps: AudioEndpoints;
  map: AudioMap;
  live: Live;
  save: (e: AudioEndpoints) => void;
  close: () => void;
  feeding: (outputId: string) => SourceData[];
  disconnect: (edgeId: string) => void;
  nameOf: (nodeId: string) => string;
};
const FlowCtx = createContext<Flow | null>(null);
const useFlow = () => useContext(FlowCtx)!;

/* A popup on the chart (NodeToolbar/EdgeToolbar content): a card with a
   pointer toward what it belongs to, a header, a body of sections and an
   optional footer of actions. Fixed size whatever the zoom; the pointer
   belongs to it (nodrag/nopan/nowheel), so a slider drag does not pan. */
function Popup({
  side, icon, title, subtitle, onClose, footer, children,
}: {
  /** Which side of the card points at its node. */
  side: 'left' | 'right';
  icon: React.ReactNode;
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  onClose: () => void;
  footer?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="nodrag nopan nowheel relative w-[300px] text-left" onClick={(e) => e.stopPropagation()}>
      <span aria-hidden
        className={`hair absolute top-6 size-3 rotate-45 border bg-panel ${side === 'left' ? '-left-1.5 border-r-0 border-t-0' : '-right-1.5 border-b-0 border-l-0'}`} />
      <div className="hair relative overflow-hidden rounded-2xl border bg-panel shadow-2xl">
        <header className="flex items-center gap-3 px-4 pb-3 pt-4">
          <div className="shrink-0">{icon}</div>
          <div className="min-w-0 flex-1 leading-tight">
            <div className="truncate text-sm font-medium text-ink">{title}</div>
            {subtitle && <div className="mt-0.5 truncate text-xs text-muted">{subtitle}</div>}
          </div>
          <button onClick={onClose} className="-mr-1 self-start rounded-md p-1 text-muted hover:bg-raised hover:text-ink" aria-label="Close">
            <X size={15} />
          </button>
        </header>
        <div className="max-h-[60vh] space-y-4 overflow-y-auto px-4 pb-4">{children}</div>
        {footer && <footer className="flex items-center gap-2 border-t px-4 py-2.5" style={{ borderColor: 'var(--edge)' }}>{footer}</footer>}
      </div>
    </div>
  );
}

function Section({ title, action, children }: { title: string; action?: React.ReactNode; children: React.ReactNode }) {
  return (
    <section>
      <div className="mb-1.5 flex items-center text-[11px] font-medium uppercase tracking-wider text-muted">
        {title}
        {action && <span className="ml-auto normal-case tracking-normal">{action}</span>}
      </div>
      {children}
    </section>
  );
}

function IconChip({ icon: Icon, live }: { icon: typeof Music; live?: boolean }) {
  return (
    <div className={`flex size-10 items-center justify-center rounded-xl ${live ? 'bg-live/15 text-live' : 'bg-accent/12 text-accent'}`}>
      <Icon size={18} />
    </div>
  );
}

/* ── nodes ────────────────────────────────────────────────────────────────── */

function SourceNode({ data, selected }: NodeProps<Node<SourceData>>) {
  const f = useFlow();
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
          {track || `${KIND_LABEL[data.kind]}${!data.connected ? (data.kind === 'input' ? ' · not routed' : ' · no default') : data.running || data.kind === 'input' ? '' : ' · not running'}`}
        </div>
      </div>
      {data.playing && <span className="size-2 shrink-0 rounded-full bg-live" title="playing" />}
      <Handle type="source" position={Position.Right} className="!size-3 !border-2 !border-[var(--panel)] !bg-[var(--accent)]" />
      <NodeToolbar isVisible={selected} position={Position.Right} align="start" offset={16}>
        <SourceInspector s={data} eps={f.eps} map={f.map} live={f.live} onSave={f.save} onClose={f.close} />
      </NodeToolbar>
    </div>
  );
}

function OutputNode({ data, selected }: NodeProps<Node<OutputData>>) {
  const f = useFlow();
  const port = f.map.outputs.find((o) => o.id === data.id);
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
      <NodeToolbar isVisible={selected} position={Position.Left} align="start" offset={16}>
        <OutputInspector id={data.id} label={data.label} device={port?.device ?? ''} live={f.live} feeding={f.feeding(data.id)}
          onClose={f.close} />
      </NodeToolbar>
    </div>
  );
}

type RouteData = { live: boolean };

/* The two line states, in one place (the legend draws the same). */
const LINE = {
  live: { stroke: 'var(--live)', strokeWidth: 2.5 },
  default: { stroke: 'var(--muted)', strokeWidth: 1.25, strokeDasharray: '5 5', opacity: 0.55 },
} as const;

/* A route: dashed while it is only a default, solid and lit while audio flows
   over it, with its popup (from → to, state, remove) at its midpoint when
   selected. */
function RouteEdge(p: EdgeProps<Edge<RouteData>>) {
  const f = useFlow();
  const [path, x, y] = getBezierPath(p);
  const live = !!p.data?.live;
  const style = { ...(live ? LINE.live : LINE.default), ...(p.selected ? { stroke: 'var(--accent)', opacity: 1 } : {}) };
  return (
    <>
      <BaseEdge id={p.id} path={path} style={style} interactionWidth={18} />
      <EdgeToolbar edgeId={p.id} x={x} y={y} isVisible={p.selected}>
        <div className="nodrag nopan hair flex items-center gap-2 rounded-full border bg-panel py-1.5 pl-3.5 pr-1.5 text-xs shadow-2xl">
          <span className={`size-1.5 shrink-0 rounded-full ${live ? 'bg-live' : 'bg-[var(--muted)]'}`} />
          <span className="max-w-36 truncate">{f.nameOf(p.source)}</span>
          <ChevronRight size={12} className="shrink-0 text-muted" />
          <span className="max-w-28 truncate">{f.nameOf(p.target)}</span>
          <span className="shrink-0 text-muted">{live ? 'playing' : 'default'}</span>
          <button onClick={() => f.disconnect(p.id)}
            className="ml-1 flex items-center gap-1 rounded-full bg-raised px-2.5 py-1 text-muted hover:bg-alarm/15 hover:text-alarm">
            <Unplug size={12} /> Remove
          </button>
        </div>
      </EdgeToolbar>
    </>
  );
}

function Legend() {
  const row = (style: React.CSSProperties, label: string) => (
    <div className="flex items-center gap-2">
      <svg width="28" height="6" aria-hidden><line x1="0" y1="3" x2="28" y2="3" style={style} /></svg>
      {label}
    </div>
  );
  return (
    <div className="hair space-y-1 rounded-lg border bg-panel/90 px-3 py-2 text-[11px] text-muted shadow-sm backdrop-blur">
      {row(LINE.live, 'Playing')}
      {row(LINE.default, 'Default route')}
    </div>
  );
}
const edgeTypes = { route: RouteEdge };

// Not `input`/`output`: those are React Flow's built-in node types, and their
// default styling (a box behind the node) applies by class name.

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

  const outLabel = (id: string) => map.outputs.find((o) => o.id === id)?.label ?? id;

  // What to draw, from the map and the live state.
  const sources: Array<{ id: string; data: SourceData }> = [];
  (['spotify', 'airplay', 'library'] as const).forEach((kind) =>
    (eps[kind] ?? []).forEach((e, n) => {
      const i = e.output ? inst(kind, e.name, e.output) : undefined;
      sources.push({ id: srcId(kind, n), data: {
        kind, index: n, name: e.name, tag: i?.tag, running: !!i?.running, playing: !!i?.running && !!i?.playing,
        meta: i?.tag ? live.meta?.[i.tag] : undefined, connected: !!e.output,
        targets: e.output ? [outLabel(e.output)] : [],
      } });
    }));
  map.inputs.forEach((p, n) => {
    const routes = eps.routes.filter((r) => r.input === p.id);
    sources.push({ id: srcId('input', p.id), data: {
      kind: 'input', index: n, name: p.label, running: true,
      playing: routes.some((r) => inst('route', p.id, r.output)?.running), connected: routes.length > 0,
      targets: routes.map((r) => outLabel(r.output)),
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

  const edges: Edge<RouteData>[] = [];
  (['spotify', 'airplay', 'library'] as const).forEach((kind) =>
    (eps[kind] ?? []).forEach((e, n) => {
      if (!e.output) return;
      const playing = sources.find((s) => s.id === srcId(kind, n))?.data.playing;
      const id = `edge:${kind}:${n}`;
      edges.push({ id, type: 'route', source: srcId(kind, n), target: outId(e.output), selected: sel === id,
        data: { live: !!playing }, zIndex: playing ? 1 : 0 });
    }));
  eps.routes.forEach((r, n) => {
    const playing = !!inst('route', r.input, r.output)?.running;
    const id = `edge:route:${n}`;
    edges.push({ id, type: 'route', source: srcId('input', r.input), target: outId(r.output), selected: sel === id,
      data: { live: playing }, zIndex: playing ? 1 : 0 });
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

  const nameOf = (nodeId: string) =>
    sources.find((s) => s.id === nodeId)?.data.name ?? map.outputs.find((o) => outId(o.id) === nodeId)?.label ?? nodeId;
  // Everything assigned to an output (lines show only what is playing).
  const feedingOf = (o: string) => {
    const label = outLabel(o);
    return sources.filter((s) => s.data.targets.includes(label)).map((s) => s.data);
  };
  const flow: Flow = { eps, map, live, save, close: () => setSel(null), feeding: feedingOf, disconnect, nameOf };
  const hasLibrary = (eps.library ?? []).length > 0;

  return (
    <FlowCtx.Provider value={flow}>
      <div className="absolute inset-0">
        <ReactFlow
          nodes={nodes.map((n) => ({ ...n, selected: n.id === sel }))}
          edges={edges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
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
          fitViewOptions={{ padding: 0.12, maxZoom: 1.25 }}
          minZoom={0.3}
          proOptions={{ hideAttribution: true }}
          deleteKeyCode={['Backspace', 'Delete']}
        >
          <Background gap={20} size={1} />
          <Controls showInteractive={false} />
          <Panel position="top-left">
            <AddSource onAdd={addSource} hasLibrary={hasLibrary} />
          </Panel>
          <Panel position="bottom-right">
            <Legend />
          </Panel>
          {err && (
            <Panel position="bottom-center">
              <div className="rounded-lg border border-alarm/40 bg-panel px-3 py-1.5 text-xs text-alarm">{err}</div>
            </Panel>
          )}
        </ReactFlow>
      </div>
    </FlowCtx.Provider>
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

/* ── inspectors (popup contents) ───────────────────────────────────────────── */

const pill = (on: boolean) =>
  `rounded-full border px-2.5 py-1 text-xs transition ${on ? 'border-accent/60 bg-accent/15 text-ink' : 'hair bg-raised text-muted hover:text-ink'}`;

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
  const [name, setName] = useState(s.name);
  const [bad, setBad] = useState<string | null>(null);
  const m = s.meta;
  const clone = (): AudioEndpoints => JSON.parse(JSON.stringify(eps));
  const list = (e: AudioEndpoints) => (s.kind === 'input' ? null : (e[s.kind] ?? []) as AudioEndpoint[]);
  const ep = list(eps)?.[s.index];
  const inputId = s.kind === 'input' ? map.inputs[s.index]?.id : undefined;
  const status = s.playing ? 'Playing' : !s.connected ? 'No default output' : s.kind === 'input' || s.running ? 'Idle' : 'Not running';
  const cover = m?.cover && m.cover !== bad ? m.cover : null;

  const rename = () => {
    const n = name.trim();
    if (!n || n === s.name || s.kind === 'input') return;
    const next = clone();
    list(next)![s.index].name = n;
    onSave(next);
  };
  // Endpoints play to one output (or none); an input to any number.
  const assigned = (o: string) =>
    s.kind === 'input' ? eps.routes.some((r) => r.input === inputId && r.output === o) : ep?.output === o;
  const toggle = (o: string) => {
    const next = clone();
    if (s.kind === 'input') {
      const i = next.routes.findIndex((r) => r.input === inputId && r.output === o);
      if (i >= 0) next.routes.splice(i, 1);
      else next.routes.push({ input: inputId!, output: o });
    } else {
      const e = list(next)![s.index];
      e.output = e.output === o ? '' : o;
    }
    onSave(next);
  };

  return (
    <Popup side="left" onClose={onClose}
      icon={cover
        ? <img src={audioCover(cover)} alt="" onError={() => setBad(cover)} className="size-10 rounded-xl object-cover" />
        : <IconChip icon={KIND_ICON[s.kind]} live={s.playing} />}
      title={m?.title || s.name}
      subtitle={m?.title ? [m.artist, m.client && `from ${m.client}`].filter(Boolean).join(' · ') : `${KIND_LABEL[s.kind]} · ${status}`}
      footer={s.kind === 'input' ? undefined : (
        <>
          <span className={`flex items-center gap-1.5 text-xs ${s.playing ? 'text-live' : 'text-muted'}`}>
            <span className={`size-1.5 rounded-full ${s.playing ? 'bg-live' : 'bg-[var(--muted)]'}`} /> {status}
          </span>
          <button onClick={() => {
            if (!confirm(`Remove "${s.name}"?`)) return;
            const next = clone(); list(next)!.splice(s.index, 1); onSave(next); onClose();
          }} className="ml-auto flex items-center gap-1 rounded-md px-2 py-1 text-xs text-muted hover:bg-alarm/10 hover:text-alarm">
            <Trash2 size={12} /> Remove
          </button>
        </>
      )}>
      {s.kind !== 'input' && (
        <Section title={s.kind === 'spotify' ? 'Name in Spotify' : s.kind === 'airplay' ? 'Name on Apple devices' : 'Name'}>
          <input value={name} onChange={(e) => setName(e.target.value)} onBlur={rename} maxLength={64}
            onKeyDown={(e) => e.key === 'Enter' && (e.target as HTMLInputElement).blur()}
            className="hair w-full rounded-lg border bg-raised px-2.5 py-1.5 text-sm text-ink outline-none focus:border-accent/60" />
        </Section>
      )}

      <Section title={s.kind === 'input' ? 'Default outputs' : 'Default output'}>
        <div className="flex flex-wrap gap-1.5">
          {map.outputs.map((o) => (
            <button key={o.id} className={pill(assigned(o.id))} onClick={() => toggle(o.id)}>{o.label}</button>
          ))}
        </div>
        <p className="mt-1.5 text-[11px] text-muted">
          {s.kind === 'input' ? 'Plays live on each one selected.' : 'Where it plays when it starts.'}
        </p>
      </Section>

      {m?.album && (
        <Section title="Album">
          <div className="truncate text-sm">{m.album}</div>
        </Section>
      )}

      {s.kind === 'library' && <LibraryPlayer state={live.library} meta={m} />}
    </Popup>
  );
}

function OutputInspector({
  id, label, device, live, feeding, onClose,
}: {
  id: string;
  label: string;
  device: string;
  live: Live;
  feeding: SourceData[];
  onClose: () => void;
}) {
  const level = live.level?.[id];
  const lastLevel = useRef(level && level > 0 ? level : 60);
  if (level && level > 0) lastLevel.current = level;
  const hasTone = live.bass !== undefined;
  const tone = { bass: live.bass?.[id] ?? 0, treble: live.treble?.[id] ?? 0, balance: live.balance?.[id] ?? 0 };
  const flat = tone.bass === 0 && tone.treble === 0 && tone.balance === 0;
  const db = (v: number) => (v > 0 ? `+${v}` : `${v}`);
  const bal = (v: number) => (v === 0 ? 'C' : v < 0 ? `L${-v}` : `R${v}`);
  const announcing = !!live.announcing?.[id];
  const playing = feeding.filter((f) => f.playing).length;
  const muted = level === 0;

  return (
    <Popup side="right" onClose={onClose}
      icon={<IconChip icon={Speaker} live={playing > 0} />}
      title={label}
      subtitle={announcing ? 'Announcing' : playing ? `${playing} playing` : 'Idle'}
      footer={
        <>
          <button onClick={() => { try { io.announce(id); } catch { /* not connected */ } }}
            className="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-muted hover:bg-raised hover:text-ink">
            <Bell size={13} /> Chime
          </button>
          <span className="ml-auto truncate font-mono text-[10px] text-muted" title={device}>{device}</span>
        </>
      }>
      <Section title="Volume">
        <div className="flex items-center gap-2">
          <button disabled={level === undefined} aria-label={muted ? 'Unmute' : 'Mute'} title={muted ? 'Unmute' : 'Mute'}
            onClick={() => { try { io.setAudioLevel(id, muted ? lastLevel.current : 0); } catch { /* not connected */ } }}
            className={`rounded-lg p-1.5 hover:bg-raised ${muted ? 'text-alarm' : 'text-muted hover:text-ink'}`}>
            {muted ? <VolumeX size={16} /> : <Volume2 size={16} />}
          </button>
          <div className="min-w-0 flex-1">
            <LiveSlider value={level ?? 0} min={0} max={100} disabled={level === undefined} label={`${label} volume`}
              fmt={(v) => `${v}%`} send={(v) => io.setAudioLevel(id, v)} />
          </div>
        </div>
      </Section>

      {hasTone && (
        <Section title="Tone" action={
          <button disabled={flat} title="Flat and centred" aria-label="Reset tone"
            onClick={() => { try { (['bass', 'treble', 'balance'] as const).forEach((k) => io.setTone(id, k, 0)); } catch { /* not connected */ } }}
            className="flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] text-muted hover:text-ink disabled:opacity-30">
            <RotateCcw size={11} /> Reset
          </button>
        }>
          <div className="space-y-1">
            <ToneSlider id={id} knob="bass" label="Bass" value={tone.bass} min={-12} max={12} fmt={db} />
            <ToneSlider id={id} knob="treble" label="Treble" value={tone.treble} min={-12} max={12} fmt={db} />
            <ToneSlider id={id} knob="balance" label="Balance" value={tone.balance} min={-100} max={100} fmt={bal} />
          </div>
        </Section>
      )}

      <Section title="Sources">
        {feeding.length ? (
          <div className="flex flex-wrap gap-1.5">
            {feeding.map((f) => {
              const Icon = KIND_ICON[f.kind];
              return (
                <span key={`${f.kind}:${f.index}`} title={`${f.name} · ${KIND_LABEL[f.kind]}`}
                  className={`flex max-w-full items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs ${f.playing ? 'border-live/50 bg-live/10 text-ink' : 'hair bg-raised text-muted'}`}>
                  <Icon size={12} className="shrink-0" />
                  <span className="truncate">{f.name}</span>
                </span>
              );
            })}
          </div>
        ) : (
          <p className="text-xs text-muted">None — drag a source here.</p>
        )}
      </Section>
    </Popup>
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
      <span className="w-10 shrink-0 text-right text-xs tabular-nums">{disabled ? '–' : fmt(shown)}</span>
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
  return (
    <div className="flex items-center gap-2 text-xs" onDoubleClick={() => { try { io.setTone(id, knob, 0); } catch { /* not connected */ } }}
      title="Double-click to reset">
      <span className="w-14 shrink-0 text-muted">{label}</span>
      <div className="min-w-0 flex-1">
        <LiveSlider value={value} min={min} max={max} label={label} fmt={fmt} send={(v) => io.setTone(id, knob, v)} />
      </div>
    </div>
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

