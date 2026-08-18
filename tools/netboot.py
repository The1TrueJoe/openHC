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
import argparse, socket, struct, subprocess, sys, os, time

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
    # The kernel is read fresh on every RRQ (below), NOT cached here: this
    # process outlives many `make image` rebuilds, and caching at startup meant a
    # rebuilt image was silently ignored — the board kept netbooting the stale
    # kernel until serve was restarted. Re-reading per request costs one file
    # read per netboot (rare) and guarantees the latest image every time.
    log(f"tftp: serving {kernel_path} on {bind_ip}:69 ({os.path.getsize(kernel_path)} bytes)"
        + (f" (only as {serve_only!r})" if serve_only else ""))
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
        data = open(kernel_path, "rb").read()   # fresh per request (see note above)
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

# ══════════════════════════════════════════════════════════════════════════
# `boot` mode — the whole netboot in one command
# ══════════════════════════════════════════════════════════════════════════
#
# The older flow was three tools and a lot of typing: `probe` to reach the
# shell, `tftp-serve.py` in a second terminal, `ohc-bootlinux.py` in a third.
# `boot` does all of it in one process:
#
#     sudo python3 tools/netboot.py --board ea3 boot
#
#   1. threaded TFTP over output/images (bzImage AND rootfs.cpio.gz)
#   2. BOOTP replies with the C4_COOKIE but a bootfile that does not exist, so
#      the mfg auto-fetch fails and CEFDK drops to the UNLOCKED shell
#   3. watches the serial console, and the moment `shell>` appears drives the
#      bootlinux sequence itself — tftp the images in, set the four globals,
#      verify them, boot
#   4. reopens the console at the kernel's real baud and streams the boot log
#
# The one thing it cannot do is hold the ID button for you.

# The kernel command line is per-board, exactly like the addressing.
#
# Keep this in sync with each board's CONFIG_CMDLINE — the kernel sees BOTH
# (CMDLINE_EXTEND concatenates them), so a stale copy here silently wins.
#
# Both EA1 and EA3 need `nocrs`: this SoC's ACPI _CRS declares only a 48 KB
# window and omits the 0xc0000000-0xdfffffff MMIO space, so without it the
# bus-01 bridge window cannot be claimed and the kernel hangs before userspace.
# Note the kernel ORs nocrs/use_crs into one flag word and nocrs wins whichever
# came last, so this cannot be overridden from the bootlinux line either way.
_CMDLINE_BASE = "console=ttyS0,115200 earlyprintk=serial,ttyS0,115200"

BOARDS = {
    # dhcp: reply style. uart: is a serial console wired? -> `boot` drives the
    # bootloader over UART if so, else over SSH (fw_setenv). ssh_*: creds for the
    # UART-less path. offer_ip is what we hand the board; the boot flow reaches
    # the stock Linux there over SSH.
    "ea1": dict(iface_ip="192.168.1.5",  client_mac="00:0f:ff:1a:fc:a9",
                offer_ip="192.168.1.50", console_baud=921600, dhcp="cefdk", uart=True,
                cmdline=_CMDLINE_BASE + " pci=realloc,nocrs"),
    "ea3": dict(iface_ip="10.0.0.105",   client_mac="00:0f:ff:94:ee:02",
                offer_ip="10.0.0.139",   console_baud=921600, dhcp="cefdk", uart=True,
                cmdline=_CMDLINE_BASE + " pci=realloc,nocrs"),
    # DM355 IO Extender V1: stock U-Boot -> PLAIN DHCP (real OFFER/ACK, no cookie),
    # TFTP the appended-DTB uImage. NO serial console wired, so `boot` drives a
    # self-reverting one-shot netboot over SSH (fw_setenv). iface auto-derives
    # from iface_ip (the P2P link).
    "ioxv1": dict(iface_ip="192.168.0.10", client_mac="00:0f:ff:18:21:9c",
                offer_ip="192.168.0.50", console_baud=115200, dhcp="plain", uart=False,
                ssh_user="root", ssh_pass="t0talc0ntr0l4!",
                cmdline="console=ttyS0,115200n8"),
    # CA-1 (i.MX6SL): stock Control4 U-Boot, mfg mode. Hold the ID button at
    # power-on -> U-Boot DHCPs with vendor-class 'c4_ca1' and needs the reply to
    # carry BOTH option 60 = "C4_COOKIE" (sets dhcp_mfgmode bit 0) AND option 43
    # sub-option 0x0a = the DTB filename (sets bit 1 + fdt_file); with mfgmode=3
    # it TFTPs ${bootfile} (kernel) and ${fdt_file} (dtb) itself and bootz's --
    # no serial driving needed. Decoded from OS-3.3.1 u-boot patch
    # 170-uboot-control4-mfg-mode. The box builds its own bootargs
    # (console=ttymxc0,115200 ip=dhcp mfgmode=1), so cmdline here is unused.
    # REQUIRES AN ISOLATED LINK. Proven on hardware 2026-08-18: the DHCP reply
    # encoding is correct (U-Boot printed 'MFG: Activated Manufacturing Mode'),
    # but the CA-1's U-Boot is built WITHOUT CONFIG_SYS_BOOTFILE_PREFIX, so its
    # `dhcp` accepts the FIRST offer it gets with no way to prefer ours. On a
    # shared LAN the home router wins the race (box bound via server-id .1, our
    # cookie/opt43 never parsed -> 'DHCP Manufacturing Mode Failure' -> stock).
    # Fix: put the CA-1 on a direct cable to this host, or a switch with only the
    # two -- no other DHCP server -- exactly like the EA/DM355 P2P links. Then
    # iface_ip is this host's address on THAT link (override with IFACE_IP=).
    # The box loads the DTB at its own ${fdt_addr} (0x83000000), only ~40 MB
    # above the kernel, so a netbooted zImage must stay lean (no embedded Node).
    "ca1": dict(iface_ip="192.168.1.171", client_mac="00:0f:ff:52:82:65",
                offer_ip="192.168.1.178", console_baud=115200, dhcp="c4mfg", uart=False,
                bootfile="openhc-ca1-zImage", fdt_file="c4-imx6sl-ca1.dtb",
                cmdline="console=ttymxc0,115200"),
}


