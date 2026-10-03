import { useEffect, useState } from 'react';
import { Music, Speaker, Radio, Volume2, CircleDot, Circle, Plus, Trash2, Cable } from 'lucide-react';
import {
  io, rest, type AudioEndpoint, type AudioMap, type AudioReceiver, type AudioStatus, type Capabilities,
} from '../api';
import { useIoState } from '../App';

/* Audio: the ALSA output the box renders to, and the two network receivers that
   render to it. Spotify Connect (librespot) and AirPlay (shairport-sync) are
   *receivers* — a phone drives playback — so this panel does not pretend to be a
   media player. It turns the two knobs that are genuinely ours (which output,
   and the volume) and reports what is true about the receivers. Transport is
   shown only when a receiver actually exposes it, which in this image is never;
   see packages/iod/src/audio.rs for the board-side wiring that would change that.

   Structure comes over REST (/api/audio) and is polled; the fast-moving bits —
   selected output, volume, receiver running — also arrive live over MQTT, so a
   change made elsewhere (or by amixer on the box) shows up here too. */
export function AudioPanel({ caps }: { caps: Capabilities }) {
  // Seed from capabilities so the first paint is instant, then keep it fresh.
  const [a, setA] = useState<AudioStatus | null>(caps.audio ?? null);
  const live = useIoState().audio;

  useEffect(() => {
    let alive = true;
    const tick = () => rest.audio().then((d) => alive && setA(d)).catch(() => { /* keep last */ });
    tick();
    const id = setInterval(tick, 5000);
    return () => { alive = false; clearInterval(id); };
  }, []);

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
  const map = live?.map ?? a.map;
  if (map) return <MapPanel map={map} />;

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

   The jacks are fixed (board.env); what you edit is which endpoints exist and
   where each one plays. Three lists — Spotify Connect, AirPlay, and line-in
   routes — each saved as one "name@output;..." string that iod validates and
   ohc-audio persists, restarting the endpoints. */

/** Characters iod refuses (the config is shell-sourced) — caught here first so
 *  the user sees why rather than a silent no-op. */
const FORBIDDEN = /[$`\\"\n\r;@]/;

type Row = { left: string; right: string };

function parseList(list: string): Row[] {
  return list
    .split(';')
    .map((e) => e.trim())
    .filter(Boolean)
    .map((e) => {
      const at = e.lastIndexOf('@');
      return at < 0 ? { left: e, right: '' } : { left: e.slice(0, at).trim(), right: e.slice(at + 1).trim() };
    });
}

const serialise = (rows: Row[]) => rows.map((r) => `${r.left.trim()}@${r.right}`).join(';');

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

      <ListEditor
        title="Spotify Connect"
        icon={<Music size={15} className="text-muted" />}
        kind="spotify"
        list={map.spotify}
        leftLabel="Name in the Spotify app"
        lefts={null}
        outputs={map.outputs}
        running={map.endpoints.filter((e) => e.kind === 'spotify')}
        label={label}
      />
      <ListEditor
        title="AirPlay"
        icon={<Radio size={15} className="text-muted" />}
        kind="airplay"
        list={map.airplay}
        leftLabel="Name on Apple devices"
        lefts={null}
        outputs={map.outputs}
        running={map.endpoints.filter((e) => e.kind === 'airplay')}
        label={label}
      />
      {map.inputs.length > 0 && (
        <ListEditor
          title="Input routes"
          icon={<Cable size={15} className="text-muted" />}
          kind="routes"
          list={map.routes}
          leftLabel="Input"
          lefts={map.inputs}
          outputs={map.outputs}
          running={map.endpoints.filter((e) => e.kind === 'route')}
          label={label}
          hint="A route plays the input live on the output, mixed with anything else playing there."
        />
      )}
    </div>
  );
}

function ListEditor({
  title, icon, kind, list, leftLabel, lefts, outputs, running, label, hint,
}: {
  title: string;
  icon: React.ReactNode;
  kind: 'spotify' | 'airplay' | 'routes';
  list: string;
  leftLabel: string;
  lefts: AudioMap['inputs'] | null;
  outputs: AudioMap['outputs'];
  running: AudioEndpoint[];
  label: (id: string) => string;
  hint?: string;
}) {
  const [rows, setRows] = useState<Row[]>(() => parseList(list));
  const [saved, setSaved] = useState(list);
  const [busy, setBusy] = useState(false);
  // A save elsewhere (another page, MQTT) replaces our copy unless we have edits.
  useEffect(() => {
    if (list !== saved) {
      setSaved(list);
      setRows(parseList(list));
      setBusy(false);
    }
  }, [list, saved]);

  const dirty = serialise(rows) !== serialise(parseList(saved));
  const bad = rows.find((r) => !r.left.trim() || !r.right || (!lefts && FORBIDDEN.test(r.left)));
  const isUp = (r: Row, i: number) => {
    const e = running[i];
    return !!e && e.running && (e.name ?? e.input) === r.left.trim() && e.output === r.right;
  };
  const set = (i: number, p: Partial<Row>) => setRows(rows.map((r, j) => (j === i ? { ...r, ...p } : r)));

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
            {isUp(r, i) ? (
              <CircleDot size={13} className="text-live" aria-label="running" />
            ) : (
              <Circle size={13} className="text-muted" aria-label="not running" />
            )}
            {lefts ? (
              <select
                value={r.left}
                onChange={(e) => set(i, { left: e.target.value })}
                aria-label={leftLabel}
                className="hair rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50"
              >
                {!r.left && <option value="">Input…</option>}
                {lefts.map((l) => (
                  <option key={l.id} value={l.id}>{l.label}</option>
                ))}
              </select>
            ) : (
              <input
                value={r.left}
                onChange={(e) => set(i, { left: e.target.value })}
                placeholder={leftLabel}
                aria-label={leftLabel}
                className="hair min-w-0 flex-1 rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50"
              />
            )}
            <span className="text-xs text-muted">→</span>
            <select
              value={r.right}
              onChange={(e) => set(i, { right: e.target.value })}
              aria-label="Output"
              className="hair rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50"
            >
              {!r.right && <option value="">Output…</option>}
              {outputs.map((o) => (
                <option key={o.id} value={o.id}>{label(o.id)}</option>
              ))}
            </select>
            <button
              onClick={() => setRows(rows.filter((_, j) => j !== i))}
              className="rounded-lg p-2 text-muted hover:text-ink"
              aria-label="Remove"
              title="Remove"
            >
              <Trash2 size={14} />
            </button>
          </div>
        ))}
        {!rows.length && <p className="text-xs text-muted">None.</p>}
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <button
          onClick={() => setRows([...rows, { left: lefts?.[0]?.id ?? '', right: outputs[0]?.id ?? '' }])}
          className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-3 py-1.5 text-xs text-ink hover:border-accent/50"
        >
          <Plus size={13} /> Add
        </button>
        <button
          disabled={!dirty || !!bad || busy}
          onClick={() => {
            setBusy(true);
            io.setAudioEndpoints(kind, serialise(rows));
          }}
          className="rounded-lg bg-accent/20 px-3 py-1.5 text-xs transition hover:bg-accent/30 disabled:opacity-40"
        >
          {busy ? 'Applying…' : 'Save'}
        </button>
        {dirty && bad && (
          <span className="text-xs text-alarm">
            {lefts ? 'Every route needs an input and an output.' : 'Names cannot be empty or contain $ ` \\ " ; @'}
          </span>
        )}
      </div>
    </section>
  );
}
