import { useEffect, useMemo, useRef, useState } from 'react';
import { ReactFlow, Background, Controls, type Edge, type Node } from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import { ChevronDown, RefreshCw, Share2 } from 'lucide-react';
import { type Proto, trees, useTree, zigbeeDevices, zwaveDevices } from './model';
import { ghost } from './Radios';

/* The mesh as the coordinator sees it: the radio's own `topology` request.

   Zigbee answers with links (who heard whom, with LQI and RSSI) from last hops,
   route records and neighbour tables. Z-Wave asks every node for its neighbours
   and answers with them, plus the repeaters each node's last frame came through.
   Both become one picture: the coordinator in the middle, each device on a ring
   by how many hops it is from it, links coloured by how good they are. */

type Graph = { nodes: { id: string; label: string; hub: boolean; hops: number }[]; edges: { a: string; b: string; quality?: number; route?: boolean; text?: string }[] };

const HUB = 'hub';

function zigbeeGraph(data: any, names: Map<string, string>, coordinator?: string): Graph {
  const links: any[] = data?.links ?? [];
  const id = (ieee: string | null, nwk: number) => (nwk === 0 || (coordinator && ieee === coordinator) ? HUB : ieee ?? `0x${nwk.toString(16).padStart(4, '0')}`);
  const pairs = new Map<string, Graph['edges'][number]>();
  for (const l of links) {
    const a = id(l.from, l.from_network_address);
    const b = id(l.to, l.to_network_address);
    if (a === b) continue;
    const k = [a, b].sort().join('|');
    const best = pairs.get(k);
    if (!best || (l.lqi ?? 0) > (best.quality ?? 0)) {
      pairs.set(k, { a, b, quality: l.lqi, text: l.lqi !== undefined ? `LQI ${l.lqi}${l.rssi !== undefined ? ` · ${l.rssi} dBm` : ''}` : undefined });
    }
  }
  const edges = [...pairs.values()];
  const ids = new Set<string>([HUB, ...names.keys(), ...edges.flatMap((e) => [e.a, e.b])]);
  return { nodes: ring(ids, edges, (n) => names.get(n) ?? n), edges };
}

function zwaveGraph(data: any, names: Map<string, string>): Graph {
  const ctl = String(data?.controller ?? 1);
  const nid = (n: number | string) => (String(n) === ctl ? HUB : String(n));
  const pairs = new Map<string, Graph['edges'][number]>();
  const routed = new Set<string>();
  for (const n of data?.nodes ?? []) {
    // The path its last frame took: node, its repeaters, the controller.
    const path = [nid(n.id), ...(n.route ?? []).map(nid), HUB];
    for (let i = 0; i + 1 < path.length; i++) routed.add([path[i], path[i + 1]].sort().join('|'));
    for (const m of n.neighbors ?? []) {
      const a = nid(n.id);
      const b = nid(m);
      if (a !== b) pairs.set([a, b].sort().join('|'), { a, b });
    }
    if (typeof n.rssi === 'number' && !(n.route ?? []).length) {
      const k = [nid(n.id), HUB].sort().join('|');
      pairs.set(k, { a: nid(n.id), b: HUB, text: `${n.rssi} dBm`, quality: rssiQuality(n.rssi) });
    }
  }
  for (const k of routed) {
    const [a, b] = k.split('|');
    pairs.set(k, { ...(pairs.get(k) ?? { a, b }), route: true });
  }
  const edges = [...pairs.values()];
  const ids = new Set<string>([HUB, ...names.keys(), ...edges.flatMap((e) => [e.a, e.b])]);
  return { nodes: ring(ids, edges, (n) => names.get(n) ?? `Node ${n}`), edges };
}

/** -50 dBm or louder is excellent, -90 or quieter is poor; as an LQI-like 0..255. */
const rssiQuality = (dbm: number) => Math.max(0, Math.min(255, Math.round(((dbm + 95) / 45) * 255)));