def iface_for_ip(ip):
    """Interface name that currently owns `ip` (macOS/BSD ifconfig)."""
    try:
        out = subprocess.run(["ifconfig"], capture_output=True, text=True).stdout
    except Exception:
        return None
    cur = None
    for line in out.splitlines():
        if line and not line[0].isspace():
            cur = line.split(":", 1)[0]
        elif line.strip().startswith(f"inet {ip} "):
            return cur
    return None

# Verified on the EA1; the EA3 is the same CE5310 so the same globals apply.
# Kernel is copied by the loader to 0x100000, so stage it high enough that the
# source never overlaps the destination. Initrd page-aligned = used in place.
KERNEL_ADDR = 0x06000000   # 96 MB
INITRD_ADDR = 0x04000000   # 64 MB
G_KBASE     = 0x000c90a4   # linuxKernelBase
G_RD_FLAG   = 0x00837560   # ramdisk present
G_RD_ADDR   = 0x00837564   # ramdisk address
G_RD_SIZE   = 0x00837568   # ramdisk size


def _tftp_module():
    """Load tools/tftp-serve.py as a module (its name has a dash)."""
    import importlib.util
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tftp-serve.py")
    spec = importlib.util.spec_from_file_location("ohc_tftp", p)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def tftp_dir_serve(root, log, stop):
    """Threaded, directory-serving TFTP on :69.

    Reuses tftp-serve.py's transfer(), which already caps blksize to 1468 (CEFDK
    asks for 47040, which macOS cannot send and CEFDK cannot reassemble) and
    threads each transfer instead of forking (fork is fork-unsafe on macOS and
    died during the mfg request storm).
    """
    import threading
    t = _tftp_module()
    root = os.path.realpath(root)
    srv = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("", 69))
    srv.settimeout(0.5)
    log(f"tftp: serving {root} on :69")
    while not stop.is_set():
        try:
            pkt, cli = srv.recvfrom(2048)
        except socket.timeout:
            continue
        except OSError:
            break
        if len(pkt) < 2:
            continue
        op = struct.unpack(">H", pkt[:2])[0]
        if op != t.RRQ:
            continue
        fname = pkt[2:].split(b"\x00")[0].decode("latin1")
        path = os.path.realpath(os.path.join(root, fname.lstrip("/")))
        if not path.startswith(root) or not os.path.isfile(path):
            # The mfg auto-fetch asks for a bootfile we deliberately do not
            # have. Refusing it inline is what pushes CEFDK to the shell.
            e = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            e.sendto(struct.pack(">HH", t.ERROR, 1) + b"not found\x00", cli)
            e.close()
            log(f"tftp: refused {fname!r} (expected — this is what drops CEFDK to the shell)")
            continue
        log(f"tftp: RRQ {fname} from {cli[0]}")
        threading.Thread(target=t.transfer, args=(root, cli, pkt), daemon=True).start()


