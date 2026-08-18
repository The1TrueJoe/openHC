#!/usr/bin/env python3
"""Self-check for the CA-1 mfg-mode DHCP reply encoding (tools/netboot.py).

The wire format is exact and unforgiving: U-Boot's dhcp_vendorex_proc checks
option 60 == "C4_COOKIE" by length (a trailing NUL breaks it, unlike CEFDK) and
reads the DTB filename from option 43 sub-option 0x0a. Get either wrong and the
box never sets dhcp_mfgmode=3, so it silently falls through instead of netbooting.

Run:  python3 tools/test_netboot_ca1.py
"""
import importlib.util
import os
import socket
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("nb", os.path.join(HERE, "netboot.py"))
nb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(nb)


def build_ca1_ack(offer="192.168.1.178", host="192.168.1.171",
                  fdt_name="c4-imx6sl-ca1.dtb", kernel="openhc-ca1-zImage"):
    """Reproduce the option set a c4mfg ACK carries (mirrors bootp_responder)."""
    fdt = fdt_name.encode() + b"\x00"
    opt43 = bytes([0x0a, len(fdt)]) + fdt
    base = [(53, bytes([5])), (54, socket.inet_aton(host)),
            (51, struct.pack("!I", 86400)),
            (1, socket.inet_aton("255.255.255.0")),
            (3, socket.inet_aton(host)),
            (60, b"C4_COOKIE"), (43, opt43)]
    return nb.build_reply(0x1234, nb.mac_bytes("00:0f:ff:52:82:65"), offer,
                          base, 0x8000, host, next_server=host, bootfile=kernel)


def test_reply():
    pkt = build_ca1_ack()
    assert pkt[:1] == b"\x02", "op must be BOOTREPLY"
    assert socket.inet_ntoa(pkt[16:20]) == "192.168.1.178", "yiaddr"
    assert socket.inet_ntoa(pkt[20:24]) == "192.168.1.171", "siaddr (TFTP next-server)"
    assert pkt[108:236].split(b"\x00")[0].decode() == "openhc-ca1-zImage", "bootfile"
    opts = nb.parse_options(pkt[240:])              # options follow the 4-byte magic
    assert opts[60] == b"C4_COOKIE", f"opt60 must be exactly b'C4_COOKIE', got {opts[60]!r}"
    assert len(opts[60]) == 9, "cookie must be 9 bytes (NO trailing NUL)"
    v = opts[43]
    assert v[0] == 0x0a, "opt43 sub-option id must be 0x0a (fdt_file)"
    name = v[2:2 + v[1]].rstrip(b"\x00").decode()
    assert name == "c4-imx6sl-ca1.dtb", f"opt43 fdt_file name = {name!r}"


def test_board_preset():
    b = nb.BOARDS["ca1"]
    assert b["dhcp"] == "c4mfg" and b["uart"] is False
    assert b["bootfile"] == "openhc-ca1-zImage"
    assert b["fdt_file"] == "c4-imx6sl-ca1.dtb"


if __name__ == "__main__":
    test_reply()
    test_board_preset()
    print("ok: CA-1 c4mfg DHCP reply + board preset")
