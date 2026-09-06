// Minimal ASH + EZSP "version" exchange against the EM357 NCP (UG101).
const fs = require("fs");
const DEV = "/dev/ttymxc4";
const FLAG = 0x7e, ESC = 0x7d, XON = 0x11, XOFF = 0x13, SUB = 0x18, CAN = 0x1a;

const sleep = (ms) => { const e = Date.now() + ms; while (Date.now() < e) {} };
function crc16(buf) { let c = 0xffff;
  for (const b of buf) { c ^= b << 8;
    for (let i = 0; i < 8; i++) c = (c & 0x8000) ? ((c << 1) ^ 0x1021) & 0xffff : (c << 1) & 0xffff; }
  return c; }
function stuff(buf) { const o = [];
  for (const b of buf) {
    if ([FLAG, ESC, XON, XOFF, SUB, CAN].includes(b)) { o.push(ESC, b ^ 0x20); } else o.push(b);
  } return Buffer.from(o); }
function unstuff(buf) { const o = []; let esc = false;
  for (const b of buf) {
    if (b === ESC) { esc = true; continue; }
    o.push(esc ? b ^ 0x20 : b); esc = false;
  } return Buffer.from(o); }
// ASH randomises the DATA field: seed 0x42, next = (r>>1) ^ (r&1 ? 0xB8 : 0)
function rand(n) { const o = []; let r = 0x42;
  for (let i = 0; i < n; i++) { o.push(r); r = (r & 1) ? (r >> 1) ^ 0xb8 : r >> 1; }
  return Buffer.from(o); }
const xorRand = (b) => Buffer.from(b.map((v, i) => v ^ rand(b.length)[i]));

const fd = fs.openSync(DEV, "r+");
const rb = Buffer.alloc(512);
function rd(ms, untilFlag) { const end = Date.now() + ms; let out = Buffer.alloc(0);
  while (Date.now() < end) { let n = 0;
    try { n = fs.readSync(fd, rb, 0, rb.length, null); } catch (e) { n = 0; }
    if (n > 0) { out = Buffer.concat([out, rb.slice(0, n)]);
      if (untilFlag && out.includes(FLAG)) return out; } else sleep(5); }
  return out; }
const wr = (b) => fs.writeSync(fd, b, 0, b.length, null);

function ashFrame(control, payload) {
  const body = Buffer.concat([Buffer.from([control]), payload ? xorRand(payload) : Buffer.alloc(0)]);
  const c = crc16(body);
  return Buffer.concat([stuff(Buffer.concat([body, Buffer.from([(c >> 8) & 0xff, c & 0xff])])),
                        Buffer.from([FLAG])]);
}

// 0. the radio comes back to the bootloader whenever the port is reopened, and the bootloader
// ECHOES input (that is how we spotted it). Start the app first, in this same open.
wr(Buffer.from("\r\n")); rd(1200);
wr(Buffer.from("2"));
const boot = rd(3000, true);
console.log("run  :", boot.toString("hex").slice(0, 80));

// 1. RST -> RSTACK
wr(Buffer.from([0x1a, 0xc0, 0x38, 0xbc, 0x7e]));
const rst = rd(2000, true);
console.log("RSTACK:", rst.toString("hex"));
if (!rst.includes(0xc1)) { console.log("no RSTACK"); process.exit(1); }

// 2. EZSP version(4): DATA frame, frmNum 0, ackNum 0
const ezsp = Buffer.from([0x00, 0x00, 0x00, 0x04]);   // seq, frameCtrl, frameId=version, ver=4
wr(ashFrame(0x00, ezsp));
const resp = rd(3000, true);
console.log("raw   :", resp.toString("hex"));
const f = resp.slice(resp.indexOf(0x7e) >= 0 ? 0 : 0);
const parts = resp.toString("binary").split("~").filter(Boolean).map(s => Buffer.from(s, "binary"));
for (const p of parts) {
  const u = unstuff(p);
  if (u.length < 3) continue;
  const ctrl = u[0];
  if ((ctrl & 0x80) === 0) {                       // DATA frame
    const payload = xorRand(u.slice(1, u.length - 2));
    console.log("EZSP  :", payload.toString("hex"));
    if (payload.length >= 4 && payload[2] === 0x00) {
      console.log("  -> EZSP version", payload[3], " stackType", payload[4],
                  " stackVer 0x" + ((payload[6] << 8 | payload[5]) >>> 0).toString(16));
    }
  } else console.log("ctrl  : 0x" + ctrl.toString(16));
}
fs.closeSync(fd);
