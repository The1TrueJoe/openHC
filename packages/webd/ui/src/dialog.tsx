// The UI's own confirm and prompt: the browser's are unstyled and block the
// page. `await ask(...)` / `await askText(...)` work like confirm()/prompt(),
// drawn by the one <Dialogs/> host in main.tsx (like the toasts).
import { useEffect, useState } from 'react';

type Req = {
  title: string; body?: string; ok: string; danger?: boolean;
  text?: string; // set for askText: the field's starting value
  done: (v: string | boolean | null) => void;
};
let show: ((r: Req | null) => void) | null = null;

const open = (r: Omit<Req, 'done'>) =>
  new Promise<string | boolean | null>((done) => (show ? show({ ...r, done }) : done(null)));

/** A yes/no question. `danger` paints the button red (deletes, removals). */
export const ask = (title: string, o: { body?: string; ok?: string; danger?: boolean } = {}) =>
  open({ title, ok: o.ok ?? 'OK', ...o }).then((v) => v === true);

/** A one-line text answer, or null when cancelled or left empty. */
export const askText = (title: string, value = '', ok = 'OK') =>
  open({ title, ok, text: value }).then((v) => (typeof v === 'string' && v.trim() ? v.trim() : null));

export function Dialogs() {
  const [r, setR] = useState<Req | null>(null);
  const [text, setText] = useState('');
  useEffect(() => { show = (n) => { setR(n); setText(n?.text ?? ''); }; return () => { show = null; }; }, []);
  if (!r) return null;
  const close = (v: string | boolean | null) => { r.done(v); setR(null); };
  const yes = () => close(r.text !== undefined ? text : true);
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50 p-4" onMouseDown={() => close(null)}>
      <div
        role="alertdialog"
        aria-modal="true"
        aria-label={r.title}
        className="hair w-full max-w-sm rounded-2xl border bg-panel p-5 shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
        // Stops here so a dialog underneath (the device sheet) keeps its own Escape.
        onKeyDown={(e) => { e.stopPropagation(); if (e.key === 'Escape') close(null); }}
      >
        <h2 className="text-sm font-semibold">{r.title}</h2>
        {r.body && <p className="mt-1.5 text-xs text-muted">{r.body}</p>}
        <form onSubmit={(e) => { e.preventDefault(); yes(); }}>
          {r.text !== undefined && (
            <input
              autoFocus
              value={text}
              onChange={(e) => setText(e.target.value)}
              onFocus={(e) => e.target.select()}
              className="hair mt-3 w-full rounded-lg border bg-raised px-2.5 py-1.5 text-sm outline-none focus:border-accent/50"
            />
          )}
          <div className="mt-4 flex justify-end gap-2">
            <button type="button" onClick={() => close(null)}
              className="hair rounded-lg border px-3 py-1.5 text-sm text-ink hover:border-accent/50">Cancel</button>
            <button type="submit" autoFocus={r.text === undefined}
              className={`rounded-lg px-3 py-1.5 text-sm font-medium text-white hover:opacity-90 ${r.danger ? 'bg-alarm' : 'bg-accent'}`}>
              {r.ok}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
