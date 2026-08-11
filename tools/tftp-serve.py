#!/usr/bin/env python3
"""Minimal read-only TFTP server for netbooting the EA1 from the CEFDK shell.

CEFDK's `tftp get <server-ip> <ram-addr> <file>` pulls a file straight into RAM.
This serves those files. Read requests only (the shell never writes back).

Port 69 is privileged, so run with sudo:

    sudo python3 tools/tftp-serve.py output/images --ip 192.168.1.5

Design notes (learned the hard way against this CEFDK):
- Bind ("", 69), NOT a specific IP: a specific-IP UDP bind can silently fail to
  receive on multi-homed macOS.
- THREAD per transfer, never os.fork(). fork() on macOS is fork-unsafe and was
  crashing the whole server mid-run during CEFDK's manufacturing-mode request
  storm. Threads share the process cleanly; the accept loop never blocks.
- Not-found (CEFDK's mfg auto-fetch of vmlinuz.xz hammers this during shell
  entry) is answered INLINE in the accept loop — instant, no thread.
- The accept loop catches everything and can never die.
- Cap blksize: CEFDK asks 47040, which macOS can't send in one datagram and
  CEFDK can't reassemble. Answer with <=1468 (one Ethernet frame).
- octet mode only.
"""
import argparse, os, socket, struct, sys, threading, time

RRQ, WRQ, DATA, ACK, ERROR, OACK = 1, 2, 3, 4, 5, 6
MAX_BLK = 1468  # 1500 MTU - 20 IP - 8 UDP - 4 TFTP


def parse_rrq(pkt):
    parts = pkt[2:].split(b"\x00")
    fname = parts[0].decode("latin1")
    mode = (parts[1].decode("latin1").lower() if len(parts) > 1 else "octet")
    opts, rest = {}, parts[2:]
    for i in range(0, len(rest) - 1, 2):
        if rest[i]:
            opts[rest[i].decode("latin1").lower()] = rest[i + 1].decode("latin1")
    return fname, mode, opts


def transfer(root, cli, pkt, retries=6, timeout=1.5):
    """One transfer on its own ephemeral socket, in its own thread."""
    fname, mode, opts = parse_rrq(pkt)
    t = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        t.bind(("", 0)); t.settimeout(timeout)
        path = os.path.realpath(os.path.join(root, fname.lstrip("/")))
        if not path.startswith(root) or not os.path.isfile(path):
            t.sendto(struct.pack(">HH", ERROR, 1) + b"not found\x00", cli); return
        size = os.path.getsize(path)

        blksize, neg = 512, {}
        if "blksize" in opts:
            blksize = max(8, min(int(opts["blksize"]), MAX_BLK)); neg["blksize"] = str(blksize)
        if "tsize" in opts:
            neg["tsize"] = str(size)
        if "timeout" in opts:
            neg["timeout"] = opts["timeout"]

        def wait_ack(want):
            for _ in range(retries):
                try:
                    d, a = t.recvfrom(2048)
                except socket.timeout:
                    return False
                except OSError:
                    return False
                if a == cli and len(d) >= 4 and struct.unpack(">H", d[:2])[0] == ACK \
                   and struct.unpack(">H", d[2:4])[0] == (want & 0xFFFF):
                    return True
            return False

        t0 = time.monotonic()
        print(f"  -> {fname} ({size} bytes, blksize={blksize}) to {cli[0]}")
        if neg:
            oack = struct.pack(">H", OACK)
            for k, v in neg.items():
                oack += k.encode() + b"\x00" + v.encode() + b"\x00"
            for _ in range(retries):
                t.sendto(oack, cli)
                if wait_ack(0):
                    break
            else:
                print(f"  !! {fname}: no ACK0 for OACK"); return

        with open(path, "rb") as f:
            block = 1
            while True:
                f.seek((block - 1) * blksize)
                chunk = f.read(blksize)
                data_pkt = struct.pack(">HH", DATA, block & 0xFFFF) + chunk
                ok = False
                for _ in range(retries):
                    t.sendto(data_pkt, cli)
                    if wait_ack(block):
                        ok = True; break
                if not ok:
                    print(f"  !! {fname}: block {block} unacked, abort"); return
                if len(chunk) < blksize:
                    break
                block += 1
        print(f"  ok {fname}: {size} bytes in {time.monotonic()-t0:.1f}s")
    except Exception as e:
        print(f"  !! {fname}: transfer error: {e}")
    finally:
        t.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("root", help="directory to serve (e.g. output/images)")
    ap.add_argument("--ip", default="", help="informational only; server binds all interfaces")
    ap.add_argument("--port", type=int, default=69)
    args = ap.parse_args()
    root = os.path.realpath(args.root)
    if not os.path.isdir(root):
        sys.exit(f"not a directory: {root}")

    srv = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        srv.bind(("", args.port))
    except PermissionError:
        sys.exit(f"bind :{args.port} needs root — rerun with sudo")
    print(f"tftp: serving {root} on {args.ip or 'all interfaces'}:{args.port}  (Ctrl-C to stop)")

    while True:
        try:
            pkt, cli = srv.recvfrom(2048)
            if len(pkt) < 2:
                continue
            op = struct.unpack(">H", pkt[:2])[0]
            if op == RRQ:
                fname = pkt[2:].split(b"\x00")[0].decode("latin1")
                path = os.path.realpath(os.path.join(root, fname.lstrip("/")))
                if not path.startswith(root) or not os.path.isfile(path):
                    # mfg auto-fetch storm — answer inline, no thread
                    e = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                    e.sendto(struct.pack(">HH", ERROR, 1) + b"not found\x00", cli); e.close()
                    print(f"refused {fname!r} (expected for the mfg auto-fetch)")
                    continue
                print(f"RRQ {fname} from {cli[0]}:{cli[1]} -> thread")
                threading.Thread(target=transfer, args=(root, cli, pkt), daemon=True).start()
            elif op == WRQ:
                e = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                e.sendto(struct.pack(">HH", ERROR, 2) + b"read-only\x00", cli); e.close()
        except KeyboardInterrupt:
            print("\nbye"); return
        except Exception as ex:
            print(f"  !! accept-loop error (surviving): {ex!r}")


if __name__ == "__main__":
    main()