class Shell:
    """Drive the CEFDK `shell>` over the serial console."""

    def __init__(self, port, baud=115200):
        import serial
        self.serial = serial
        self.port = port
        self.s = serial.Serial(port, baud, timeout=0.2)

    def _read(self, secs, until=None):
        out, end = "", time.monotonic() + secs
        while time.monotonic() < end:
            d = self.s.read(4096)
            if d:
                out += bytes(b for b in d if b != 0).decode("utf-8", "replace")
                if until and until in out:
                    break
        return out

    def cmd(self, c, wait=3.0, until="shell>"):
        # Drop anything already buffered FIRST. The previous command left its
        # own "shell>" prompt sitting in the input queue, so without this the
        # read below matches `until` instantly and returns only the echoed
        # command — which is how a perfectly staged kernel got misread as a bad
        # one (the "magic" came back as the address, not the value).
        try:
            self.s.reset_input_buffer()
        except Exception:
            pass
        self.s.write(c.encode() + b"\r")
        return self._read(wait, until)

    def wait_for_shell(self, secs, log):
        """Watch the boot for the unlocked shell prompt."""
        out, end = "", time.monotonic() + secs
        seen_mfg = False
        while time.monotonic() < end:
            d = self.s.read(4096)
            if not d:
                continue
            txt = bytes(b for b in d if b != 0).decode("utf-8", "replace")
            sys.stdout.write(txt); sys.stdout.flush()
            out += txt
            if not seen_mfg and "Manufacturing Mode" in out:
                seen_mfg = True
                log("saw 'Manufacturing Mode' — ID button registered")
            if "Factory Restore (Button): Enabled" in out:
                log("!! that is the RECOVERY button, not ID — it will factory-restore. Power off now.")
                return False
            if "shell>" in out:
                return True
        return False

    @staticmethod
    def _val(out):
        """Pull the value out of an ord2/ord4 reply.

        The reply is BARE uppercase hex on its own line — no `0x` prefix:

            ord2 0x60001fe        <- echo (contains the ADDRESS)
            AA55                  <- the value we want
            shell>

        An earlier version scanned for tokens starting with `0x` and took the
        last one, which can only ever match the address in the echo. Match a
        whole line of hex digits instead, and skip the echo and the prompt.
        """
        import re
        for line in out.splitlines():
            line = line.strip()
            if not line or line.startswith("shell>"):
                continue
            if line.split()[0].lower() in ("ord1", "ord2", "ord4", "owr1", "owr2", "owr4"):
                continue                      # the echoed command
            if re.fullmatch(r"[0-9A-Fa-f]{1,16}", line):
                return int(line, 16)
        return None

    def rd2(self, addr):
        return self._val(self.cmd(f"ord2 {addr:#x}", 2))

    def rd4(self, addr):
        return self._val(self.cmd(f"ord4 {addr:#x}", 2))

    def wr4(self, addr, val):
        self.cmd(f"ord4 {addr:#x} = {val:#x}", 2)


def shell_bootlinux(sh, server_ip, images, cmdline, console_baud, boot_secs, log,
                    kernel="bzImage", initrd="rootfs.cpio.gz", dry_run=False):
    """From an unlocked `shell>`, stage the images and bootlinux them."""
    initrd_path = os.path.join(images, initrd)
    if not os.path.isfile(initrd_path):
        sys.exit(f"missing {initrd_path}")
    initrd_size = os.path.getsize(initrd_path)

    log(f"tftp kernel {kernel} -> {KERNEL_ADDR:#x}")
    sh.cmd(f"tftp get {server_ip} {KERNEL_ADDR:#x} {kernel}", 180)
    m = sh.rd2(KERNEL_ADDR + 0x1FE)
    log(f"  bzImage magic @+0x1fe = {m if m is None else hex(m)} (want 0xaa55)")
    if m != 0xAA55:
        sys.exit("!! kernel did not stage — refusing to boot")

    log(f"tftp initrd {initrd} -> {INITRD_ADDR:#x} ({initrd_size} bytes)")
    sh.cmd(f"tftp get {server_ip} {INITRD_ADDR:#x} {initrd}", 120)
    g = sh.rd2(INITRD_ADDR)
    log(f"  gzip magic @initrd = {g if g is None else hex(g)} (want 0x8b1f)")
    if g != 0x8B1F:
        sys.exit("!! initrd did not stage — refusing to boot")

    log("setting loader globals")
    for name, addr, want in (("linuxKernelBase", G_KBASE, KERNEL_ADDR),
                             ("rd_flag", G_RD_FLAG, 1),
                             ("rd_addr", G_RD_ADDR, INITRD_ADDR),
                             ("rd_size", G_RD_SIZE, initrd_size)):
        sh.wr4(addr, want)
    bad = []
    for name, addr, want in (("linuxKernelBase", G_KBASE, KERNEL_ADDR),
                             ("rd_flag", G_RD_FLAG, 1),
                             ("rd_addr", G_RD_ADDR, INITRD_ADDR),
                             ("rd_size", G_RD_SIZE, initrd_size)):
        got = sh.rd4(addr)
        log(f"  {name:16s} {addr:#010x} = {got if got is None else hex(got)} (want {want:#x})"
            + ("" if got == want else "   MISMATCH"))
        if got != want:
            bad.append(name)
    if bad:
        sys.exit(f"!! globals did not stick ({', '.join(bad)}) — refusing to boot")

    if dry_run:
        log(f'--dry-run: staged. Run  bootlinux "{cmdline}"  to boot.')
        return

    log(f'bootlinux "{cmdline}"')
    sh.s.write(f'bootlinux "{cmdline}"\r'.encode())
    sh.s.flush()
    if console_baud != 115200:
        # Switch FAST. The kernel reprograms this UART in its first few
        # instructions (earlyprintk), so anything printed before we reopen is
        # gone. A 1.0 s pause here lost the entire early boot on the first EA3
        # attempt — the board hung seconds later and there was nothing to read.
        # We do not care about CEFDK's echo of the command; the kernel's first
        # lines are the whole point.
        log(f"switching console to {console_baud} immediately "
            f"(CE5310 legacy UART clock is 8x standard)")
        time.sleep(0.15)
        sh.s.close()
        sh.s = sh.serial.Serial(sh.port, console_baud, timeout=0.05)
    else:
        time.sleep(0.5)

    logf = os.path.realpath(os.path.join(images, "..", "boot-console.log"))
    log(f"streaming kernel console for {boot_secs}s -> {logf}")
    print("=" * 70)
    with open(logf, "wb") as lf:
        end = time.monotonic() + boot_secs
        while time.monotonic() < end:
            d = sh.s.read(4096)
            if d:
                lf.write(d); lf.flush()
                sys.stdout.write(d.decode("latin1")); sys.stdout.flush()
    print("\n" + "=" * 70)
    n = os.path.getsize(logf)
    log(f"boot log saved -> {logf}  ({n} bytes)")
    if n == 0:
        log("NOTHING was captured. Either the kernel died before printing, or")
        log(f"{console_baud} is the wrong rate for this board. Sweep the line with:")
        log("  for b in 115200 230400 460800 921600 1500000; do \\")
        log("    python3 tools/serial-console.py --baud $b --listen 2; done")
        log("A hung board is silent at EVERY rate — that distinguishes the two.")


