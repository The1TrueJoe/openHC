import { useEffect, useRef, useState } from 'react';
import {
  Music, Speaker, Radio, Volume2, CircleDot, Circle, Plus, Trash2, Cable, Bell, Workflow, ChevronRight, Library,
  Play, Pause, SkipBack, SkipForward, Square, Folder, FileAudio, Search, Shuffle, Repeat, RefreshCw,
} from 'lucide-react';
import {
  io, rest, audioCover, type AudioEndpoints, type AudioInstance, type AudioMap, type AudioMeta, type AudioReceiver,
  type AudioStatus, type Capabilities, type LibraryEntry, type LibraryState,
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
  if (map && Array.isArray(map.outputs) && Array.isArray(map.instances))
    return (
      <MapPanel map={map} level={live?.level ?? {}} announcing={live?.announcing ?? {}} meta={live?.meta ?? {}}
        library={live?.library} />
    );

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
type Kind = 'spotify' | 'airplay' | 'routes' | 'library';

const rowsOf = (map: AudioMap, kind: Kind): Row[] =>
  kind === 'routes'
    ? map.routes.map((r) => ({ left: r.input, right: r.output }))
    : (map[kind] ?? []).map((e) => ({ left: e.name, right: e.output }));

function MapPanel({
  map, level, announcing, meta, library,
}: {
  map: AudioMap;
  level: Record<string, number>;
  announcing: Record<string, boolean>;
  meta: Record<string, AudioMeta>;
  library: LibraryState | undefined;
}) {
  const label = (id: string) => map.outputs.find((o) => o.id === id)?.label ?? id;
  const lib = map.library?.[0];
  return (
    <div className="space-y-4">
      <Routing map={map} level={level} announcing={announcing} meta={meta} />
      {lib && (
        <LibraryPanel name={lib.name} output={label(lib.output)} state={library} meta={meta['library-0']} />
      )}

      {/* Configuration, folded away: the diagram above already shows every
          endpoint and where it plays, so this is only for changing that. */}
      <details className="hair group rounded-xl border bg-panel">
        <summary className="flex cursor-pointer list-none items-center gap-2 p-4 text-sm font-medium">
          <ChevronRight size={15} className="text-muted transition group-open:rotate-90" />
          Edit endpoints
          <span className="ml-auto text-xs font-normal text-muted">
            add, rename or move Spotify, AirPlay and input routes
          </span>
        </summary>
        <div className="space-y-5 px-4 pb-4">
          <ListEditor title="Spotify Connect" icon={<Music size={15} className="text-muted" />} kind="spotify"
            map={map} leftLabel="Name in the Spotify app" label={label} />
          <ListEditor title="AirPlay" icon={<Radio size={15} className="text-muted" />} kind="airplay"
            map={map} leftLabel="Name on Apple devices" label={label} />
          <ListEditor title="Library player" icon={<Library size={15} className="text-muted" />} kind="library"
            map={map} leftLabel="Name" label={label} max={1}
            hint="Plays music from the drives plugged into the box (USB, eSATA). One player; pick its output." />
          {map.inputs.length > 0 && (
            <ListEditor title="Input routes" icon={<Cable size={15} className="text-muted" />} kind="routes"
              map={map} leftLabel="Input" label={label}
              hint="A route plays the input live on the output, mixed with anything else playing there." />
          )}
        </div>
      </details>
    </div>
  );
}

/* ── routing ────────────────────────────────────────────────────────────────

   Who plays where, drawn: every source (Spotify and AirPlay endpoints, input
   routes) on the left, every output on the right with its level, and a line
   from each source to the output it plays on — lit while that source is
   running. An output's level is the one volume of the jack: the slider here,
   `cmd/audio/level/<id>`, and the AirPlay/Spotify volume sliders all move the
   same control, so whichever moved last is what shows. */

const SRC_ROW = 52;
const OUT_ROW = 84;
const LINK_W = 72;

type Source = {
  key: string; kind: 'spotify' | 'airplay' | 'route' | 'library'; name: string;
  /** '' = an input that is not routed anywhere. */
  output: string;
  running: boolean; playing: boolean; tag?: string;
};

/** Every source, ordered by the output it plays on so the lines fan in
 *  instead of crossing. */
function sourcesOf(map: AudioMap): Source[] {
  const inst = (kind: AudioInstance['kind'], name: string, output: string) =>
    map.instances.find((i) => i.kind === kind && (i.name ?? i.input) === name && i.output === output);
  const src = (key: string, kind: Source['kind'], name: string, match: string, output: string): Source => {
    const i = inst(kind, match, output);
    return { key, kind, name, output, running: !!i?.running, playing: !!i?.running && !!i?.playing, tag: i?.tag };
  };
  const inputLabel = (id: string) => map.inputs.find((i) => i.id === id)?.label ?? id;
  const order = (id: string) => {
    const j = map.outputs.findIndex((o) => o.id === id);
    return j < 0 ? map.outputs.length : j;
  };
  return [
    ...map.spotify.map((e, n) => src(`s${n}`, 'spotify', e.name, e.name, e.output)),
    ...map.airplay.map((e, n) => src(`a${n}`, 'airplay', e.name, e.name, e.output)),
    ...(map.library ?? []).map((e, n) => src(`l${n}`, 'library', e.name, e.name, e.output)),
    ...map.routes.map((r, n) => src(`r${n}`, 'route', inputLabel(r.input), r.input, r.output)),
    // Inputs with no route still show, so the jack is visible.
    ...map.inputs
      .filter((i) => !map.routes.some((r) => r.input === i.id))
      .map((i): Source => ({ key: `i${i.id}`, kind: 'route', name: i.label, output: '', running: false, playing: false })),
  ].sort((a, b) => order(a.output) - order(b.output));
}

const KIND_ICON = { spotify: Music, airplay: Radio, route: Cable, library: Library } as const;

function Routing({
  map, level, announcing, meta,
}: {
  map: AudioMap;
  level: Record<string, number>;
  announcing: Record<string, boolean>;
  meta: Record<string, AudioMeta>;
}) {
  const sources = sourcesOf(map);
  const height = Math.max(sources.length * SRC_ROW, map.outputs.length * OUT_ROW, OUT_ROW);
  // Both columns are centred in the same height, so a row's centre is
  // computable rather than measured.
  const srcTop = (height - sources.length * SRC_ROW) / 2;
  const outTop = (height - map.outputs.length * OUT_ROW) / 2;
  const outY = (id: string) => {
    const j = map.outputs.findIndex((o) => o.id === id);
    return j < 0 ? null : outTop + j * OUT_ROW + OUT_ROW / 2;
  };

  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Workflow size={15} className="text-muted" />
        Routing
      </h2>
      <div className="flex" style={{ height }}>
        <div className="min-w-0 flex-1" style={{ paddingTop: srcTop }}>
          {sources.map((s) => (
            <SourceRow key={s.key} s={s} meta={s.tag ? meta[s.tag] : undefined} />
          ))}
          {!sources.length && <p className="text-xs text-muted">Nothing is set up to play yet.</p>}
        </div>

        <svg width={LINK_W} height={height} className="shrink-0" aria-hidden="true">
          {sources.map((s, i) => {
            const y2 = s.output ? outY(s.output) : null;
            if (y2 === null) return null;
            const y1 = srcTop + i * SRC_ROW + SRC_ROW / 2;
            return (
              // Playing: solid and lit. Idle (running, nothing playing): faint.
              // Not running: dashed.
              <path
                key={s.key}
                d={`M0 ${y1} C ${LINK_W / 2} ${y1}, ${LINK_W / 2} ${y2}, ${LINK_W} ${y2}`}
                fill="none"
                strokeWidth={s.playing ? 2.25 : 1.25}
                strokeDasharray={s.running ? undefined : '3 3'}
                style={{ stroke: s.playing ? 'var(--live)' : 'var(--muted)', opacity: s.playing ? 1 : 0.4 }}
              />
            );
          })}
        </svg>

        <div className="w-60 shrink-0 sm:w-72" style={{ paddingTop: outTop }}>
          {map.outputs.map((o) => (
            <OutputRow key={o.id} id={o.id} label={o.label} device={o.device}
              level={level[o.id]} announcing={!!announcing[o.id]} />
          ))}
        </div>
      </div>
      <p className="mt-3 text-xs text-muted">
        Lit lines are playing. Sources on one output mix; each output has one level, which the
        AirPlay and Spotify volume sliders move too. The bell plays a chime over the output,
        ducking its music.
      </p>
    </section>
  );
}

