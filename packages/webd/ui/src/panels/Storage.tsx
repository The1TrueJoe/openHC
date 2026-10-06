import { useCallback, useEffect, useRef, useState } from 'react';
import {
  HardDrive, Usb, Share2, Copy, Check, ArrowUpFromLine, Lock, Folder, File as FileIcon, FolderPlus, Upload, Download,
  Pencil, Trash2, ChevronRight, FolderOpen,
} from 'lucide-react';
import { io, rest, type Capabilities, type FileEntry, type StorageShare, type StorageVolume } from '../api';
import { useIoState } from '../App';

/* Storage: drives plugged into the box, and the network share they are on.

   ohc-storaged mounts every drive on an external bus (USB; the HC-800's eSATA
   jack) as it is plugged in and serves it over SMB, which macOS and Windows
   both open natively. Volumes come and go live over MQTT; the share settings
   are configuration (REST). Music on the drives is played from the Audio page
   on boards that have audio. */
export function StoragePanel({ caps }: { caps: Capabilities }) {
  const live = useIoState().storage;
  const volumes = live?.volumes ?? caps.storage?.volumes ?? [];
  const share = live?.share ?? caps.storage?.share;
  return (
    <div className="space-y-4">
      <Drives volumes={volumes} />
      {volumes.length > 0 && <Files volumes={volumes} />}
      {share && <Share share={share} volumes={volumes} initialPassword={caps.storage?.share.initial_password} />}
    </div>
  );
}

const gb = (b: number) => (b >= 1e12 ? `${(b / 1e12).toFixed(1)} TB` : b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

function Drives({ volumes }: { volumes: StorageVolume[] }) {
  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <HardDrive size={15} className="text-muted" />
        Drives
      </h2>
      {volumes.length ? (
        <div className="space-y-2">
          {volumes.map((v) => (
            <VolumeRow key={v.id} v={v} />
          ))}
        </div>
      ) : (
        <p className="text-xs text-muted">
          Nothing plugged in. Plug in a USB drive and it appears here and on the network within a few seconds.
        </p>
      )}
    </section>
  );
}

function VolumeRow({ v }: { v: StorageVolume }) {
  const [busy, setBusy] = useState(false);
  const used = v.used_bytes ?? 0;
  const pct = v.size_bytes ? Math.min(100, (used / v.size_bytes) * 100) : 0;
  return (
    <div className="hair flex items-center gap-3 rounded-xl border bg-raised p-3">
      {v.bus === 'usb' ? <Usb size={18} className="shrink-0 text-muted" /> : <HardDrive size={18} className="shrink-0 text-muted" />}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-sm">
          <span className="truncate">{v.name}</span>
          {v.read_only && <span className="flex items-center gap-1 text-xs text-warm"><Lock size={11} /> read-only</span>}
        </div>
        <div className="truncate text-xs text-muted" title={v.mount}>
          {[v.drive, v.fs.toUpperCase(), v.bus === 'esata' ? 'eSATA' : 'USB'].filter(Boolean).join(' · ')}
        </div>
        <div className="mt-1.5 flex items-center gap-2">
          <div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-edge">
            <div className="h-full rounded-full bg-accent" style={{ width: `${pct}%` }} />
          </div>
          <span className="shrink-0 text-xs tabular-nums text-muted">
            {v.used_bytes !== undefined ? `${gb(used)} of ${gb(v.size_bytes)}` : gb(v.size_bytes)}
          </span>
        </div>
      </div>
      <button
        disabled={busy}
        onClick={() => {
          setBusy(true);
          try { io.ejectVolume(v.id); } catch { setBusy(false); }
        }}
        title="Unshare and unmount, so the drive can be pulled out"
        className="hair flex shrink-0 items-center gap-1.5 rounded-lg border bg-panel px-2.5 py-1.5 text-xs hover:border-accent/50 disabled:opacity-40">
        <ArrowUpFromLine size={13} /> {busy ? 'Ejecting…' : 'Eject'}
      </button>
    </div>
  );
}