def find_serial_port():
    import glob
    for pat in ("/dev/cu.usbserial*", "/dev/cu.usbmodem*", "/dev/ttyUSB*"):
        m = sorted(glob.glob(pat))
        if m:
            return m[0]
    sys.exit("no USB-serial adapter found — pass --serial-port")


def boot_mode_ssh(a, log):
    """No serial console: bring the netboot up over the network. Serve plain
    DHCP + TFTP, wait for the stock Linux to take the lease, SSH in and set
    bootcmd to 'run tst; run oldbootcmd' (netboot first, stock as automatic
    fallback), reboot. U-Boot then DHCP+TFTPs our kernel. To recover a hung
    kernel: stop this server and power-cycle -- `run tst` fails with no TFTP and
    `run oldbootcmd` boots stock. No serial, no brick."""
    import threading, tempfile, shutil

    kernel = a.kernel
    if not (kernel and os.path.isfile(kernel)):
        sys.exit(f"boot mode needs --kernel at a real file (got {kernel!r}) — "
                 "run 'make image BOARD=%s' first" % (a.board or "ioxv1"))
    if not a.ssh_pass:
        sys.exit("UART-less boot needs --ssh-pass (or a board preset that has it)")
    if not shutil.which("sshpass"):
        sys.exit("UART-less boot needs sshpass:  brew install sshpass")

    host = a.offer_ip
    log(f"board={a.board or 'custom'} iface={a.iface_ip} ({a.iface or 'all'}) "
        f"mac={a.client_mac} offer={host}  [no UART -> SSH-driven]")

    # U-Boot's `run tst` fetches ${tstimage} (hammer/uImage) from ${tstserverip}
    # (=192.168.0.10). Serve a dir with that exact name -> our kernel. Threaded
    # TFTP (tftp_dir_serve), not the forking one, so it is safe next to threads.
    tdir = tempfile.mkdtemp(prefix="openhc-tftp-")
    os.makedirs(os.path.join(tdir, "hammer"), exist_ok=True)
    shutil.copy(kernel, os.path.join(tdir, "hammer", "uImage"))

    stop = threading.Event()
    threading.Thread(target=tftp_dir_serve, args=(tdir, log, stop), daemon=True).start()
    threading.Thread(target=bootp_responder, args=(a, log, stop), daemon=True).start()
    time.sleep(0.4)

    ssh = ["sshpass", "-p", a.ssh_pass, "ssh",
           "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
           "-o", "ConnectTimeout=5", "-o", "LogLevel=ERROR",
           "-o", "HostKeyAlgorithms=+ssh-rsa", "-o", "PubkeyAcceptedAlgorithms=+ssh-rsa",
           "-o", "KexAlgorithms=+diffie-hellman-group1-sha1,diffie-hellman-group14-sha1",
           f"{a.ssh_user}@{host}"]

    print(); log("=" * 64)
    log(f"POWER-CYCLE the board now. It boots stock Linux, DHCPs, and takes {host}.")
    log("=" * 64); print()

    log(f"waiting up to {a.wait_secs}s for stock Linux at {host} over ssh...")
    deadline = time.time() + a.wait_secs
    reached = False
    while time.time() < deadline:
        try:
            if subprocess.run(ssh + ["true"], capture_output=True,
                              timeout=15).returncode == 0:
                reached = True
                break
        except subprocess.TimeoutExpired:
            pass
        time.sleep(3)
    if not reached:
        stop.set()
        sys.exit(f"!! never reached the stock Linux at {host} — is it powered, on "
                 "the link, and did the DHCP log show it take the lease?")
    log(f"stock Linux up at {host}; arming bootcmd = 'run tst; run oldbootcmd'")

    # The stock BusyBox has no `command` builtin, and fw_setenv is /usr/bin (a
    # symlink to fw_printenv). Save the ORIGINAL bootcmd once (guard against
    # re-arming clobbering it) and set a simple two-step bootcmd: try our netboot,
    # then fall back to the stock boot via `run oldbootcmd` — U-Boot executes that
    # saved variable natively, so the complex boot-counter script round-trips
    # without any risky re-quoting. Recovery from a hung kernel: stop this server
    # and power-cycle -> `run tst` fails with no TFTP -> `run oldbootcmd` -> stock.
    remote = (
        '[ -x /usr/bin/fw_setenv ] || { echo NO_FW_SETENV; exit 1; }; '
        'cur=$(/usr/bin/fw_printenv -n bootcmd); '
        'case "$cur" in '
        '  *"run tst"*) echo "already armed (keeping saved oldbootcmd)" ;; '
        '  *) /usr/bin/fw_setenv oldbootcmd "$cur" || { echo SAVE_FAILED; exit 1; } ;; '
        'esac; '
        "/usr/bin/fw_setenv bootcmd 'run tst; run oldbootcmd' && "
        'echo ARMED && sync && reboot'
    )
    r = subprocess.run(ssh + [remote], capture_output=True, text=True, timeout=30)
    if r.stdout.strip():
        print(r.stdout.strip())
    if "ARMED" not in r.stdout:
        stop.set()
        sys.exit(f"!! failed to arm the netboot: {r.stderr.strip() or r.stdout.strip()}")
    log("armed + rebooting. Watch below for U-Boot to DHCP then TFTP hammer/uImage")
    log("(a 'tftp: RRQ hammer/uImage' line = U-Boot fetched it and is about to bootm).")

    try:
        time.sleep(a.boot_secs)
    except KeyboardInterrupt:
        pass
    stop.set()
    log("done watching. To recover/retry: leave this running and power-cycle "
        "(re-netboots), or stop it and power-cycle (falls back to stock Linux).")