function SourceRow({ s, meta }: { s: Source; meta: AudioMeta | undefined }) {
  const Icon = KIND_ICON[s.kind];
  const unrouted = s.output === '';
  const track = meta?.title ? [meta.title, meta.artist].filter(Boolean).join(' — ') : '';
  const status = unrouted ? 'not routed' : s.playing ? 'playing' : meta?.state === 'paused' ? 'paused' : s.running ? 'idle' : 'not running';
  const tip = [s.name, track, meta?.album, meta?.client && `from ${meta.client}`, status].filter(Boolean).join('\n');
  // A cover that will not load (offline box, expired CDN link) falls back to
  // the source's icon rather than a broken-image glyph.
  const [badCover, setBadCover] = useState<string | null>(null);
  const cover = meta?.cover && meta.cover !== badCover ? meta.cover : null;
  return (
    <div className="flex items-center pr-1" style={{ height: SRC_ROW }}>
      <div title={tip}
        className={`hair flex h-11 min-w-0 flex-1 items-center gap-2 rounded-lg border bg-raised px-2 text-xs ${s.running || track ? '' : 'opacity-50'}`}>
        {cover ? (
          <img src={audioCover(cover)} alt="" onError={() => setBadCover(cover)}
            className="size-8 shrink-0 rounded object-cover" />
        ) : (
          <div className="flex size-8 shrink-0 items-center justify-center">
            <Icon size={14} className={s.playing ? 'text-live' : 'text-muted'} />
          </div>
        )}
        <div className="min-w-0 flex-1 leading-tight">
          {track ? (
            <>
              <div className="truncate text-ink">{track}</div>
              <div className="truncate text-muted">
                {s.name}{meta?.client ? ` · ${meta.client}` : ''}
              </div>
            </>
          ) : (
            <div className="truncate">{s.name}</div>
          )}
        </div>
        {(s.playing || meta?.state === 'paused' || unrouted) && (
          <span className={`shrink-0 ${s.playing ? 'text-live' : 'text-muted'}`}>{status}</span>
        )}
      </div>
    </div>
  );
}