function CopyLine({ label, value }: { label: string; value: string }) {
  const [done, setDone] = useState(false);
  return (
    <div className="flex items-center gap-2 text-sm">
      <span className="w-20 shrink-0 text-xs text-muted">{label}</span>
      <code className="hair min-w-0 flex-1 truncate rounded-lg border bg-raised px-2 py-1 font-mono text-xs">{value}</code>
      <button
        onClick={() => {
          navigator.clipboard?.writeText(value).then(() => {
            setDone(true);
            setTimeout(() => setDone(false), 1200);
          });
        }}
        className="rounded-md p-1 text-muted hover:text-ink" title="Copy" aria-label={`Copy the ${label} address`}>
        {done ? <Check size={14} className="text-live" /> : <Copy size={14} />}
      </button>
    </div>
  );
}

function Share({
  share, volumes, initialPassword,
}: {
  share: StorageShare;
  volumes: StorageVolume[];
  /** Generated on first start; shown until a password is set. */
  initialPassword?: string;
}) {
  const [password, setPassword] = useState('');
  const [changed, setChanged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);

  const save = async (u: Parameters<typeof rest.saveShare>[0], done: string) => {
    setBusy(true);
    setMsg(null);
    try {
      await rest.saveShare(u);
      setMsg({ ok: true, text: done });
      setPassword('');
      if (u.password) setChanged(true);
    } catch (e) {
      setMsg({ ok: false, text: e instanceof Error ? e.message : String(e) });
    } finally {
      setBusy(false);
    }
  };

  const first = volumes[0]?.name;
  return (
    <section className="hair rounded-xl border bg-panel p-4">
      <h2 className="mb-3 flex items-center gap-2 text-sm font-medium">
        <Share2 size={15} className="text-muted" />
        Network share
        <label className="ml-auto flex items-center gap-2 text-xs font-normal text-muted">
          <input type="checkbox" checked={share.enabled} disabled={busy}
            onChange={(e) => save({ enabled: e.target.checked }, e.target.checked ? 'Sharing on' : 'Sharing off')}
            className="accent-accent" />
          Share drives
        </label>
      </h2>

      <div className="space-y-2">
        <CopyLine label="Mac" value={first ? `${share.smb}/${encodeURIComponent(first)}` : share.smb} />
        <CopyLine label="Windows" value={first ? `${share.windows}\\${first}` : share.windows} />
      </div>
      <p className="mt-2 text-xs text-muted">
        On a Mac: Finder → Go → Connect to Server, or the box under Network in the sidebar. On Windows: paste
        the address into File Explorer. Each drive is its own share.
      </p>

      <div className="mt-4 grid gap-3 sm:grid-cols-2">
        <div>
          <div className="mb-1 text-xs text-muted">Login</div>
          <div className="hair rounded-lg border bg-raised px-2.5 py-2 font-mono text-sm">{share.user}</div>
          {initialPassword && !changed && (
            <p className="mt-2 text-xs text-muted">
              Password: <code className="font-mono text-ink">{initialPassword}</code> — generated for this box on
              first start. Set your own to replace it.
            </p>
          )}
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (password) save({ password }, 'Password changed');
          }}>
          <div className="mb-1 text-xs text-muted">New password</div>
          <div className="flex gap-2">
            <input type="password" value={password} onChange={(e) => setPassword(e.target.value)} maxLength={128}
              autoComplete="new-password" placeholder="••••••"
              className="hair min-w-0 flex-1 rounded-lg border bg-raised p-2 text-sm text-ink outline-none focus:border-accent/50" />
            <button disabled={!password || busy}
              className="rounded-lg bg-accent/20 px-3 py-1.5 text-xs transition hover:bg-accent/30 disabled:opacity-40">
              Set
            </button>
          </div>
        </form>
      </div>

      <label className="mt-3 flex items-center gap-2 text-xs text-muted">
        <input type="checkbox" checked={share.guest} disabled={busy}
          onChange={(e) => save({ guest: e.target.checked }, e.target.checked ? 'Guest access on' : 'Guest access off')}
          className="accent-accent" />
        Allow guest access (no password). Macs connect as Guest; Windows 11 refuses guest logins unless that is
        turned on in Windows too.
      </label>
      {msg && <p className={`mt-2 text-xs ${msg.ok ? 'text-live' : 'text-alarm'}`}>{msg.text}</p>}
    </section>
  );
}

