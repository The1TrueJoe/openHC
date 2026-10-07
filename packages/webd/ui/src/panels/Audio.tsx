import { useEffect, useState } from 'react';
import { Music, Speaker, Radio, Volume2, CircleDot, Circle, Plus, Trash2, Cable } from 'lucide-react';
import {
  io, rest, type AudioEndpoints, type AudioInstance, type AudioMap, type AudioReceiver, type AudioStatus,
  type Capabilities,
} from '../api';
import { useIoState } from '../App';

/* Audio: the ALSA output the box renders to, and the two network receivers that
   render to it. Spotify Connect (librespot) and AirPlay (shairport-sync) are
   *receivers* — a phone drives playback — so this panel does not pretend to be a
   media player. It turns the two knobs that are genuinely ours (which output,
   and the volume) and reports what is true about the receivers. Transport is
   shown only when a receiver actually exposes it, which in this image is never;
   see packages/iod/src/audio.rs for the board-side wiring that would change that.

   Structure is seeded once from the capabilities the app loaded; everything
   that changes — selected output, volume, receiver running, and on boards with
   named outputs the whole endpoint map — arrives live over MQTT. Nothing here
   polls REST. */
export function AudioPanel({ caps }: { caps: Capabilities }) {
  const a: AudioStatus | null = caps.audio ?? null;
  const live = useIoState().audio;

  /* Same rule as every other panel: nothing behind it, nothing drawn. The rail
     entry is already gated on caps.audio, but a board that lost its card between
     load and now should collapse gracefully too. */
  if (!a || (!a.outputs.length && !a.receivers.some((r) => r.installed))) return null;

  // Live overrides win over the slower REST snapshot where present.
  const selected = live?.output ?? a.selected ?? '';
  const volume = live?.volume ?? a.volume;

  /* A board with named outputs runs N endpoints mapped onto its jacks; the
     single-output selector and the master volume do not apply there (each
     endpoint has its own volume, controlled from the phone). */
  // Live map from MQTT (retained, so it is there on connect), falling back to
  // the capabilities snapshot; rendered only once it has the full shape — the
  // retained topics arrive leaf by leaf.
  const map = live?.map ?? a.map;
  if (map && Array.isArray(map.outputs) && Array.isArray(map.instances)) return <MapPanel map={map} />;

  return (
    <div className="space-y-4">
      <OutputSection outputs={a.outputs} selected={selected} volume={volume} />
      <ReceiversSection receivers={a.receivers} live={live?.receiver} />
    </div>
  );
}

/* The output selector and the volume. Volume only appears when amixer gave us a
   level to show — on a card with no recognisable volume control it is simply
   absent rather than a slider that does nothing. */
function OutputSection({
  outputs,
  selected,
  volume,
}: {
  outputs: AudioStatus['outputs'];
  selected: string;
  volume: number | undefined;
}) {
  // Optimistic local volume so the slider tracks the thumb, reconciled by the
  // next REST/MQTT read.
  const [vol, setVol] = useState<number | null>(null);
  const shown = vol ?? volume ?? 0;

  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Speaker size={15} className="text-muted" />
        Output
      </h2>

      {outputs.length ? (
        <label className="flex flex-col gap-1.5">
          <span className="text-xs text-muted">ALSA device the receivers render to</span>
          <select
            value={selected}
            onChange={(e) => io.setAudioOutput(e.target.value)}
            className="hair w-full max-w-sm rounded-lg border bg-raised p-2.5 text-sm text-ink outline-none focus:border-accent/50"
          >
            {/* No selection yet means the receivers follow the default PCM, which
                is what S95librespot does today. */}
            {!selected && <option value="">System default</option>}
            {outputs.map((o) => (
              <option key={o.id} value={o.id}>
                {o.name} ({o.id})
              </option>
            ))}
          </select>
        </label>
      ) : (
        <p className="text-xs text-muted">No ALSA output card is present yet.</p>
      )}

      {/* The receivers read the selection at startup, so a change needs a restart
          to take effect. Honest about that rather than implying it is live. */}
      {outputs.length > 0 && (
        <p className="mt-2 text-xs text-muted">
          Receivers apply a new output when they next restart.
        </p>
      )}

      {volume !== undefined && (
        <div className="mt-4">
          <div className="mb-1.5 flex items-center gap-2 text-xs text-muted">
            <Volume2 size={14} />
            Volume
            <span className="ml-auto tabular-nums text-ink">{shown}%</span>
          </div>
          <input
            type="range"
            min={0}
            max={100}
            value={shown}
            onChange={(e) => setVol(Number(e.target.value))}
            onPointerUp={() => {
              if (vol !== null) io.setAudioVolume(vol);
            }}
            onKeyUp={() => {
              if (vol !== null) io.setAudioVolume(vol);
            }}
            className="w-full max-w-sm accent-accent"
          />
        </div>
      )}
    </section>
  );
}

