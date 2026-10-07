import { useState } from 'react';
import { Music, Speaker, Radio, Volume2, CircleDot, Circle } from 'lucide-react';
import { io, type AudioReceiver, type AudioStatus, type Capabilities } from '../api';
import { useIoState } from '../App';
import { AudioFlow } from './AudioFlow';

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

  /* A board with named outputs gets the switcher (AudioFlow): sources wired to
     outputs, each node's settings a click away. The live map comes over MQTT
     (retained, so it is there on connect), falling back to the capabilities
     snapshot; it renders once it has the full shape — retained topics arrive
     leaf by leaf. */
  const map = live?.map ?? a.map;
  if (map && Array.isArray(map.outputs) && Array.isArray(map.instances)) return <AudioFlow map={map} live={live ?? {}} />;

  return (
    <div className="space-y-4 p-5 sm:p-7">
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