/* ── the file browser ────────────────────────────────────────────────────────

   Folders and files on the mounted drives (ohc-storaged's /api/files, confined
   to the drives): open folders, download, rename, delete, make folders, and
   upload — drop files on the panel or pick them; each streams to the drive with
   its own progress bar. */

type Upload = { name: string; progress: number; error?: string };

const when = (s: number) =>
  s ? new Date(s * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '';

function Files({ volumes }: { volumes: StorageVolume[] }) {
  const [vol, setVol] = useState(volumes[0].name);
  const [dir, setDir] = useState('');
  const [items, setItems] = useState<FileEntry[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [uploads, setUploads] = useState<Upload[]>([]);
  const [dragging, setDragging] = useState(false);
  const picker = useRef<HTMLInputElement>(null);

  // A drive that was ejected takes its tab with it.
  useEffect(() => {
    if (!volumes.some((v) => v.name === vol)) {
      setVol(volumes[0].name);
      setDir('');
    }
  }, [volumes, vol]);

  const here = dir ? `${vol}/${dir}` : vol;
  const readOnly = volumes.find((v) => v.name === vol)?.read_only ?? false;
  const reload = useCallback(() => {
    setErr(null);
    rest.files(here).then(setItems).catch((e) => { setItems([]); setErr(e instanceof Error ? e.message : String(e)); });
  }, [here]);
  useEffect(reload, [reload]);

  const act = async (p: Promise<unknown>) => {
    try { await p; } catch (e) { setErr(e instanceof Error ? e.message : String(e)); }
    reload();
  };

  const upload = async (files: FileList | File[]) => {
    const list = Array.from(files);
    if (!list.length || readOnly) return;
    const target = here;
    setUploads((u) => [...u, ...list.map((f) => ({ name: f.name, progress: 0 }))]);
    for (const f of list) {
      const set = (p: Partial<Upload>) => setUploads((u) => u.map((x) => (x.name === f.name ? { ...x, ...p } : x)));
      try {
        await rest.fileUpload(target, f, (progress) => set({ progress }));
        set({ progress: 1 });
        setTimeout(() => setUploads((u) => u.filter((x) => x.name !== f.name || x.error)), 1500);
      } catch (e) {
        set({ error: e instanceof Error ? e.message : String(e) });
      }
      reload();
    }
  };

  const crumbs = dir ? dir.split('/') : [];
  const rowBtn = 'rounded-md p-1 text-muted opacity-60 hover:text-ink group-hover:opacity-100';
  return (
    <section
      className={`hair rounded-xl border bg-panel p-4 ${dragging ? 'border-accent/60' : ''}`}
      onDragOver={(e) => { if (!readOnly) { e.preventDefault(); setDragging(true); } }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => { e.preventDefault(); setDragging(false); upload(e.dataTransfer.files); }}
    >
      <h2 className="mb-3 flex flex-wrap items-center gap-2 text-sm font-medium">
        <FolderOpen size={15} className="text-muted" />
        Files
        <div className="ml-2 flex flex-wrap gap-1">
          {volumes.map((v) => (
            <button key={v.name} onClick={() => { setVol(v.name); setDir(''); }}
              className={`rounded-lg px-2.5 py-1 text-xs font-normal ${v.name === vol ? 'bg-accent/15 text-ink' : 'text-muted hover:text-ink'}`}>
              {v.name}
            </button>
          ))}
        </div>
        <div className="ml-auto flex gap-1.5 font-normal">
          <button disabled={readOnly}
            onClick={() => {
              const n = prompt('New folder name');
              if (n) act(rest.fileMkdir(`${here}/${n}`));
            }}
            className="hair flex items-center gap-1.5 rounded-lg border bg-raised px-2.5 py-1.5 text-xs hover:border-accent/50 disabled:opacity-40">
            <FolderPlus size={13} /> New folder
          </button>
          <button disabled={readOnly} onClick={() => picker.current?.click()}
            className="flex items-center gap-1.5 rounded-lg bg-accent/20 px-2.5 py-1.5 text-xs hover:bg-accent/30 disabled:opacity-40">
            <Upload size={13} /> Upload
          </button>
          <input ref={picker} type="file" multiple hidden
            onChange={(e) => { if (e.target.files) upload(e.target.files); e.target.value = ''; }} />
        </div>
      </h2>

      <div className="mb-2 flex flex-wrap items-center gap-1 text-xs">
        <button onClick={() => setDir('')} className="text-muted hover:text-ink">{vol}</button>
        {crumbs.map((c, i) => (
          <span key={i} className="flex items-center gap-1">
            <ChevronRight size={12} className="text-muted" />
            <button onClick={() => setDir(crumbs.slice(0, i + 1).join('/'))} className="max-w-48 truncate hover:text-ink">{c}</button>
          </span>
        ))}
        {readOnly && <span className="ml-2 flex items-center gap-1 text-warm"><Lock size={11} /> read-only</span>}
      </div>

      {uploads.length > 0 && (
        <div className="mb-2 space-y-1.5">
          {uploads.map((u) => (
            <div key={u.name} className="flex items-center gap-2 text-xs">
              <Upload size={12} className="shrink-0 text-muted" />
              <span className="w-40 truncate sm:w-60">{u.name}</span>
              {u.error ? (
                <span className="text-alarm">{u.error}</span>
              ) : (
                <div className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-edge">
                  <div className="h-full rounded-full bg-accent transition-all" style={{ width: `${Math.round(u.progress * 100)}%` }} />
                </div>
              )}
            </div>
          ))}
        </div>
      )}
      {err && <p className="mb-2 text-xs text-alarm">{err}</p>}

      <div className="max-h-[28rem] overflow-auto">
        {items?.length === 0 && !err && (
          <p className="py-6 text-center text-xs text-muted">
            {readOnly ? 'Empty folder.' : 'Empty folder. Drop files here to upload them.'}
          </p>
        )}
        {items?.map((e) => {
          const path = `${here}/${e.name}`;
          return (
            <div key={e.name} className="group flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm hover:bg-raised">
              {e.dir ? <Folder size={15} className="shrink-0 text-accent" /> : <FileIcon size={15} className="shrink-0 text-muted" />}
              {e.dir ? (
                <button className="min-w-0 flex-1 truncate text-left" onClick={() => setDir(dir ? `${dir}/${e.name}` : e.name)}>
                  {e.name}
                </button>
              ) : (
                <a className="min-w-0 flex-1 truncate" href={rest.fileDownloadUrl(path)} download={e.name}>{e.name}</a>
              )}
              <span className="hidden w-20 shrink-0 text-right text-xs tabular-nums text-muted sm:block">{e.dir ? '' : gb(e.size)}</span>
              <span className="hidden w-36 shrink-0 text-right text-xs text-muted md:block">{when(e.modified)}</span>
              {!e.dir && (
                <a href={rest.fileDownloadUrl(path)} download={e.name} className={rowBtn} title="Download" aria-label={`Download ${e.name}`}>
                  <Download size={13} />
                </a>
              )}
              {!readOnly && (
                <>
                  <button className={rowBtn} title="Rename" aria-label={`Rename ${e.name}`}
                    onClick={() => {
                      const n = prompt('Rename to', e.name);
                      if (n && n !== e.name) act(rest.fileRename(path, n));
                    }}>
                    <Pencil size={13} />
                  </button>
                  <button className={rowBtn} title="Delete" aria-label={`Delete ${e.name}`}
                    onClick={() => {
                      if (confirm(e.dir ? `Delete the folder "${e.name}" and everything in it?` : `Delete "${e.name}"?`)) act(rest.fileDelete(path));
                    }}>
                    <Trash2 size={13} />
                  </button>
                </>
              )}
            </div>
          );
        })}
      </div>
    </section>
  );
}