/* The receivers, one card each. A receiver whose binary is not on this image
   says so plainly; a running one shows a live dot and whatever now-playing it
   has fed us (none, in this build). */
function ReceiversSection({
  receivers,
  live,
}: {
  receivers: AudioReceiver[];
  live: Record<string, { running?: boolean }> | undefined;
}) {
  if (!receivers.length) return null;
  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Radio size={15} className="text-muted" />
        Network receivers
      </h2>
      <div className="grid gap-2 sm:grid-cols-2">
        {receivers.map((r) => (
          <ReceiverCard key={r.id} r={r} running={live?.[r.id]?.running ?? r.running} />
        ))}
      </div>
    </section>
  );
}

function ReceiverCard({ r, running }: { r: AudioReceiver; running: boolean }) {
  const np = nowPlaying(r.now_playing);
  return (
    <div className="hair rounded-xl border bg-raised p-3">
      <div className="flex items-center gap-2">
        <Music size={16} className={running ? 'text-live' : 'text-muted'} />
        <div className="text-sm">{r.name}</div>
        <div className="ml-auto flex items-center gap-1 text-xs">
          {!r.installed ? (
            <span className="text-muted">not installed</span>
          ) : running ? (
            <span className="flex items-center gap-1 text-live">
              <CircleDot size={13} /> running
            </span>
          ) : (
            <span className="flex items-center gap-1 text-muted">
              <Circle size={13} /> stopped
            </span>
          )}
        </div>
      </div>

      {/* Now-playing, only when a receiver actually fed us some. */}
      {np && (
        <div className="mt-2 text-xs">
          <div className="truncate text-ink" title={np.title}>{np.title}</div>
          {np.artist && <div className="truncate text-muted" title={np.artist}>{np.artist}</div>}
        </div>
      )}

      {/* Transport is shown ONLY if the receiver exposes it. Neither does in
          this image (librespot has no control API; shairport-sync is
          built without MPRIS), so this is the honest "not available" line rather
          than dead buttons. */}
      {r.installed && !r.supports_transport && (
        <p className="mt-2 text-xs text-muted">
          Playback is controlled from the {r.kind === 'spotify' ? 'Spotify' : 'AirPlay'} app.
        </p>
      )}
    </div>
  );
}

/* Pull a title/artist out of whatever a receiver's hook wrote, without assuming
   a schema iod cannot guarantee. Returns null unless there is at least a title. */
function nowPlaying(v: unknown): { title: string; artist?: string } | null {
  if (!v || typeof v !== 'object') return null;
  const o = v as Record<string, unknown>;
  const str = (k: string) => (typeof o[k] === 'string' ? (o[k] as string) : undefined);
  const title = str('title') ?? str('track') ?? str('name');
  if (!title) return null;
  return { title, artist: str('artist') ?? str('album_artist') };
}

/* ── boards with named outputs ──────────────────────────────────────────────

   The jacks are fixed (board.env); what you configure is which endpoints exist
   and where each one plays. That is CONFIGURATION, so it is saved over REST
   (ohc-audiod, PUT /audio/api/audio/endpoints); what is running is live state
   and arrives over MQTT (`<base>/state/audio/map`). */

type Row = { left: string; right: string };
type Kind = 'spotify' | 'airplay' | 'routes';

const rowsOf = (map: AudioMap, kind: Kind): Row[] =>
  kind === 'routes'
    ? map.routes.map((r) => ({ left: r.input, right: r.output }))
    : map[kind].map((e) => ({ left: e.name, right: e.output }));

function MapPanel({ map }: { map: AudioMap }) {
  const label = (id: string) => map.outputs.find((o) => o.id === id)?.label ?? id;
  return (
    <div className="space-y-4">
      <section className="hair rounded-xl border bg-panel p-4">
        <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
          <Speaker size={15} className="text-muted" />
          Outputs and inputs
        </h2>
        <div className="flex flex-wrap gap-2 text-xs">
          {map.outputs.map((o) => (
            <span key={o.id} className="hair rounded-lg border bg-raised px-2.5 py-1.5" title={o.device}>
              {o.label}
            </span>
          ))}
          {map.inputs.map((i) => (
            <span key={i.id} className="hair rounded-lg border bg-raised px-2.5 py-1.5 text-muted" title={i.device}>
              {i.label} (input)
            </span>
          ))}
        </div>
        <p className="mt-2 text-xs text-muted">
          Any number of endpoints can share an output; they mix. Volume is per endpoint, from the app
          playing to it.
        </p>
      </section>

      <ListEditor title="Spotify Connect" icon={<Music size={15} className="text-muted" />} kind="spotify"
        map={map} leftLabel="Name in the Spotify app" label={label} />
      <ListEditor title="AirPlay" icon={<Radio size={15} className="text-muted" />} kind="airplay"
        map={map} leftLabel="Name on Apple devices" label={label} />
      {map.inputs.length > 0 && (
        <ListEditor title="Input routes" icon={<Cable size={15} className="text-muted" />} kind="routes"
          map={map} leftLabel="Input" label={label}
          hint="A route plays the input live on the output, mixed with anything else playing there." />
      )}
    </div>
  );
}

