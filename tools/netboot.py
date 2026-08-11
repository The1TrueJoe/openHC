#!/usr/bin/env python3
"""ohc-netboot — BOOTP + TFTP server for booting a custom kernel on the EA1.

Hold the **ID button** (back of the unit, GPIO 32) at power-on and CEFDK does a
BOOTP request. If the reply carries the magic cookie in DHCP option 60, CEFDK
enters "Control4 Manufacturing Mode", TFTP-fetches the file named in the reply's
`file` field from the reply's `siaddr`, and boots it into RAM. Nothing touches
flash, so a bad kernel is a power-cycle, not a brick.

The protocol is read exactly from the CEFDK GPL source (brd_gen5 bootflow):

  * The RECOVERY button (GPIO 31) boots the eMMC recovery kernel (ip=none) and
    never networks — it is NOT this path. Use the ID button.
  * Reply vendor area must start with the RFC1048 magic cookie (0x63825363).
  * Option 60 (vendor class) value must strcmp-equal "C4_COOKIE".
    getVendorSpecificBytes copies 10 bytes (MAX_VENDOR_SPECIFIC_BYTES), so the
    value is sent as "C4_COOKIE\\0" to guarantee the trailing NUL matches.
  * Server IP for the TFTP = the reply's siaddr. Bootfile = the reply's `file`.
  * On ANY failure after the cookie matches — no tftp, bad file, boot fails —
    CEFDK calls shell(0, NULL): the UNLOCKED bootloader shell. That is a feature
    for us, not a bug (the mfg-floor break-in the C4 comment describes).

Modes:

  observe   Reply with NO cookie. CEFDK BOOTPs, finds no cookie, does not enter
            mfg mode. Pure recon: logs what the EA1 sends (its option 60
            "c4_001", param list). Zero side effects.

  probe     Reply WITH the cookie but serve no working TFTP. CEFDK enters mfg
            mode, tries to fetch, fails, and drops to the unlocked shell. This
            both proves the trigger and hands us a CEFDK prompt.

  serve     Reply WITH the cookie and run a real TFTP server. Point --kernel at
            an xz-compressed bzImage; CEFDK fetches and boots it.

Needs root (binds UDP 67, and 69 in serve mode):

  sudo python3 tools/ohc-netboot.py --iface-ip 192.168.1.5 observe
  sudo python3 tools/ohc-netboot.py --iface-ip 192.168.1.5 probe
  sudo python3 tools/ohc-netboot.py --iface-ip 192.168.1.5 serve --kernel vmlinuz.xz

Pure stdlib, no dependencies — same rule as ohc-nfsd.py.
"""
import argparse, socket, struct, sys, os, time

DHCP_MAGIC = bytes([0x63, 0x82, 0x53, 0x63])

def mac_bytes(s):
    return bytes(int(x, 16) for x in s.replace("-", ":").split(":"))

def mac_str(b):
    return ":".join(f"{x:02x}" for x in b)

def parse_options(data):
    """Return {code: value_bytes} from the DHCP options area."""
    opts, i = {}, 0
    while i < len(data):
        code = data[i]
        if code == 0xFF:            # END
            break
        if code == 0x00:            # PAD
            i += 1; continue
        length = data[i + 1]
        opts[code] = data[i + 2:i + 2 + length]
        i += 2 + length
    return opts

# Human names for the options we care about, so the log reads plainly.
OPT_NAMES = {
    12: "hostname", 43: "vendor-specific", 53: "msg-type", 54: "server-id",
    55: "param-req-list", 57: "max-msg-size", 60: "vendor-class",
    61: "client-id", 93: "client-arch", 94: "client-net-iface",
    97: "client-machine-id", 66: "tftp-server", 67: "bootfile",
}
MSG_TYPES = {1: "DISCOVER", 2: "OFFER", 3: "REQUEST", 4: "DECLINE",
             5: "ACK", 6: "NAK", 7: "RELEASE", 8: "INFORM"}

def describe(opts):
    lines = []
    for code, val in sorted(opts.items()):
        name = OPT_NAMES.get(code, f"opt{code}")
        if code == 53:
            shown = MSG_TYPES.get(val[0], val[0])
        elif code == 55:
            shown = ",".join(str(b) for b in val)  # requested option codes
        elif code in (12, 60, 66, 67) or all(32 <= b < 127 for b in val):
            shown = repr(val.decode("latin1"))
        else:
            shown = val.hex()
        lines.append(f"      {code:>3} {name:<16} {shown}")
    return "\n".join(lines)

