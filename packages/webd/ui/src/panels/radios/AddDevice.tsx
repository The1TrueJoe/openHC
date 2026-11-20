import { useEffect, useRef, useState } from 'react';
import { X, Check, Loader2, ShieldCheck, ShieldAlert, CircleAlert, KeyRound, Radio, Plus, Minus } from 'lucide-react';
import { type Proto, trees, useTree, zigbeeDevices, zwaveDevices, parse } from './model';
import { label, primary, ghost } from './Radios';

/* Adding and removing devices, as a guided flow.

   Zigbee: the network opens for joining, and each device that joins is shown
   as it is set up (joined, interviewed, ready), then can be named.

   Z-Wave: an S2 device's code (the first five digits of its DSK) is asked for
   BEFORE inclusion starts, so the bridge grants the keys and checks the code
   itself, in the device's own time window. A person answering a prompt while
   the device waits is too slow: the device gives up on its key exchange in
   seconds and is left joined without security. */

type Step = 'choose' | 'zigbee' | 'zwave-code' | 'zwave-add' | 'zwave-remove';

export function AddDevice({ protos, mode, onClose }: { protos: Proto[]; mode: 'add' | 'remove'; onClose: () => void }) {
  const first: Step = mode === 'remove' ? 'zwave-remove' : protos.length === 1 ? (protos[0] === 'zigbee' ? 'zigbee' : 'zwave-code') : 'choose';
  const [step, setStep] = useState<Step>(first);
  const [pin, setPin] = useState('');

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" onMouseDown={onClose}>
      <div className="hair w-full max-w-lg overflow-hidden rounded-xl border bg-panel shadow-2xl" onMouseDown={(e) => e.stopPropagation()}>
        <div className="hair flex items-center gap-2 border-b px-4 py-3">
          <Radio size={15} className="text-muted" />
          <h2 className="text-sm font-medium">{title(step)}</h2>
          <button className="ml-auto text-muted hover:text-ink" onClick={onClose} aria-label="Close"><X size={18} /></button>
        </div>
        <div className="p-4">
          {step === 'choose' && (
            <div className="grid gap-3 sm:grid-cols-2">
              {protos.map((p) => (
                <button
                  key={p}
                  className="hair rounded-xl border bg-raised p-4 text-left hover:border-accent/50"
                  onClick={() => setStep(p === 'zigbee' ? 'zigbee' : 'zwave-code')}
                >
                  <div className="text-sm font-medium">{label(p)}</div>
                  <div className="mt-1 text-xs text-muted">
                    {p === 'zigbee' ? 'Hue, IKEA, Aqara, Sonoff and other Zigbee 3.0 devices' : 'Z-Wave and Z-Wave Plus devices, with S2 security'}
                  </div>
                </button>
              ))}
            </div>
          )}
          {step === 'zigbee' && <ZigbeeFlow onClose={onClose} />}
          {step === 'zwave-code' && (
            <ZwaveCode
              pin={pin}
              setPin={setPin}
              onStart={() => setStep('zwave-add')}
              onBack={protos.length > 1 ? () => setStep('choose') : undefined}
            />
          )}
          {step === 'zwave-add' && <ZwaveAdd pin={pin} onClose={onClose} onRetry={() => setStep('zwave-remove')} />}
          {step === 'zwave-remove' && <ZwaveRemove onClose={onClose} onAdd={mode === 'add' ? () => setStep('zwave-code') : undefined} />}
        </div>
      </div>
    </div>
  );
}

function title(s: Step) {
  return s === 'zwave-remove' ? 'Remove a Z-Wave device' : s === 'zigbee' ? 'Add a Zigbee device' : s === 'choose' ? 'Add a device' : 'Add a Z-Wave device';
}

/** Run `fn` once on mount, even under React's development double-mount. */
function useOnce(fn: () => void) {
  const done = useRef(false);
  useEffect(() => {
    if (done.current) return;
    done.current = true;
    fn();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
}

function useSecondsLeft(total: number, running: boolean) {
  const [left, setLeft] = useState(total);
  const start = useRef(Date.now());
  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => setLeft(Math.max(0, total - Math.round((Date.now() - start.current) / 1000))), 500);
    return () => clearInterval(t);
  }, [running, total]);
  return left;
}

function Steps({ items }: { items: { label: string; state: 'done' | 'now' | 'todo' | 'bad'; note?: string }[] }) {
  return (
    <ol className="space-y-2">
      {items.map((s, i) => (
        <li key={i} className="flex items-start gap-2.5 text-sm">
          <span className="mt-0.5">
            {s.state === 'done' ? <Check size={16} className="text-live" />
              : s.state === 'now' ? <Loader2 size={16} className="animate-spin text-accent" />
              : s.state === 'bad' ? <CircleAlert size={16} className="text-alarm" />
              : <span className="block h-4 w-4 rounded-full border-2 border-edge" />}
          </span>
          <span className={s.state === 'todo' ? 'text-muted' : ''}>
            {s.label}
            {s.note && <span className="block text-xs text-muted">{s.note}</span>}
          </span>
        </li>
      ))}
    </ol>
  );
}

