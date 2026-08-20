#!/usr/bin/env node
//
// Reflash the on-board EM357 Zigbee NCP over its UART. Common to every openHC board — they all
// carry an EM357 on a serial port — so this is a board-agnostic utility: point it at the port and
// an EmberZNet `.ebl` and it handles bootloader entry, the XMODEM upload and verification.
//
//   node flash-ncp.js <image.ebl> [/dev/ttymxc4]
//
// How it enters the bootloader, and why it is safe:
//
//   A running NCP does not drop to its bootloader on a port open, so this asks it to, over EZSP:
//   launchStandaloneBootloader (frame 0x8F, mode 1). The NCP jumps to the EM357 serial bootloader
//   ("BL >"), which is in a protected region and survives any app flash. If the upload fails or is
//   interrupted the app is left invalid, so the bootloader simply stays resident on the next reset
//   — recoverable. The one non-recoverable case is flashing a *valid* image whose UART pinout does
//   not match the board, which then runs but cannot be spoken to; keep to images built for this
//   board's pinout (EM357 PB1=TXD/PB2=RXD here).
//
// No sz/lsz/python on these boards, so the XMODEM-CRC sender is here.

const fs = require('node:fs');

const IMG = process.argv[2];
const DEV = process.argv[3] || '/dev/ttymxc4';
if (!IMG) { console.error('usage: flash-ncp.js <image.ebl> [device]'); process.exit(2); }

const FLAG = 0x7e, ESC = 0x7d, XON = 0x11, XOFF = 0x13, SUBST = 0x18, CANB = 0x1a;
const SOH = 0x01, EOT = 0x04, ACK = 0x06, NAK = 0x15, XSUB = 0x1a, CRCCHR = 0x43;

const sleep = (ms) => { const e = Date.now() + ms; while (Date.now() < e) {} };
function crc16(b, init) {
  let c = init;
  for (const x of b) { c ^= x << 8; for (let i = 0; i < 8; i++) c = (c & 0x8000) ? ((c << 1) ^ 0x1021) & 0xffff : (c << 1) & 0xffff; }
  return c;
}
const needsEsc = (b) => [FLAG, ESC, XON, XOFF, SUBST, CANB].includes(b);
const stuff = (b) => Buffer.from([].concat(...[...b].map((v) => (needsEsc(v) ? [ESC, v ^ 0x20] : [v]))));
function unstuff(b) { const o = []; let e = false; for (const v of b) { if (v === ESC) { e = true; continue; } o.push(e ? v ^ 0x20 : v); e = false; } return Buffer.from(o); }
function randmask(n) { const o = []; let r = 0x42; for (let i = 0; i < n; i++) { o.push(r); r = (r & 1) ? (r >> 1) ^ 0xb8 : r >> 1; } return o; }
const xr = (b) => { const m = randmask(b.length); return Buffer.from(b.map((v, i) => v ^ m[i])); };

const fd = fs.openSync(DEV, 'r+');
const rb = Buffer.alloc(1024);
let acc = Buffer.alloc(0);
const wr = (b) => fs.writeSync(fd, b, 0, b.length, null);
function ashPump(ms) {
  const end = Date.now() + ms; const out = [];
  while (Date.now() < end) {
    let n = 0; try { n = fs.readSync(fd, rb, 0, rb.length, null); } catch (e) { n = 0; }
    if (n > 0) acc = Buffer.concat([acc, rb.slice(0, n)]); else { sleep(3); continue; }
    let i; while ((i = acc.indexOf(FLAG)) >= 0) { const raw = acc.slice(0, i); acc = acc.slice(i + 1); if (raw.length) out.push(unstuff(raw)); }
  }
  return out;
}
function raw(ms, want) {
  const end = Date.now() + ms; let o = Buffer.alloc(0);
  while (Date.now() < end) { let n = 0; try { n = fs.readSync(fd, rb, 0, rb.length, null); } catch (e) { n = 0; } if (n > 0) { o = Buffer.concat([o, rb.slice(0, n)]); if (want !== undefined && o.includes(want)) return o; } else sleep(5); }
  return o;
}

