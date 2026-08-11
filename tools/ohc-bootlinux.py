#!/usr/bin/env python3
"""Netboot a custom kernel on the EA1 through CEFDK's own `bootlinux`, unsigned.

How this works (see docs/content/bootloader-access.md): CEFDK's bootlinux command
copies a bzImage from RAM to 0x100000 and jumps to it after checking ONLY the
bzImage magics (0xAA55 / "HdrS") — no signature, no fuse. The load address it
uses is the global linuxKernelBase at 0x000c90a4, and the ramdisk comes from the
globals at 0x837560/564/568. All four are plain writable RAM. So:

  1. tftp the kernel + initrd into scratch DRAM,
  2. point linuxKernelBase at the kernel and the ramdisk globals at the initrd,
  3. run bootlinux.

No flash write, no brick risk, factory-restore untouched.

Prereq: a TFTP server on this machine serving the two files, e.g.
    sudo python3 tools/tftp-serve.py output/images --ip 192.168.1.5
Then:
    python3 tools/ohc-bootlinux.py --server 192.168.1.5 --images output/images
"""
import argparse, glob, os, sys, time, serial

# Verified layout. Kernel copied by the loader to 0x100000 (protmode <7.0MB, ours
# ~6.3MB); staged high so source never overlaps the copy. Initrd page-aligned so
# the loader uses it in place. Both inside e820 usable RAM (1-200MB).
KERNEL_ADDR = 0x06000000   # 96MB
INITRD_ADDR = 0x04000000   # 64MB, page-aligned
G_KBASE   = 0x000c90a4     # linuxKernelBase
G_RD_FLAG = 0x00837560     # ramdisk present flag
G_RD_ADDR = 0x00837564     # ramdisk address
G_RD_SIZE = 0x00837568     # ramdisk size


def find_port():
    ports = sorted(glob.glob("/dev/cu.usbserial*"))
    if not ports:
        sys.exit("no /dev/cu.usbserial* found")
    return ports[0]