function NameIt({ name, onSave }: { name: string; onSave: (n: string) => void }) {
  const [v, setV] = useState(name);
  const [saved, setSaved] = useState(false);
  return (
    <div className="flex gap-2">
      <input
        value={v}
        onChange={(e) => { setV(e.target.value); setSaved(false); }}
        placeholder="Name, e.g. Kitchen light"
        className="hair w-full rounded-md border bg-raised px-2 py-1.5 text-sm outline-none focus:border-accent/50"
      />
      <button
        className={ghost}
        disabled={!v.trim() || v.trim() === name || saved}
        onClick={() => { onSave(v.trim()); setSaved(true); }}
      >
        {saved ? <Check size={14} /> : 'Save'}
      </button>
    </div>
  );
}

/* ---------------- Zigbee ---------------- */

const ZB_WINDOW = 254;

function ZigbeeFlow({ onClose }: { onClose: () => void }) {
  const t = useTree('zigbee');
  const since = useRef(Date.now());
  useOnce(() => trees.zigbee.send('bridge/request/permit_join', { time: ZB_WINDOW }));
  const open = !!parse(t.topics.get('bridge/info'))?.permit_join;
  const left = useSecondsLeft(ZB_WINDOW, open);
  const devices = zigbeeDevices(t);

  // Each device that joined since this dialog opened, and how far it got.
  const found = new Map<string, { state: 'joined' | 'setup' | 'ready' | 'failed'; note?: string }>();
  for (const l of [...t.log].reverse()) {
    if (l.at < since.current || l.tail !== 'bridge/event') continue;
    const d = l.data?.data ?? {};
    const id = d.ieee_address;
    if (!id) continue;
    if (l.data.type === 'device_joined') found.set(id, { state: 'joined' });
    if (l.data.type === 'device_interview') {
      found.set(id, d.status === 'successful'
        ? { state: 'ready', note: [d.manufacturer, d.model].filter(Boolean).join(' ') }
        : d.status === 'failed' ? { state: 'failed', note: d.error ?? d.stage } : { state: 'setup' });
    }
  }

  const stop = () => {
    if (open) trees.zigbee.send('bridge/request/permit_join', { time: 0 });
    onClose();
  };

  return (
    <div className="space-y-4">
      {open ? (
        <div className="flex items-center gap-3 rounded-lg bg-accent/10 p-3">
          <Loader2 size={18} className="animate-spin text-accent" />
          <div className="text-sm">
            Looking for devices… <span className="tabular-nums text-muted">{Math.floor(left / 60)}:{String(left % 60).padStart(2, '0')}</span>
            <div className="text-xs text-muted">
              Put the device in pairing mode: a new one usually is when first powered; otherwise reset it, or hold its button until its light blinks.
            </div>
          </div>
        </div>
      ) : (
        <div className="flex items-center gap-3 rounded-lg bg-raised p-3 text-sm">
          Pairing is closed.
          <button className={`${ghost} ml-auto`} onClick={() => { since.current = Date.now(); trees.zigbee.send('bridge/request/permit_join', { time: ZB_WINDOW }); }}>
            Search again
          </button>
        </div>
      )}

      {[...found].map(([id, f]) => {
        const dev = devices.find((d) => d.id === id);
        return (
          <div key={id} className="hair space-y-3 rounded-xl border bg-raised p-3">
            <Steps
              items={[
                { label: 'Joined the network', state: 'done', note: id },
                {
                  label: 'Setting it up',
                  state: f.state === 'ready' ? 'done' : f.state === 'failed' ? 'bad' : 'now',
                  note: f.state === 'failed' ? `Stopped: ${f.note}. It carries on the next time the device wakes or is pressed.` : dev?.interviewing ? dev.status : undefined,
                },
                { label: f.state === 'ready' ? `Ready${f.note ? `: ${f.note}` : ''}` : 'Ready', state: f.state === 'ready' ? 'done' : 'todo' },
              ]}
            />
            {f.state === 'ready' && dev && <NameIt name={dev.name} onSave={(n) => trees.zigbee.send('bridge/request/rename', { from: dev.name, to: n })} />}
          </div>
        );
      })}

      <div className="flex justify-end">
        <button className={primary} onClick={stop}>{found.size ? 'Done' : 'Stop'}</button>
      </div>
    </div>
  );
}

/* ---------------- Z-Wave ---------------- */