def boot_mode_c4mfg(a, log):
    """CA-1 mfg-mode netboot: serve DHCP (cookie + fdt option) and TFTP, then
    wait while the box does the rest itself. No serial or SSH needed — holding
    the ID button at power-on makes U-Boot DHCP, take our cookie+fdt+bootfile,
    TFTP the kernel and DTB, and bootz. Nothing is written to flash, so a bad
    kernel is just a power-cycle back to stock. Ctrl-C to stop serving."""
    import threading

    images = os.path.realpath(a.images)
    need = [a.bootfile, a.fdt_file]
    for f in need:
        if not os.path.isfile(os.path.join(images, f)):
            sys.exit(f"missing {os.path.join(images, f)} — run 'make image BOARD={a.board}' first")

    stop = threading.Event()
    threading.Thread(target=tftp_dir_serve, args=(images, log, stop), daemon=True).start()
    threading.Thread(target=bootp_responder, args=(a, log, stop), daemon=True).start()
    time.sleep(0.4)

    print()
    log("=" * 62)
    log("HOLD THE ID BUTTON (rear) and power-cycle the CA-1.")
    log("The ID button is NOT the recessed factory-restore button (that one")
    log("reimages stock from SPI-NOR). U-Boot prints 'MFG: Active' when it")
    log("takes the button; then watch the DHCP/TFTP log below.")
    log("Nothing here writes flash — power-cycle with no button to return to stock.")
    log("=" * 62)
    print()
    log(f"serving {a.bootfile} + {a.fdt_file} from {images}; DHCP cookie={a.cookie!r}")
    log("waiting for the box (Ctrl-C to stop)...")
    try:
        while not stop.is_set():
            time.sleep(1)
    except KeyboardInterrupt:
        stop.set()
        log("stopped.")


