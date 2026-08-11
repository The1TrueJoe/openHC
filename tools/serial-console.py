#!/usr/bin/env python3
"""ohc-serial — talk to the EA1 CEFDK/kernel console over a USB-TTL adapter.

Not interactive by design: each run sends an optional line, then reads for a
fixed window and prints what came back. That suits a bootloader you break into
on a timer, and it lets an agent drive the console one turn at a time without
holding a terminal open.

    ohc-serial.py --listen 8                 # just read for 8s
    ohc-serial.py --send '' --read 5         # tap ENTER, read 5s
    ohc-serial.py --send 'help' --read 4     # run a command
    ohc-serial.py --send-raw 63,34 --read 4  # send raw bytes (c,4) no newline

--port defaults to the first cu.usbserial*; 115200 8N1.
"""
import argparse, glob, sys, time
import serial

def find_port():
    for pat in ("/dev/cu.usbserial*", "/dev/cu.usbmodem*", "/dev/tty.usbserial*"):
        m = sorted(glob.glob(pat))
        if m:
            return m[0]
    sys.exit("no USB-serial adapter found")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port")
    ap.add_argument("--baud", type=int, default=115200)
    ap.add_argument("--listen", type=float)
    ap.add_argument("--send")               # a line; newline appended
    ap.add_argument("--send-raw")           # comma-separated byte values, no newline
    ap.add_argument("--eol", default="\r")  # CEFDK wants CR
    ap.add_argument("--read", type=float, default=5.0)
    a = ap.parse_args()

    port = a.port or find_port()
    # No DTR toggle: cu.* already avoids it, and we never want to reset the far
    # end by opening the port.
    s = serial.Serial(port, a.baud, timeout=0.2)

    if a.listen is not None:
        deadline = time.monotonic() + a.listen
    else:
        if a.send_raw is not None:
            s.write(bytes(int(x) & 0xFF for x in a.send_raw.split(",")))
        elif a.send is not None:
            s.write(a.send.encode() + a.eol.encode())
        s.flush()
        deadline = time.monotonic() + a.read

    out = bytearray()
    while time.monotonic() < deadline:
        chunk = s.read(4096)
        if chunk:
            out += chunk
    s.close()
    sys.stdout.buffer.write(bytes(b for b in out if b != 0))
    sys.stdout.flush()

if __name__ == "__main__":
    main()