function ZwaveCode({ pin, setPin, onStart, onBack }: { pin: string; setPin: (p: string) => void; onStart: () => void; onBack?: () => void }) {
  const [noCode, setNoCode] = useState(false);
  return (
    <div className="space-y-4">
      <div className="flex gap-3">
        <KeyRound size={18} className="mt-0.5 shrink-0 text-muted" />
        <div className="text-sm">
          Most Z-Wave devices have a security code on their label or box: a QR code with a row of numbers under it.
          Type the <b>first five digits</b> here, so the device is secured as it joins.
        </div>
      </div>
      <input
        autoFocus
        inputMode="numeric"
        maxLength={5}
        value={pin}
        disabled={noCode}
        onChange={(e) => setPin(e.target.value.replace(/\D/g, ''))}
        placeholder="12345"
        className="hair w-40 rounded-md border bg-raised px-3 py-2 text-center font-mono text-lg tracking-[0.3em] outline-none focus:border-accent/50 disabled:opacity-40"
      />
      <label className="flex items-center gap-2 text-xs text-muted">
        <input type="checkbox" checked={noCode} onChange={(e) => { setNoCode(e.target.checked); if (e.target.checked) setPin(''); }} className="accent-[var(--accent)]" />
        My device has no code (older Z-Wave)
      </label>
      <div className="flex justify-between">
        {onBack ? <button className={ghost} onClick={onBack}>Back</button> : <span />}
        <button className={primary} disabled={!noCode && pin.length !== 5} onClick={onStart}>
          <Plus size={15} /> Start adding
        </button>
      </div>
    </div>
  );
}

const ZW_WINDOW = 60;

function ZwaveAdd({ pin, onClose, onRetry }: { pin: string; onClose: () => void; onRetry: () => void }) {
  const t = useTree('zwave');
  const since = useRef(Date.now());
  const [codeAgain, setCodeAgain] = useState('');
  useOnce(() => trees.zwave.send('bridge/request/include', pin ? { strategy: 'requested', pin } : { strategy: 'requested' }));

  // What happened since this step began, oldest first.
  const ev = [...t.log].reverse().filter((l) => l.at >= since.current);
  const has = (e: string) => ev.find((l) => l.tail === 'bridge/event' && l.data?.event === e)?.data;
  const added = has('node_added');
  const node: number | undefined = added?.node;
  const stopped = has('inclusion_stopped');
  const result = ev.find((l) => l.tail === 'bridge/response/include')?.data;
  const secured = has('security_bootstrapped');
  const secFail = has('security_failed');
  const askPin = has('validate_dsk');
  const pinSent = ev.some((l) => l.tail === 'bridge/response/validate_dsk');
  const interviewDone = ev.some((l) => l.data?.event === 'interview_completed' && l.data?.node === node);
  const interviewFail = ev.find((l) => l.data?.event === 'interview_failed' && l.data?.node === node)?.data;
  const stage = [...ev].reverse().find((l) => l.data?.event === 'interview_stage' && l.data?.node === node)?.data?.stage;
  const waiting = !added && !stopped && !result;
  const left = useSecondsLeft(ZW_WINDOW, waiting);
  const dev = node !== undefined ? zwaveDevices(t).find((d) => d.id === String(node)) : undefined;
  const nothing = !added && (stopped || result);
  const security = result?.security ?? secured?.classes;

  return (
    <div className="space-y-4">
      {waiting && (
        <div className="flex items-center gap-3 rounded-lg bg-accent/10 p-3">
          <Loader2 size={18} className="animate-spin text-accent" />
          <div className="text-sm">
            Press the device's inclusion button now <span className="tabular-nums text-muted">({left}s)</span>
            <div className="text-xs text-muted">Usually three quick presses, or as its manual says for "include" or "pair".</div>
          </div>
        </div>
      )}

      {nothing && (
        <div className="space-y-3">
          <div className="flex items-start gap-2 text-sm"><CircleAlert size={16} className="mt-0.5 text-alarm" /> No device answered within {ZW_WINDOW} seconds.</div>
          <p className="text-xs text-muted">
            If it was paired with another controller before, remove it first: Z-Wave devices join only one network at a time.
          </p>
          <div className="flex justify-end gap-2">
            <button className={ghost} onClick={onRetry}><Minus size={14} /> Remove it first</button>
            <button className={primary} onClick={() => { since.current = Date.now(); trees.zwave.send('bridge/request/include', pin ? { strategy: 'requested', pin } : { strategy: 'requested' }); }}>
              Try again
            </button>
          </div>
        </div>
      )}

      {added && (
        <Steps
          items={[
            { label: `Joined as node ${node}`, state: 'done' },
            {
              label: security?.length ? `Secured (${security.map(cls).join(', ')})` : secFail || (result && !security?.length) ? 'Not secured' : 'Securing',
              state: security?.length ? 'done' : secFail || (result && !security?.length) ? 'bad' : 'now',
              note: secFail?.reason ?? result?.security_error ?? undefined,
            },
            {
              label: interviewDone ? 'Set up' : interviewFail ? 'Setting up stopped' : 'Setting it up',
              state: interviewDone ? 'done' : interviewFail ? 'bad' : result ? 'now' : 'todo',
              note: interviewFail?.reason ?? (stage && !interviewDone ? stage.replace(/_/g, ' ') : undefined),
            },
          ]}
        />
      )}

      {askPin && !pinSent && (
        <div className="hair space-y-2 rounded-xl border border-accent/50 bg-raised p-3">
          <div className="text-sm">Type the first five digits of the code on node {askPin.node}</div>
          <div className="flex gap-2">
            <input
              autoFocus
              inputMode="numeric"
              maxLength={5}
              value={codeAgain}
              onChange={(e) => setCodeAgain(e.target.value.replace(/\D/g, ''))}
              className="hair w-32 rounded-md border bg-panel px-2 py-1.5 text-center font-mono tracking-widest outline-none focus:border-accent/50"
            />
            <button className={primary} disabled={codeAgain.length !== 5} onClick={() => trees.zwave.send('bridge/request/validate_dsk', { pin: codeAgain })}>
              Confirm
            </button>
          </div>
        </div>
      )}

      {result && !security?.length && (
        <div className="flex gap-2 rounded-lg bg-warm/10 p-3 text-xs">
          <ShieldAlert size={15} className="mt-0.5 shrink-0 text-warm" />
          <div>
            It joined, but without security{result.security_error ? ` (${result.security_error})` : ''}. It works, unencrypted. To secure it,
            remove it and add it again with its code.
            <button className="mt-1 block text-accent hover:underline" onClick={onRetry}>Remove and try again</button>
          </div>
        </div>
      )}
      {security?.length > 0 && (
        <div className="flex items-center gap-2 text-xs text-live"><ShieldCheck size={14} /> Encrypted with S2.</div>
      )}

      {added && dev && (
        <div className="space-y-1">
          <div className="text-xs text-muted">Name it</div>
          <NameIt name={dev.name} onSave={(n) => trees.zwave.send('bridge/request/rename', { node, name: n, location: '' })} />
        </div>
      )}

      <div className="flex justify-end gap-2">
        {waiting && <button className={ghost} onClick={() => { trees.zwave.send('bridge/request/stop'); onClose(); }}>Cancel</button>}
        {!waiting && <button className={primary} onClick={onClose}>Done</button>}
      </div>
    </div>
  );
}