def boot_mode(a, log):
    """BOOTP + TFTP + automated bring-up, one command. UART boards drive the
    bootloader over the serial console; UART-less boards (uart=False in BOARDS)
    drive a self-reverting one-shot netboot over SSH instead."""
    if getattr(a, "dhcp", "cefdk") == "c4mfg":
        return boot_mode_c4mfg(a, log)
    if not getattr(a, "uart", True):
        return boot_mode_ssh(a, log)
    import threading

    images = os.path.realpath(a.images)
    for f in ("bzImage", "rootfs.cpio.gz"):
        if not os.path.isfile(os.path.join(images, f)):
            sys.exit(f"missing {os.path.join(images, f)} — run 'make image BOARD=...' first")

    try:
        port = a.serial_port or find_serial_port()
        sh = Shell(port)
    except ImportError:
        sys.exit("boot mode needs pyserial:  python3 -m pip install --user pyserial")

    log(f"board={a.board or 'custom'} iface={a.iface_ip} mac={a.client_mac} offer={a.offer_ip}")
    log(f"console={port} @115200 -> {a.console_baud} after handoff")

    stop = threading.Event()
    threading.Thread(target=tftp_dir_serve, args=(images, log, stop), daemon=True).start()
    threading.Thread(target=bootp_responder, args=(a, log, stop), daemon=True).start()
    time.sleep(0.4)

    print()
    log("=" * 62)
    log("HOLD THE ID BUTTON (rear) and power-cycle, or warm-reboot the unit.")
    log("The ID button is NOT the recessed recovery button — that one")
    log("factory-restores from p2. The banner will say which you got.")
    log("=" * 62)
    print()

    log(f"waiting up to {a.wait_secs}s for the CEFDK shell...")
    if not sh.wait_for_shell(a.wait_secs, log):
        stop.set()
        sys.exit("!! never reached 'shell>' — see docs/bootloader-access.md "
                 "(wrong button, or BOOTP never arrived)")
    log("at the unlocked shell")

    try:
        shell_bootlinux(sh, a.iface_ip, images, a.cmdline, a.console_baud,
                        a.boot_secs, log, dry_run=a.dry_run)
    finally:
        stop.set()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iface-ip", help="this machine's IP on the segment the controller is on")
    # No defaults here: a --board preset has to be able to fill these in, and an
    # EA1 default silently applied to an EA3 means netboot.py ignores every
    # request (the responder filters on MAC) with no obvious symptom.
    ap.add_argument("--client-mac", help="controller MAC (default: EA1's, or from --board)")
    ap.add_argument("--offer-ip", help="address to hand the controller (or from --board)")
    ap.add_argument("--netmask", default="255.255.255.0")
    # tftpd: JUST a TFTP server, no DHCP. For the shell-boot path — the mfg auto
    # path enforces an RSA signature (SOFT_HANG on a kernel we can't sign), but
    # the CEFDK shell's `bootlinux` does NOT verify. So: get to the shell via
    # `probe`, then from the shell `tftp get <this-ip> 0xc0140000 bzImage` (this
    # server) and `bootlinux "<cmdline>"`.
    ap.add_argument("mode", choices=["observe", "probe", "serve", "tftpd", "shellboot", "boot"])
    ap.add_argument("--kernel", help="bzImage to serve (serve/tftpd mode)")
    ap.add_argument("--bootfile", help="DHCP bootfile name (default: from --board, else vmlinuz.xz)")
    ap.add_argument("--fdt-file", help="DTB filename served via DHCP option 43 (c4mfg; from --board)")
    ap.add_argument("--cookie", default="C4_COOKIE", help="option-60 cookie value the board checks")
    ap.add_argument("--any-client", action="store_true",
                    help="reply to the MAC even without CEFDK's 'c4_*' vendor class "
                         "(default: ignore the stock OS's normal DHCP)")
    # boot mode
    ap.add_argument("--board", choices=sorted(BOARDS), help="preset iface-ip / MAC / offer-ip / console baud")
    ap.add_argument("--dhcp-style", choices=["cefdk", "plain", "c4mfg"],
                    help="DHCP reply style: 'cefdk' (option-60 cookie, EA/Intel), "
                         "'plain' (real OFFER/ACK for stock U-Boot, DM355), or "
                         "'c4mfg' (CA-1: OFFER/ACK + option-60 cookie + option-43 "
                         "fdt_file). Default: from --board, else cefdk.")
    ap.add_argument("--iface", help="restrict DHCP to this host interface (the "
                    "P2P link) so the main LAN's DHCP is ignored and replies stay "
                    "on the link. Default: auto-derived from the iface-ip.")
    ap.add_argument("--uart", dest="uart", action="store_true", default=None,
                    help="board HAS a serial console (boot drives it over UART)")
    ap.add_argument("--no-uart", dest="uart", action="store_false",
                    help="board has NO serial console (boot drives it over SSH)")
    ap.add_argument("--ssh-user", help="ssh user for the UART-less boot (from --board)")
    ap.add_argument("--ssh-pass", help="ssh password for the UART-less boot (from --board)")
    ap.add_argument("--images", default="output/images", help="dir with bzImage + rootfs.cpio.gz (boot mode)")
    ap.add_argument("--serial-port", help="console device; autodetected if omitted")
    ap.add_argument("--console-baud", type=int, help="kernel console baud (CE5310 default 921600)")
    ap.add_argument("--cmdline",
                    help="kernel command line (default: from --board; see BOARDS)")
    ap.add_argument("--boot-secs", type=int, default=120, help="seconds to stream the boot log")
    ap.add_argument("--wait-secs", type=int, default=300, help="how long to wait for the shell")
    ap.add_argument("--dry-run", action="store_true", help="stage everything but do not bootlinux")
    a = ap.parse_args()

    # A board preset fills in anything not given explicitly.
    prof = BOARDS.get(a.board, {})
    for k in ("iface_ip", "client_mac", "offer_ip"):
        if getattr(a, k, None) in (None, "") and k in prof:
            setattr(a, k, prof[k])
    a.dhcp = a.dhcp_style or prof.get("dhcp", "cefdk")
    if not a.bootfile:
        a.bootfile = prof.get("bootfile", "vmlinuz.xz")
    if not a.fdt_file:
        a.fdt_file = prof.get("fdt_file", "")
    if a.uart is None:
        a.uart = prof.get("uart", True)
    if not a.iface:
        a.iface = prof.get("iface") or (iface_for_ip(a.iface_ip) if a.iface_ip else None)
    if not a.ssh_user:
        a.ssh_user = prof.get("ssh_user", "root")
    if not a.ssh_pass:
        a.ssh_pass = prof.get("ssh_pass")
    if not a.kernel and a.board:
        cand = os.path.join(a.images, f"openhc-{a.board}-kernel.img")
        if os.path.isfile(cand):
            a.kernel = cand
    if a.console_baud is None:
        a.console_baud = prof.get("console_baud", 921600)
    if not a.cmdline:
        a.cmdline = prof.get("cmdline", _CMDLINE_BASE + " pci=realloc")
    if not a.iface_ip:
        ap.error("--iface-ip is required (or use --board)")
    if not a.client_mac:
        a.client_mac = "00:0f:ff:1a:fc:a9"
    if not a.offer_ip:
        a.offer_ip = "192.168.1.50"

    if a.mode in ("serve", "tftpd", "shellboot") and not (a.kernel and os.path.isfile(a.kernel)):
        sys.exit(f"{a.mode} mode needs --kernel pointing at a real file")

    def _log(m):
        print(f"[{time.strftime('%H:%M:%S')}] {m}", flush=True)

    if a.mode == "boot":
        return boot_mode(a, _log)

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

    bootp_responder(a, log)


