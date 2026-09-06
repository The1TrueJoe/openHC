// XMODEM-CRC sender for the EM357 serial bootloader ("1. upload ebl").
// No sz/lsz/python on this box, so this is the transfer.
const fs = require("fs");
const { execSync } = require("child_process");

const DEV = "/dev/ttymxc4";
const IMG = process.argv[2];
const SOH = 0x01, EOT = 0x04, ACK = 0x06, CAN = 0x18, SUB = 0x1a, CRCCHR = 0x43;

const sleep = (ms) => { const e = Date.now() + ms; while (Date.now() < e) {} };
function crc16(buf) {                       // CCITT poly 0x1021, init 0 - XMODEM flavour
  let c = 0;
  for (const b of buf) {
    c ^= b << 8;
    for (let i = 0; i < 8; i++) c = (c & 0x8000) ? ((c << 1) ^ 0x1021) & 0xffff : (c << 1) & 0xffff;
  }
  return c;
}

const fd = fs.openSync(DEV, "r+");
const rb = Buffer.alloc(256);
function rd(ms, want) {
  const end = Date.now() + ms; let out = Buffer.alloc(0);
  while (Date.now() < end) {
    let n = 0;
    try { n = fs.readSync(fd, rb, 0, rb.length, null); } catch (e) { n = 0; }
    if (n > 0) { out = Buffer.concat([out, rb.slice(0, n)]); if (want !== undefined && out.includes(want)) return out; } else sleep(5);
  }
  return out;
}
const wr = (b) => fs.writeSync(fd, b, 0, b.length, null);

//execSync("devmem 0x0209C000 32 0x00030400"); sleep(300);
//execSync("devmem 0x0209C000 32 0x00030500"); sleep(1500);
rd(200);
wr(Buffer.from("\r\n"));
const menu = rd(1500);
if (!menu.includes("BL >")) { console.log("no BL prompt:", JSON.stringify(menu.toString())); process.exit(1); }
console.log("bootloader ready");

wr(Buffer.from("1"));
let start = Buffer.alloc(0); const t0 = Date.now();
while (Date.now() - t0 < 8000 && !start.includes(CRCCHR)) start = Buffer.concat([start, rd(300)]);
if (!start.includes(CRCCHR)) { console.log("no C:", start.toString("hex")); process.exit(1); }
console.log("receiver requested XMODEM-CRC");

const img = fs.readFileSync(IMG);
const blocks = Math.ceil(img.length / 128);
console.log("sending", img.length, "bytes in", blocks, "blocks");
let bn = 1;
for (let i = 0; i < blocks; i++) {
  const data = Buffer.alloc(128, SUB);
  img.copy(data, 0, i * 128, Math.min((i + 1) * 128, img.length));
  const crc = crc16(data);
  const pkt = Buffer.concat([Buffer.from([SOH, bn & 0xff, (~bn) & 0xff]), data,
                             Buffer.from([(crc >> 8) & 0xff, crc & 0xff])]);
  let ok = false;
  for (let retry = 0; retry < 6 && !ok; retry++) {
    wr(pkt);
    const r = rd(2000, ACK);
    if (r.includes(ACK)) ok = true;
    else if (r.includes(CAN)) { console.log("cancelled at block", bn); process.exit(1); }
  }
  if (!ok) { console.log("no ACK for block", bn); process.exit(1); }
  bn = (bn + 1) & 0xff;
  if (i % 100 === 0) console.log("  block", i, "/", blocks);
}
wr(Buffer.from([EOT]));
const fin = rd(3000);
console.log("EOT reply:", fin.toString("hex"), fin.toString().replace(/[^ -~]/g, "."));
fs.closeSync(fd);