def build_reply(xid, chaddr, yiaddr, opts, flags, server_ip, next_server="",
                bootfile=""):
    """Assemble a BOOTREPLY. yiaddr='' for a NAK-ish empty offer is not used."""
    pkt = struct.pack(
        "!BBBB I HH 4s 4s 4s 4s 16s",
        2, 1, 6, 0,                       # op=REPLY, htype=eth, hlen=6, hops
        xid, 0, flags,                    # xid, secs, flags (echo broadcast bit)
        b"\x00\x00\x00\x00",              # ciaddr
        socket.inet_aton(yiaddr),         # yiaddr — the address we assign
        socket.inet_aton(next_server) if next_server else b"\x00\x00\x00\x00",
        b"\x00\x00\x00\x00",              # giaddr
        chaddr + b"\x00" * (16 - len(chaddr)),
    )
    # sname (64) — leave empty; file (128) carries the bootfile name.
    pkt += b"\x00" * 64
    pkt += (bootfile.encode() + b"\x00" * 128)[:128]
    pkt += DHCP_MAGIC
    body = b""
    for code, val in opts:
        body += bytes([code, len(val)]) + val
    body += b"\xff"
    return pkt + body

def tftp_serve(kernel_path, bind_ip, log, serve_only=None):
    """Minimal read-only TFTP (RFC 1350), octet mode, 512-byte blocks.

    Runs in the foreground of its own process; forked per-transfer so DHCP keeps
    answering. CEFDK is the only client, so no security beyond "read the one file
    we were told to serve".

    If serve_only is set, a request for any OTHER filename gets a TFTP error.
    That is how shellboot mode makes the mfg auto-fetch (which requests the
    bogus bootfile in the DHCP reply) fail fast — CEFDK then drops to the
    unlocked shell — while still serving the real kernel to the manual
    `tftp get ... bzImage` we type at that shell (which does not verify).
    """
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    # Bind to any address, not bind_ip: binding the specific IP can fail
    # EADDRNOTAVAIL on macOS depending on interface state, and the route to
    # the client already egresses the right interface (source IP = iface IP).
    s.bind(("", 69))
    log(f"tftp: serving {kernel_path} on {bind_ip}:69"
        + (f" (only as {serve_only!r})" if serve_only else ""))
    data = open(kernel_path, "rb").read()
    while True:
        req, client = s.recvfrom(2048)
        op = struct.unpack("!H", req[:2])[0]
        # WRQ (opcode 2) = client wants to UPLOAD to us (tftp put). Used to
        # dump device RAM over ethernet for offline analysis. Write to
        # <bind dir>/<filename>. Much faster than ord-over-serial.
        if op == 2:
            fn = req[2:].split(b"\x00")[0].decode("latin1")
            dest = os.path.join("/tmp", "ramdump_" + os.path.basename(fn or "upload"))
            log(f"tftp: WRQ {fn!r} -> {dest}")
            if os.fork() != 0:
                continue
            t = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            t.bind(("", 0)); t.settimeout(8)
            t.sendto(struct.pack("!HH", 4, 0), client)   # ACK block 0
            out = open(dest, "wb"); expect = 1
            while True:
                try:
                    pkt, who = t.recvfrom(2048)
                except socket.timeout:
                    break
                if pkt[:2] != struct.pack("!H", 3):     # DATA
                    continue
                blk = struct.unpack("!H", pkt[2:4])[0]
                if blk == expect:
                    out.write(pkt[4:]); expect = (expect + 1) & 0xFFFF
                t.sendto(struct.pack("!HH", 4, blk), who)
                if len(pkt) - 4 < 512:
                    break
            out.close(); log(f"tftp: upload done -> {dest} ({os.path.getsize(dest)} bytes)")
            os._exit(0)
        if op != 1:                       # only RRQ past here
            continue
        fn = req[2:].split(b"\x00")[0].decode("latin1")
        if serve_only is not None and fn != serve_only:
            # TFTP ERROR opcode 5, code 1 = file not found -> CEFDK mfg tftp fails.
            t = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            t.sendto(struct.pack("!HH", 5, 1) + b"no\x00", client)
            t.close()
            log(f"tftp: refused {fn!r} (mfg auto-fetch) -> CEFDK should drop to shell")
            continue
        log(f"tftp: RRQ {fn!r} from {client[0]} — sending {len(data)} bytes")
        if os.fork() != 0:
            continue
        # child: transfer on a fresh ephemeral port, as TFTP requires
        t = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        t.bind(("", 0))
        t.settimeout(5)
        blk = 1
        for off in range(0, len(data) + 1, 512):
            chunk = data[off:off + 512]
            pkt = struct.pack("!HH", 3, blk & 0xFFFF) + chunk
            for _ in range(5):
                t.sendto(pkt, client)
                try:
                    ack, _ = t.recvfrom(64)
                    if ack[:2] == struct.pack("!H", 4) and ack[2:4] == struct.pack("!H", blk & 0xFFFF):
                        break
                except socket.timeout:
                    continue
            blk += 1
            if len(chunk) < 512:
                break
        log("tftp: transfer complete")
        os._exit(0)

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iface-ip", required=True, help="this machine's IP on the direct link, e.g. 192.168.1.5")
    ap.add_argument("--client-mac", default="00:0f:ff:1a:fc:a9", help="EA1 MAC")
    ap.add_argument("--offer-ip", default="192.168.1.50", help="address to hand the EA1")
    ap.add_argument("--netmask", default="255.255.255.0")
    # tftpd: JUST a TFTP server, no DHCP. For the shell-boot path — the mfg auto
    # path enforces an RSA signature (SOFT_HANG on a kernel we can't sign), but
    # the CEFDK shell's `bootlinux` does NOT verify. So: get to the shell via
    # `probe`, then from the shell `tftp get <this-ip> 0xc0140000 bzImage` (this
    # server) and `bootlinux "<cmdline>"`.
    ap.add_argument("mode", choices=["observe", "probe", "serve", "tftpd", "shellboot"])
    ap.add_argument("--kernel", help="bzImage to serve (serve/tftpd mode)")
    ap.add_argument("--bootfile", default="vmlinuz.xz")
    ap.add_argument("--cookie", default="C4_COOKIE", help="option-60 value CEFDK checks")
    a = ap.parse_args()

    if a.mode in ("serve", "tftpd", "shellboot") and not (a.kernel and os.path.isfile(a.kernel)):
        sys.exit(f"{a.mode} mode needs --kernel pointing at a real file")

    def _log(m):
        print(f"[{time.strftime('%H:%M:%S')}] {m}", flush=True)

    if a.mode == "tftpd":
        _log(f"TFTP-only on {a.iface_ip}:69 serving {a.kernel} for any filename.")
        _log("At the CEFDK shell: tftp get %s 0xc0140000 bzImage ; bootlinux \"console=ttyS0,115200\"" % a.iface_ip)
        tftp_serve(a.kernel, a.iface_ip, _log)
        return

    want_mac = mac_bytes(a.client_mac)

    def log(m):
        print(f"[{time.strftime('%H:%M:%S')}] {m}", flush=True)

    if a.mode in ("serve", "shellboot"):
        # TFTP in a child so the DHCP loop below keeps running. shellboot serves
        # ONLY as "bzImage": the mfg auto-fetch asks for the bootfile name in the
        # reply (vmlinuz.xz), gets refused, and CEFDK drops to the shell — where
        # `bootlinux` boots our kernel without the RSA check.
        if os.fork() == 0:
            tftp_serve(a.kernel, a.iface_ip, log,
                       serve_only="bzImage" if a.mode == "shellboot" else None)
            return

    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.bind(("", 67))
    log(f"DHCP up on :67 ({a.mode} mode). Waiting for the EA1 — power-cycle holding the button.")
    if a.mode == "serve":
        log(f"will offer {a.offer_ip}, next-server {a.iface_ip}, bootfile {a.bootfile!r}")

    while True:
        data, src = s.recvfrom(2048)
        if len(data) < 240 or data[:1] != b"\x01":   # BOOTREQUEST only
            continue
        xid, flags = struct.unpack("!I", data[4:8])[0], struct.unpack("!H", data[10:12])[0]
        chaddr = data[28:34]
        opts = parse_options(data[240:])
        mtype = opts.get(53, b"\x00")[0]
        log(f"{MSG_TYPES.get(mtype, mtype)} from {mac_str(chaddr)}:")
        print(describe(opts), flush=True)

        if chaddr != want_mac:
            log(f"  (ignoring — not the EA1 {a.client_mac})")
            continue

        # CEFDK parses only tags 1 (subnet), 3 (gateway) and 60 (the cookie);
        # everything else hits its "don't support the tag" default and is
        # ignored, so keep the reply to what it reads.
        base = [
            (1, socket.inet_aton(a.netmask)),
            (3, socket.inet_aton(a.iface_ip)),           # gateway = us
        ]
        if a.mode in ("probe", "serve", "shellboot"):
            # Option 60 value must strcmp-equal the cookie. CEFDK copies 10 bytes
            # into a zeroed buffer, so send the NUL terminator explicitly.
            base.append((60, a.cookie.encode() + b"\x00"))

        # siaddr (server IP for TFTP) and the file field carry the rest.
        reply = build_reply(xid, chaddr, a.offer_ip, base, flags,
                            a.iface_ip, next_server=a.iface_ip, bootfile=a.bootfile)

        # Broadcast the reply — the client has no IP yet, so unicast can't ARP.
        # Send to the SUBNET-directed broadcast (e.g. 192.168.1.255), not the
        # limited 255.255.255.255: on a Mac with several interfaces the limited
        # broadcast egresses the default route (wifi), not the direct link, so
        # CEFDK never hears it and reports "Bootp configuration failed". The
        # directed broadcast is routed out the interface that owns the subnet.
        ip = [int(x) for x in a.iface_ip.split(".")]
        nm = [int(x) for x in a.netmask.split(".")]
        bcast = ".".join(str((ip[i] & nm[i]) | (~nm[i] & 0xFF)) for i in range(4))
        for dst in (bcast, "255.255.255.255"):
            try:
                s.sendto(reply, (dst, 68))
            except OSError as e:
                log(f"  (send to {dst} failed: {e})")
        if a.mode == "observe":
            log(f"  -> reply {a.offer_ip}, NO cookie (recon only — CEFDK stays out of mfg mode)")
        else:
            log(f"  -> reply {a.offer_ip}, cookie={a.cookie!r}, siaddr={a.iface_ip}, file={a.bootfile!r}"
                + ("  [probe: no TFTP -> expect drop to unlocked shell]" if a.mode == "probe" else ""))

if __name__ == "__main__":
    main()