/** Hops from the hub by breadth-first search over the links; unreachable ones last. */
function ring(ids: Set<string>, edges: Graph['edges'], name: (id: string) => string): Graph['nodes'] {
  const adj = new Map<string, string[]>();
  for (const e of edges) {
    adj.set(e.a, [...(adj.get(e.a) ?? []), e.b]);
    adj.set(e.b, [...(adj.get(e.b) ?? []), e.a]);
  }
  const hops = new Map<string, number>([[HUB, 0]]);
  const queue = [HUB];
  while (queue.length) {
    const cur = queue.shift()!;
    for (const nb of adj.get(cur) ?? []) {
      if (!hops.has(nb)) {
        hops.set(nb, hops.get(cur)! + 1);
        queue.push(nb);
      }
    }
  }
  const far = Math.max(1, ...hops.values()) + 1;
  return [...ids].map((id) => ({ id, label: id === HUB ? '' : name(id), hub: id === HUB, hops: hops.get(id) ?? far }));
}

const NODE_W = 130;
const NODE_H = 32;
const MAP_H = 420;

/** The viewport that fits every node, centred, at most 1.2x. */
function fit(nodes: Node[], width: number, height = MAP_H) {
  const xs = nodes.map((n) => n.position.x);
  const ys = nodes.map((n) => n.position.y);
  const [x0, x1] = [Math.min(...xs), Math.max(...xs) + NODE_W];
  const [y0, y1] = [Math.min(...ys), Math.max(...ys) + NODE_H];
  const zoom = Math.min(1.2, (width * 0.8) / (x1 - x0), (height * 0.8) / (y1 - y0));
  return { zoom, x: width / 2 - ((x0 + x1) / 2) * zoom, y: height / 2 - ((y0 + y1) / 2) * zoom };
}

function layout(g: Graph, hubLabel: string): { nodes: Node[]; edges: Edge[] } {
  const rings = new Map<number, string[]>();
  for (const n of g.nodes) if (!n.hub) rings.set(n.hops, [...(rings.get(n.hops) ?? []), n.id]);
  const pos = new Map<string, { x: number; y: number }>([[HUB, { x: 0, y: 0 }]]);
  for (const [r, members] of rings) {
    members.forEach((id, i) => {
      const a = (2 * Math.PI * i) / members.length - Math.PI / 2 + r * 0.4;
      pos.set(id, { x: Math.cos(a) * 190 * r, y: Math.sin(a) * 150 * r });
    });
  }
  const nodes: Node[] = g.nodes.map((n) => ({
    id: n.id,
    position: pos.get(n.id)!,
    data: { label: n.hub ? hubLabel : n.label },
    draggable: true,
    connectable: false,
    className: n.hub ? 'ohc-map-hub' : 'ohc-map-node',
    style: {
      fontSize: 11,
      width: NODE_W,
      padding: '6px 8px',
      borderRadius: 8,
      background: n.hub ? 'var(--accent)' : 'var(--raised)',
      color: n.hub ? 'white' : 'var(--ink)',
      border: '1px solid var(--edge)',
    },
  }));
  const colour = (q?: number) => (q === undefined ? 'var(--muted)' : q >= 150 ? 'var(--live)' : q >= 80 ? 'var(--warm)' : 'var(--alarm)');
  const edges: Edge[] = g.edges.map((e, i) => ({
    id: `e${i}`,
    source: e.a,
    target: e.b,
    label: e.text,
    labelStyle: { fontSize: 10, fill: 'var(--muted)' },
    labelBgStyle: { fill: 'var(--panel)' },
    style: { stroke: e.route ? 'var(--accent)' : colour(e.quality), strokeWidth: e.route ? 2.5 : 1.5, opacity: e.route || e.quality !== undefined ? 1 : 0.45 },
    animated: !!e.route,
  }));
  return { nodes, edges };
}