const cls = (c: string) => c.replace(/_/g, ' ').replace(/^s2 /, 'S2 ').replace(/^s0/, 'S0');

function ZwaveRemove({ onClose, onAdd }: { onClose: () => void; onAdd?: () => void }) {
  const t = useTree('zwave');
  const since = useRef(Date.now());
  useOnce(() => trees.zwave.send('bridge/request/exclude'));
  const ev = [...t.log].reverse().filter((l) => l.at >= since.current);
  const removed = ev.find((l) => l.data?.event === 'node_removed')?.data;
  const response = ev.find((l) => l.tail === 'bridge/response/exclude')?.data;
  const waiting = !response;
  const left = useSecondsLeft(ZW_WINDOW, waiting);
  const again = () => { since.current = Date.now(); trees.zwave.send('bridge/request/exclude'); };
  return (
    <div className="space-y-4">
      {waiting && (
        <div className="flex items-center gap-3 rounded-lg bg-accent/10 p-3">
          <Loader2 size={18} className="animate-spin text-accent" />
          <div className="text-sm">
            Press the device's inclusion button now <span className="tabular-nums text-muted">({left}s)</span>
            <div className="text-xs text-muted">The same presses that add it. Any device can be removed this way, even one from another controller.</div>
          </div>
        </div>
      )}
      {response && (removed || response.ok) && (
        <div className="flex items-center gap-2 text-sm">
          <Check size={16} className="text-live" />
          {removed?.node ?? response.node ? `Removed node ${removed?.node ?? response.node}.` : 'Removed. It was not on this network, and is reset now.'}
        </div>
      )}
      {response && !removed && !response.ok && (
        <div className="space-y-2">
          <div className="flex items-start gap-2 text-sm"><CircleAlert size={16} className="mt-0.5 text-alarm" /> No device answered within {ZW_WINDOW} seconds.</div>
          <button className={ghost} onClick={again}>Try again</button>
        </div>
      )}
      <div className="flex justify-end gap-2">
        {waiting && <button className={ghost} onClick={() => { trees.zwave.send('bridge/request/stop'); onClose(); }}>Cancel</button>}
        {!waiting && onAdd && <button className={ghost} onClick={onAdd}><Plus size={14} /> Add it now</button>}
        {!waiting && <button className={primary} onClick={onClose}>Done</button>}
      </div>
    </div>
  );
}