def bootp_responder(a, log, stop=None):
    """Answer the controller's BOOTP. Blocks; pass `stop` to run it in a thread.

    Only ever replies to a.client_mac. That matters on a live network: this
    binds :67 alongside the real DHCP server, and without the MAC filter it
    would hand bogus leases to everything on the segment. The real server also
    answers CEFDK, but its reply has no option-60 cookie, so CEFDK rejects it
    and retries — ours wins on one of the 12 attempts.
    """
    want_mac = mac_bytes(a.client_mac)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
    s.bind(("", 67))
    if stop is not None:
        s.settimeout(0.5)

    # Bind to one interface (the P2P link) if we know it. macOS has no
    # SO_BINDTODEVICE, but IP_BOUND_IF binds the socket to one interface for BOTH
    # directions: we receive only that link's DHCP, and -- crucially -- our
    # broadcast replies EGRESS that interface. U-Boot (pre-IP) only accepts the
    # 255.255.255.255 limited broadcast; without IP_BOUND_IF that leaves via the
    # default route (Wi-Fi) and never reaches the board on the P2P link, so its
    # dhcp never completes. Fail-open if we cannot set it.
    IP_BOUND_IF = 25  # <netinet/in.h> on Darwin
    bound = None
    if getattr(a, "iface", None):
        try:
            s.setsockopt(socket.IPPROTO_IP, IP_BOUND_IF, socket.if_nametoindex(a.iface))
            bound = a.iface
        except OSError as e:
            log(f"  (cannot bind to {a.iface!r}: {e}; using all interfaces)")

    scope = f" on {bound}" if bound else ""
    log(f"DHCP up on :67 ({a.mode} mode){scope}, answering only {a.client_mac}")
    if a.mode in ("serve", "boot"):
        log(f"will offer {a.offer_ip}, next-server {a.iface_ip}, bootfile {a.bootfile!r}")

    while True:
        if stop is not None and stop.is_set():
            return
        try:
            data, src = s.recvfrom(2048)
        except socket.timeout:
            continue
        except OSError:
            return
        if len(data) < 240 or data[:1] != b"\x01":   # BOOTREQUEST only
            continue
        xid, flags = struct.unpack("!I", data[4:8])[0], struct.unpack("!H", data[10:12])[0]
        chaddr = data[28:34]
        opts = parse_options(data[240:])
        mtype = opts.get(53, b"\x00")[0]
        if chaddr != want_mac:
            continue                    # ignore the rest of the segment silently
        log(f"{MSG_TYPES.get(mtype, mtype)} from {mac_str(chaddr)}:")
        print(describe(opts), flush=True)

        # The MAC is not enough. The SAME MAC runs a normal Linux dhclient once
        # the stock image boots, and answering that is actively harmful: our
        # reply is BOOTP-shaped with no DHCP ACK, so dhclient rejects it, retries
        # in a tight loop, and the controller never gets an address at all —
        # which also costs us the SSH we need to warm-reboot it.
        #
        # CEFDK announces itself in manufacturing mode with vendor-class
        # 'c4_010' (option 60) and asks for 1,3,6,60. Linux sends a hostname and
        # a much longer request list. Match on the vendor class.
        style = getattr(a, "dhcp", "cefdk")
        plain = style == "plain"
        c4mfg = style == "c4mfg"

        vclass = opts.get(60, b"")
        if not plain and not (a.any_client or vclass.startswith(b"c4_")):
            log(f"  (ignoring — vendor-class {vclass!r} is not mfg mode;"
                f" this is the stock OS asking for a normal lease)")
            continue

        if c4mfg:
            # CA-1 stock U-Boot mfg mode: a real DHCP exchange (like 'plain', so
            # DISCOVER->OFFER / REQUEST->ACK with server-id + lease) that ALSO
            # carries the two vendor options U-Boot's dhcp_vendorex_proc reads to
            # set dhcp_mfgmode=3:
            #   opt 60 = "C4_COOKIE" EXACTLY (9 bytes, NO trailing NUL — the box
            #            checks oplen==strlen("C4_COOKIE") && strncmp; a NUL makes
            #            oplen=10 and it fails). Contrast CEFDK, which needs the NUL.
            #   opt 43 = vendor-encapsulated, sub-option 0x0a = the DTB filename
            #            (the box memcpy's voptlen bytes into fdt_file; the NUL is
            #            counted in the length per the source comment).
            # siaddr (next-server) = TFTP host; the file field = the kernel name.
            req_ip = opts.get(50)
            if mtype == 3 and req_ip and socket.inet_ntoa(req_ip) != a.offer_ip:
                resp = 6   # NAK a stale requested-IP so it re-DISCOVERs (see plain)
                reply = build_reply(xid, chaddr, "0.0.0.0",
                                    [(53, bytes([6])),
                                     (54, socket.inet_aton(a.iface_ip))],
                                    flags, a.iface_ip)
            else:
                resp = {1: 2, 3: 5}.get(mtype, 0)   # DISCOVER->OFFER, REQUEST->ACK
                fdt = a.fdt_file.encode() + b"\x00"
                opt43 = bytes([0x0a, len(fdt)]) + fdt      # sub-opt 0x0a = fdt_file
                base = []
                if resp:
                    base += [(53, bytes([resp])),
                             (54, socket.inet_aton(a.iface_ip)),
                             (51, struct.pack("!I", 86400))]
                base += [(1, socket.inet_aton(a.netmask)),
                         (3, socket.inet_aton(a.iface_ip)),
                         (60, a.cookie.encode()),          # "C4_COOKIE", no NUL
                         (43, opt43)]
                reply = build_reply(xid, chaddr, a.offer_ip, base, flags,
                                    a.iface_ip, next_server=a.iface_ip,
                                    bootfile=a.bootfile)
        elif plain:
            # Stock U-Boot (DM355): a real DHCP exchange, no CEFDK cookie.
            # U-Boot runs the state machine, so answer DISCOVER with an OFFER and
            # REQUEST with an ACK, each carrying the server-id (54) and a lease
            # (51) so U-Boot accepts them; a pure BOOTP request (no message type)
            # gets a plain reply. siaddr + file give it the TFTP server/name.
            req_ip = opts.get(50)   # option 50 = Requested IP Address
            if mtype == 3 and req_ip and socket.inet_ntoa(req_ip) != a.offer_ip:
                # The client is clinging to a stale lease (e.g. its old 10.0.0.x
                # address after moving to this P2P link) we cannot honor. RFC 2131
                # says NAK it -> the client drops that lease, DISCOVERs, and then
                # accepts our offer_ip. Without this it just re-REQUESTs forever.
                resp = 6   # NAK
                reply = build_reply(xid, chaddr, "0.0.0.0",
                                    [(53, bytes([6])),
                                     (54, socket.inet_aton(a.iface_ip))],
                                    flags, a.iface_ip)
            else:
                resp = {1: 2, 3: 5}.get(mtype, 0)   # DISCOVER->OFFER, REQUEST->ACK
                base = []
                if resp:
                    base += [(53, bytes([resp])),
                             (54, socket.inet_aton(a.iface_ip)),   # DHCP server id
                             (51, struct.pack("!I", 86400))]       # lease: 1 day
                base += [(1, socket.inet_aton(a.netmask)),
                         (3, socket.inet_aton(a.iface_ip))]        # gateway = us
                reply = build_reply(xid, chaddr, a.offer_ip, base, flags,
                                    a.iface_ip, next_server=a.iface_ip,
                                    bootfile=a.bootfile)
        else:
            # CEFDK parses only tags 1 (subnet), 3 (gateway) and 60 (the cookie);
            # everything else hits its "don't support the tag" default and is
            # ignored, so keep the reply to what it reads.
            base = [
                (1, socket.inet_aton(a.netmask)),
                (3, socket.inet_aton(a.iface_ip)),           # gateway = us
            ]
            if a.mode in ("probe", "serve", "shellboot", "boot"):
                # Option 60 value must strcmp-equal the cookie. CEFDK copies 10
                # bytes into a zeroed buffer, so send the NUL terminator
                # explicitly. Without this CEFDK never enters mfg mode at all.
                base.append((60, a.cookie.encode() + b"\x00"))

            # siaddr (server IP for TFTP) and the file field carry the rest.
            reply = build_reply(xid, chaddr, a.offer_ip, base, flags,
                                a.iface_ip, next_server=a.iface_ip,
                                bootfile=a.bootfile)

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
        if c4mfg:
            kind = {2: "OFFER", 5: "ACK", 6: "NAK"}.get(resp, "BOOTP reply")
            tgt = "0.0.0.0" if resp == 6 else a.offer_ip
            log(f"  -> {kind} {tgt}, siaddr={a.iface_ip}, cookie={a.cookie!r},"
                f" opt43 fdt_file={a.fdt_file!r}, file={a.bootfile!r} (CA-1 mfg mode)")
        elif plain:
            kind = {2: "OFFER", 5: "ACK", 6: "NAK"}.get(resp, "BOOTP reply")
            tgt = "0.0.0.0" if resp == 6 else a.offer_ip
            log(f"  -> {kind} {tgt}, siaddr={a.iface_ip} (plain DHCP, no cookie)")
        elif a.mode == "observe":
            log(f"  -> reply {a.offer_ip}, NO cookie (recon only — CEFDK stays out of mfg mode)")
        else:
            log(f"  -> reply {a.offer_ip}, cookie={a.cookie!r}, siaddr={a.iface_ip}, file={a.bootfile!r}"
                + ("  [probe: no TFTP -> expect drop to unlocked shell]" if a.mode == "probe" else ""))

if __name__ == "__main__":
    main()