let txSeq = 0, rxSeq = 0, seq = 0;
function ashData(p) { const ctrl = (txSeq << 4) | rxSeq; txSeq = (txSeq + 1) & 7; const body = Buffer.concat([Buffer.from([ctrl]), xr(p)]); const c = crc16(body, 0xffff); return Buffer.concat([stuff(Buffer.concat([body, Buffer.from([c >> 8 & 0xff, c & 0xff])])), Buffer.from([FLAG])]); }
const ashAck = () => { const c = crc16(Buffer.from([0x80 | rxSeq]), 0xffff); return Buffer.concat([stuff(Buffer.from([0x80 | rxSeq, c >> 8 & 0xff, c & 0xff])), Buffer.from([FLAG])]); };
function ezsp(frameId, params) {
  wr(ashData(Buffer.from([seq++ & 0xff, 0x00, frameId, ...(params || [])])));
  let status = null;
  for (const f of ashPump(1500)) { if (f.length < 3 || (f[0] & 0x80)) continue; rxSeq = (((f[0] >> 4) & 7) + 1) & 7; wr(ashAck()); const r = xr(f.slice(1, f.length - 2)); if (r[2] === frameId) status = r.slice(3); }
  return status;
}

function enterBootloader() {
  // run the app, ASH reset, then ask the NCP to launch its bootloader
  wr(Buffer.from('\r\n')); ashPump(800);
  wr(Buffer.from('2')); ashPump(2000);
  wr(Buffer.from([0x1a, 0xc0, 0x38, 0xbc, 0x7e])); ashPump(1500);
  acc = Buffer.alloc(0); txSeq = 0; rxSeq = 0; seq = 0;
  ezsp(0x00, [0x04]);              // version
  ezsp(0x8f, [0x01]);              // launchStandaloneBootloader(mode=1)
  sleep(1500);
  wr(Buffer.from('\r\n'));
  const m = raw(2500, undefined).toString();
  if (!m.includes('BL >')) throw new Error('no BL prompt after launchStandaloneBootloader: ' + JSON.stringify(m.replace(/[^\x20-\x7e]/g, '.')));
  console.log('bootloader:', (m.match(/EM357[^\r\n]*/) || ['(prompt)'])[0].trim());
}

function xmodemSend(img) {
  wr(Buffer.from('1'));            // "1. upload ebl"
  let s = Buffer.alloc(0); const t0 = Date.now();
  while (Date.now() - t0 < 8000 && !s.includes(CRCCHR)) s = Buffer.concat([s, raw(300)]);
  if (!s.includes(CRCCHR)) throw new Error('receiver never requested XMODEM-CRC');
  const blocks = Math.ceil(img.length / 128);
  console.log('uploading', img.length, 'bytes,', blocks, 'blocks');
  for (let i = 0; i < blocks; i++) {
    const data = Buffer.alloc(128, XSUB);
    img.copy(data, 0, i * 128, Math.min((i + 1) * 128, img.length));
    const crc = crc16(data, 0);
    const pkt = Buffer.concat([Buffer.from([SOH, (i + 1) & 0xff, ~(i + 1) & 0xff]), data, Buffer.from([crc >> 8 & 0xff, crc & 0xff])]);
    let ok = false;
    for (let retry = 0; retry < 8 && !ok; retry++) { wr(pkt); if (raw(2000, ACK).includes(ACK)) ok = true; }
    if (!ok) throw new Error('block ' + (i + 1) + ' not ACKed');
    if ((i & 63) === 0) process.stdout.write('.' + (i + 1));
  }
  wr(Buffer.from([EOT]));
  raw(2000, ACK);
  console.log('\nupload complete');
}

function verify() {
  wr(Buffer.from('2'));            // run the new NCP
  ashPump(2500);
  wr(Buffer.from([0x1a, 0xc0, 0x38, 0xbc, 0x7e])); ashPump(1500);
  acc = Buffer.alloc(0); txSeq = 0; rxSeq = 0; seq = 0;
  const v = ezsp(0x00, [0x04]);
  if (!v) throw new Error('new NCP did not answer EZSP version — wrong image or pinout for this board');
  console.log('new NCP EZSP protocol version:', v[0]);
  return v[0];
}

try {
  const img = fs.readFileSync(IMG);
  console.log(`flashing ${IMG} (${img.length} B) to EM357 on ${DEV}`);
  enterBootloader();
  xmodemSend(img);
  const ezspVer = verify();
  console.log(ezspVer >= 8 ? 'OK — NCP updated' : `WARNING — EZSP v${ezspVer}, expected >= 8`);
  process.exit(0);
} catch (e) {
  console.error('flash failed:', e.message);
  console.error('the bootloader is still resident; re-run to retry.');
  process.exit(1);
} finally {
  try { fs.closeSync(fd); } catch {}
}