function ListEditor({
  title, icon, kind, map, leftLabel, label, hint,
}: {
  title: string;
  icon: React.ReactNode;
  kind: Kind;
  map: AudioMap;
  leftLabel: string;
  label: (id: string) => string;
  hint?: string;
}) {
  const fromMap = rowsOf(map, kind);
  const key = JSON.stringify(fromMap);
  const [rows, setRows] = useState<Row[]>(fromMap);
  const [base, setBase] = useState(key);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  // A save elsewhere (another page, the API) replaces our copy.
  useEffect(() => {
    if (key !== base) {
      setBase(key);
      setRows(JSON.parse(key));
    }
  }, [key, base]);

  const isRoute = kind === 'routes';
  const dirty = JSON.stringify(rows) !== base;
  const bad = rows.find((r) => !r.left.trim() || !r.right);
  const instKind: AudioInstance['kind'] = isRoute ? 'route' : kind;
  const isUp = (r: Row) =>
    map.instances.some((i) => i.kind === instKind && i.running && (i.name ?? i.input) === r.left.trim() && i.output === r.right);
  const set = (i: number, p: Partial<Row>) => setRows(rows.map((r, j) => (j === i ? { ...r, ...p } : r)));

  const save = async () => {
    setBusy(true);
    setErr(null);
    const next: AudioEndpoints = { spotify: map.spotify, airplay: map.airplay, routes: map.routes };
    if (isRoute) next.routes = rows.map((r) => ({ input: r.left, output: r.right }));
    else next[kind] = rows.map((r) => ({ name: r.left.trim(), output: r.right }));
    try {
      await rest.saveEndpoints(next);
      setBase(JSON.stringify(rows));
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        {icon}
        {title}
        <span className="ml-auto text-xs font-normal text-muted">{rows.length}</span>
      </h2>
      {hint && <p className="mb-3 text-xs text-muted">{hint}</p>}

      <div className="space-y-2">
        {rows.map((r, i) => (
          <div key={i} className="flex flex-wrap items-center gap-2">
            {isUp(r) ? (
              <CircleDot size={13} className="text-live" aria-label="running" />
            ) : (
              <Circle size={13} className="text-muted" aria-label="not running" />
            )}
            {isRoute ? (
              <select value={r.left} onChange={(e) => set(i, { left: e.target.value })} aria-label={leftLabel}
                className="hair rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50">
                {!r.left && <option value="">Input…</option>}
                {map.inputs.map((l) => (
                  <option key={l.id} value={l.id}>{l.label}</option>
                ))}
              </select>
            ) : (
              <input value={r.left} onChange={(e) => set(i, { left: e.target.value })} placeholder={leftLabel}
                aria-label={leftLabel} maxLength={64}
                className="hair min-w-0 flex-1 rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50" />
            )}
            <span className="text-xs text-muted">→</span>
            <select value={r.right} onChange={(e) => set(i, { right: e.target.value })} aria-label="Output"
              className="hair rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50">
              {!r.right && <option value="">Output…</option>}
              {map.outputs.map((o) => (
                <option key={o.id} value={o.id}>{label(o.id)}</option>
              ))}
            </select>
            <button onClick={() => setRows(rows.filter((_, j) => j !== i))}
              className="rounded-lg p-2 text-muted hover:text-ink" aria-label="Remove" title="Remove">
              <Trash2 size={14} />
            </button>
          </div>
        ))}
        {!rows.length && <p className="text-xs text-muted">None.</p>}
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          onClick={() => setRows([...rows, { left: isRoute ? map.inputs[0]?.id ?? '' : '', right: map.outputs[0]?.id ?? '' }])}
          className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-3 py-1.5 text-xs text-ink hover:border-accent/50">
          <Plus size={13} /> Add
        </button>
        <button disabled={!dirty || !!bad || busy} onClick={save}
          className="rounded-lg bg-accent/20 px-3 py-1.5 text-xs transition hover:bg-accent/30 disabled:opacity-40">
          {busy ? 'Saving…' : 'Save'}
        </button>
        {dirty && bad && <span className="text-xs text-alarm">Every row needs a name and an output.</span>}
        {err && <span className="text-xs text-alarm">{err}</span>}
      </div>
    </section>
  );
}