function OutputRow({
  id, label, device, level, announcing,
}: {
  id: string;
  label: string;
  device: string;
  level: number | undefined;
  announcing: boolean;
}) {
  // Local while dragging, so the thumb follows the pointer; sent at most every
  // 120 ms while it moves and once on release, then the retained state takes
  // over again.
  const [drag, setDrag] = useState<number | null>(null);
  const sent = useRef(0);
  const shown = drag ?? level ?? 0;
  const send = (v: number, force = false) => {
    const now = Date.now();
    if (!force && now - sent.current < 120) return;
    sent.current = now;
    try { io.setAudioLevel(id, v); } catch { /* not connected: the page says so */ }
  };
  const release = () => {
    if (drag !== null) send(drag, true);
    setDrag(null);
  };

  return (
    <div className="flex items-center pl-1" style={{ height: OUT_ROW }}>
      <div className={`hair w-full rounded-xl border bg-raised p-2.5 ${announcing ? 'border-warm/60' : ''}`}>
        <div className="flex items-center gap-2 text-sm">
          <Speaker size={14} className="shrink-0 text-muted" />
          <span className="truncate" title={device}>{label}</span>
          {announcing && <span className="text-xs text-warm">announcing</span>}
          <button
            onClick={() => { try { io.announce(id); } catch { /* not connected */ } }}
            className="ml-auto rounded-md p-1 text-muted hover:text-ink"
            title="Play a chime over this output" aria-label={`Announce on ${label}`}>
            <Bell size={14} />
          </button>
        </div>
        <div className="mt-1.5 flex items-center gap-2">
          <Volume2 size={13} className="shrink-0 text-muted" />
          <input
            type="range" min={0} max={100} value={shown} disabled={level === undefined}
            aria-label={`${label} level`}
            onChange={(e) => { const v = Number(e.target.value); setDrag(v); send(v); }}
            onPointerUp={release} onKeyUp={release} onBlur={release}
            className="min-w-0 flex-1 accent-accent"
          />
          <span className="w-9 text-right text-xs tabular-nums">{level === undefined ? '–' : `${shown}%`}</span>
        </div>
      </div>
    </div>
  );
}

/* ── the library player ─────────────────────────────────────────────────────

   mpd over the drives on /media: what it is playing (with the transport), and
   a browser to find something else. Live state is MQTT (`state/audio/library`,
   `state/audio/meta/library-0`); browsing is a one-shot REST read per folder.
   Its volume is its output's level, in the routing diagram above. */

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

function LibraryPanel({
  name, output, state, meta,
}: {
  name: string;
  output: string;
  state: LibraryState | undefined;
  meta: AudioMeta | undefined;
}) {
  const send = (verb: string, arg = '') => { try { io.library(verb, arg); } catch { /* not connected */ } };
  const elapsed = useElapsed(state);
  const playing = state?.state === 'play';
  const offline = !state || state.state === 'offline';
  const [badCover, setBadCover] = useState<string | null>(null);
  const cover = meta?.cover && meta.cover !== badCover ? meta.cover : null;
  const btn = 'rounded-lg p-2 text-muted hover:text-ink disabled:opacity-30';

  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Library size={15} className="text-muted" />
        {name}
        <span className="text-xs font-normal text-muted">→ {output}</span>
        {state?.updating && (
          <span className="ml-auto flex items-center gap-1 text-xs font-normal text-muted">
            <RefreshCw size={12} className="animate-spin" /> indexing drives
          </span>
        )}
      </h2>

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
    </section>
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

function ListEditor({
  title, icon, kind, map, leftLabel, label, hint, max,
}: {
  title: string;
  icon: React.ReactNode;
  kind: Kind;
  map: AudioMap;
  leftLabel: string;
  label: (id: string) => string;
  hint?: string;
  /** At most this many rows (the library player: one). */
  max?: number;
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
    const next: AudioEndpoints = { spotify: map.spotify, airplay: map.airplay, routes: map.routes, library: map.library ?? [] };
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
    <section>
      <h3 className="mb-2 flex items-center gap-2 text-sm font-medium">
        {icon}
        {title}
        <span className="ml-auto text-xs font-normal text-muted">{rows.length}</span>
      </h3>
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
          disabled={max !== undefined && rows.length >= max}
          onClick={() => setRows([...rows, { left: isRoute ? map.inputs[0]?.id ?? '' : '', right: map.outputs[0]?.id ?? '' }])}
          className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-3 py-1.5 text-xs text-ink hover:border-accent/50 disabled:opacity-40">
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
