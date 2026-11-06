// Every error in the UI is a toast: one place, the same look, never stale red
// text left behind in a panel. Successes stay where they happened.
import { Toaster, toast } from 'sonner';

export { toast };

export function errorText(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  try { return JSON.stringify(e); } catch { return String(e); }
}

/** An error toast: `title` says what failed, the error says why. */
export function toastError(e: unknown, title?: string, id?: string) {
  const text = errorText(e);
  toast.error(title ?? text, { id, description: title ? text : undefined });
}

export function Toasts() {
  return (
    <Toaster
      position="bottom-right"
      closeButton
      richColors
      toastOptions={{ classNames: { description: 'ohc-toast-desc' } }}
      duration={8000}
      style={{
        '--normal-bg': 'var(--panel)',
        '--normal-text': 'var(--ink)',
        '--normal-border': 'var(--edge)',
        '--error-bg': 'var(--panel)',
        '--error-text': 'var(--alarm)',
        '--error-border': 'var(--alarm)',
        '--success-bg': 'var(--panel)',
        '--success-text': 'var(--live)',
        '--success-border': 'var(--edge)',
      } as React.CSSProperties}
    />
  );
}

/** An IO command (MQTT publish): throws when the IO server is not connected.
 *  One toast id, so a dragged slider updates one toast rather than stacking. */
export function tryIo(send: () => void) {
  try { send(); } catch (e) { toastError(e, 'Not sent', 'io-send'); }
}