/** `embedded`: the map is its own sub-page (always open, as tall as the view). */
export function NetworkMap({ proto, embedded = false }: { proto: Proto; embedded?: boolean }) {
  const t = useTree(proto);
  const [opened, setOpen] = useState(false);
  const open = embedded || opened;
  const asked = useRef(0);
  const [busy, setBusy] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const response = t.log.find((l) => l.tail === 'bridge/response/topology');
  useEffect(() => { if (response && response.at >= asked.current) setBusy(false); }, [response]);
  const refresh = () => {
    asked.current = Date.now();
    setBusy(true);
    trees[proto].send('bridge/request/topology');
  };
  // Asked for once, the first time the map is opened.
  useEffect(() => { if (open && !response && !busy) refresh(); }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const devices = proto === 'zigbee' ? zigbeeDevices(t) : zwaveDevices(t);
  const names = useMemo(() => new Map(devices.map((d) => [d.id, d.name])), [devices.map((d) => d.id + d.name).join()]); // eslint-disable-line react-hooks/exhaustive-deps
  const coordinator = proto === 'zigbee' ? (JSON.parse(t.topics.get('bridge/info') ?? '{}').coordinator?.ieee_address as string | undefined) : undefined;
  const graph = useMemo(() => {
    if (!response) return null;
    const data = proto === 'zigbee' ? response.data?.data : response.data;
    const g = proto === 'zigbee' ? zigbeeGraph(data, names, coordinator) : zwaveGraph(data, names);
    return layout(g, proto === 'zigbee' ? 'Coordinator' : 'Controller');
  }, [response, names, coordinator, proto]);

  return (
    <section className="hair overflow-hidden rounded-xl border bg-panel">
      {!embedded && <button className="flex w-full items-center gap-2 px-4 py-3 text-sm" onClick={() => setOpen(!open)}>
        <Share2 size={14} className="text-muted" /> Network map
        <span className="text-xs text-muted">{proto === 'zigbee' ? 'who hears whom, and how well' : 'neighbours and the routes frames take'}</span>
        <ChevronDown size={14} className={`ml-auto text-muted transition ${open ? 'rotate-180' : ''}`} />
      </button>}
      {open && (
        <div className={embedded ? '' : 'hair border-t'}>
          <div className="hair flex items-center gap-3 border-b px-4 py-2 text-xs text-muted">
            {embedded && <span className="text-ink">{proto === 'zigbee' ? 'Who hears whom, and how well' : 'Neighbours, and the route each node\'s last frame took'}</span>}
            <span className="inline-flex items-center gap-1"><span className="h-0.5 w-4 bg-live" /> strong</span>
            <span className="inline-flex items-center gap-1"><span className="h-0.5 w-4 bg-warm" /> fair</span>
            <span className="inline-flex items-center gap-1"><span className="h-0.5 w-4 bg-alarm" /> weak</span>
            {proto === 'zwave' && <span className="inline-flex items-center gap-1"><span className="h-0.5 w-4 bg-accent" /> last route</span>}
            <button className={`${ghost} ml-auto px-2 py-1 text-xs`} disabled={busy} onClick={refresh}>
              <RefreshCw size={12} className={busy ? 'animate-spin' : ''} /> {busy ? (proto === 'zwave' ? 'Asking every node…' : 'Reading…') : 'Refresh'}
            </button>
          </div>
          <div ref={box} style={{ height: embedded ? 'max(420px, calc(100vh - 250px))' : MAP_H }}>
            {graph ? (
              <div className="ohc-map h-full">
              {/* Keyed by the answer, so a new topology mounts fresh and is fitted again. */}
              <ReactFlow
                key={response?.at}
                // Uncontrolled: the map is shown, not edited, and the library
                // only measures nodes (which fitting needs) when it owns them.
                defaultNodes={graph.nodes}
                defaultEdges={graph.edges}
                defaultViewport={fit(graph.nodes, box.current?.clientWidth ?? 700, box.current?.clientHeight)}
                // And once it has initialised, which can apply its own default first.
                onInit={(rf) => requestAnimationFrame(() => rf.setViewport(fit(graph.nodes, box.current?.clientWidth ?? 700, box.current?.clientHeight)))} proOptions={{ hideAttribution: true }}
                nodesConnectable={false}
                // The wheel scrolls the page, not the map: zoom is the controls and pinch.
                zoomOnScroll={false}
                preventScrolling={false}>
                <Background gap={20} size={1} />
                <Controls showInteractive={false} />
              </ReactFlow>
              </div>
            ) : (
              <div className="grid h-full place-items-center text-xs text-muted">{busy ? 'Reading the mesh…' : 'No map yet.'}</div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