class Shell:
    def __init__(self, port):
        self.s = serial.Serial(port, 115200, timeout=0.2)

    def cmd(self, c, wait=3.0, until=None):
        self.s.write(c.encode() + b"\r")
        out, end = "", time.monotonic() + wait
        while time.monotonic() < end:
            d = self.s.read(4096)
            if d:
                out += bytes(b for b in d if b != 0).decode("utf-8", "replace")
                if until and until in out:
                    break
                if out.rstrip().endswith("shell>"):
                    # give a beat for trailing bytes, then stop
                    end = min(end, time.monotonic() + 0.3)
        return out

    def at_shell(self):
        self.s.write(b"\r"); time.sleep(0.4)
        return b"shell>" in (self.s.read(4096) or b"")

    @staticmethod
    def _parse_val(out):
        # Skip the echoed command + prompt. Value is either bare hex on its own
        # line ("5220", "522051F1") or the labeled len form
        # ("0xADDR - 0xADDR: 0xVAL 0xVAL ...").
        for line in (l.strip() for l in out.replace("\r", "\n").split("\n")):
            if not line or line.startswith("ord") or line.startswith("shell"):
                continue
            if ":" in line:  # labeled form -> first value after the colon
                after = line.split(":", 1)[1].strip().split()
                if after:
                    try: return int(after[0], 16)
                    except ValueError: pass
            tok = line.split()[0].rstrip(",")  # bare form e.g. "522051F1"
            if tok.lower().startswith("0x"):
                tok = tok[2:]
            try: return int(tok, 16)
            except ValueError: continue
        return None

    def rd2(self, addr):
        return self._parse_val(self.cmd(f"ord2 {addr:#x}", 2))

    def rd4(self, addr):
        return self._parse_val(self.cmd(f"ord4 {addr:#x}", 2))

    def wr4(self, addr, val):
        self.cmd(f"ord4 {addr:#x} = {val:#x}", 1.5)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", required=True, help="TFTP server IP (this machine's link IP)")
    ap.add_argument("--images", default="output/images", help="dir with bzImage + rootfs.cpio.gz")
    ap.add_argument("--kernel", default="bzImage")
    ap.add_argument("--initrd", default="rootfs.cpio.gz")
    ap.add_argument("--cmdline", default="console=ttyS0,115200 earlyprintk=serial,ttyS0,115200")
    ap.add_argument("--port", default=None)
    ap.add_argument("--console-baud", type=int, default=115200,
                    help="reopen the serial port at this baud to read the kernel console "
                         "after bootlinux. The CE5310 UART clock is 8x standard, so a "
                         "mainline kernel asked for 115200 actually drives 921600.")
    ap.add_argument("--boot-secs", type=int, default=120, help="seconds to stream the boot log")
    ap.add_argument("--dry-run", action="store_true", help="stage + set globals, do NOT bootlinux")
    args = ap.parse_args()

    initrd_path = os.path.join(args.images, args.initrd)
    if not os.path.isfile(initrd_path):
        sys.exit(f"missing {initrd_path}")
    initrd_size = os.path.getsize(initrd_path)

    sh = Shell(args.port or find_port())
    if not sh.at_shell():
        sys.exit("not at CEFDK shell> — interrupt boot into the shell first")
    print("[*] at shell")

    print(f"[*] ping {args.server}")
    if "Reply from" not in sh.cmd(f"ping {args.server} 2", 8):
        print("    !! no ping reply — check the link / `ip set`. continuing anyway")

    print(f"[*] tftp kernel  {args.kernel} -> {KERNEL_ADDR:#x}")
    sh.cmd(f"tftp get {args.server} {KERNEL_ADDR:#x} {args.kernel}", 180, until="shell>")
    m = sh.rd2(KERNEL_ADDR + 0x1fe)
    print(f"    bzImage boot magic @+0x1fe = {m:#06x} (want 0xaa55)" if m is not None else "    (magic read failed)")
    if m != 0xAA55:
        sys.exit("    !! kernel not staged correctly (bad magic). aborting before boot.")

    print(f"[*] tftp initrd  {args.initrd} -> {INITRD_ADDR:#x}  ({initrd_size} bytes)")
    sh.cmd(f"tftp get {args.server} {INITRD_ADDR:#x} {args.initrd}", 120, until="shell>")
    g = sh.rd2(INITRD_ADDR)
    print(f"    gzip magic @initrd = {g:#06x} (want 0x8b1f)" if g is not None else "    (magic read failed)")
    if g != 0x8B1F:
        sys.exit("    !! initrd not staged correctly (bad gzip magic). aborting.")

    print("[*] set globals")
    sh.wr4(G_KBASE, KERNEL_ADDR)
    sh.wr4(G_RD_FLAG, 1)
    sh.wr4(G_RD_ADDR, INITRD_ADDR)
    sh.wr4(G_RD_SIZE, initrd_size)
    # read back
    ok = True
    for name, addr, want in [("linuxKernelBase", G_KBASE, KERNEL_ADDR),
                             ("rd_flag", G_RD_FLAG, 1),
                             ("rd_addr", G_RD_ADDR, INITRD_ADDR),
                             ("rd_size", G_RD_SIZE, initrd_size)]:
        got = sh.rd4(addr)
        mark = "ok" if got == want else "MISMATCH"
        if got != want: ok = False
        print(f"    {name:16s} {addr:#010x} = {got:#x} (want {want:#x}) {mark}")
    if not ok:
        sys.exit("    !! a global didn't stick. aborting before boot.")

    if args.dry_run:
        print("[*] --dry-run: everything staged. Run `bootlinux \"%s\"` on the shell to boot." % args.cmdline)
        return

    print(f"[*] bootlinux \"{args.cmdline}\"")
    sh.s.write(f'bootlinux "{args.cmdline}"\r'.encode())
    time.sleep(1.0)  # let CEFDK echo + hand off
    if args.console_baud != 115200:
        print(f"[*] reopening console at {args.console_baud} baud (CE5310 UART is 8x)")
        port = args.port or find_port()
        sh.s.close()
        time.sleep(0.3)
        sh.s = serial.Serial(port, args.console_baud, timeout=0.2)
    print(f"[*] streaming kernel console for {args.boot_secs}s")
    print("=" * 70)
    binf = os.path.realpath(os.path.join(args.images, "..", "boot-console.bin"))
    with open(binf, "wb") as lf:
        end = time.monotonic() + args.boot_secs
        while time.monotonic() < end:
            d = sh.s.read(4096)
            if d:
                lf.write(d); lf.flush()
                # display: latin1 keeps every byte 1:1, printable stays readable
                sys.stdout.write(d.decode("latin1")); sys.stdout.flush()
    print("\n" + "=" * 70)
    print(f"[*] raw boot log saved -> {binf}")


if __name__ == "__main__":
    main()
