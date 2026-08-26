#!/usr/bin/env python3
"""cefdk-wrap — wrap a bzImage in the CEFDK container CEFDK's loader expects.

The EA's bootloader does not boot a bare bzImage. Both the eMMC boot path and
`bootkernel -b <addr>` from the CEFDK shell consume a *container*: a 0x580-byte
header followed by the bzImage.

    0x000  CEFDK container header (0x580 bytes)
    0x580  bzImage  (boot flag 0x55AA at 0x580+0x1FE, "HdrS" at 0x580+0x202)

WHY THIS GENERATES THE HEADER INSTEAD OF SHIPPING ONE
-----------------------------------------------------
openHC used to carry `board/ea1/cefdk-container-header.bin`, 1408 bytes lifted
verbatim off a stock unit. Taking it apart:

    0x000-0x02f    ~48 B   format fields (vendor id, offsets, version, date)
    0x094-0x196    258 B   opaque, RSA-2048 shaped
    0x480-0x580    256 B   opaque, RSA-2048 shaped
    everything else        zeros

514 of those bytes are Control4/Intel *signature material*, and the date field
held Control4's own kernel build date (0x20220525 — the same 05/25/22 the stock
kernel banner reports). CEFDK is proprietary: per docs/gpl-source.md, 1,013 of
1,014 CEFDK source files carry the Intel CEFDK Software License Agreement, none
GPL or BSD. Shipping that blob in an MIT repo was the one thing in this tree
that could not be redistributed.

It also is not needed. The signature is only *checked* when secure boot is
enforced, and it is not on this hardware (docs/gpl-source.md reads SEC_BOOT_FUSE
straight out of the DFX unit: CLEAR). So we build the header from its format
fields and leave the signature regions zero. Nothing here is copied from
Control4 — these are measured interface constants, the same kind of thing any
file-format header is made of.

If a unit ever does enforce signatures, no generated header will help; you would
need Control4's private key, which is not in the GPL drop either. What you CAN
do on hardware you own is extract your own unit's header and pass it with
--header. That stays off this repo.

    cefdk-wrap.py <bzImage> <out-container.img> [--header extracted.bin]
"""
import argparse, struct, sys, pathlib, time

HDR_LEN = 0x580
IMAGE_OFF = 0x580        # +0x28: header -> image
SIG_OFF = 0x480          # +0x2c: where the trailing signature block sits

# Header format fields, read off a stock EA1 *and* EA3 (the two are byte
# identical, so this is the EA family's layout, not one unit's).
#
# Several are still unidentified. They are reproduced as the constants the
# bootloader was observed to accept, NOT copied for their content — a wrong
# guess here is a kernel CEFDK declines to load, which is a factory-restore
# button press, not a brick. If an eMMC boot fails and the bzImage itself is
# sound, +0x18 is the first thing to suspect: it is the only field whose value
# looks like it could be payload-dependent, and its meaning is not known.
FIELDS = {
    0x00: 0x00000006,    # format/version
    0x04: 0x000000a1,    # unidentified
    0x08: 0x00010000,    # unidentified
    0x0c: 0x80000001,    # unidentified (top bit set — flags?)
    0x10: 0x00008086,    # PCI vendor id, Intel
    0x14: None,          # build date, BCD-ish yyyymmdd — filled in at build time
    0x18: 0x001abc31,    # UNIDENTIFIED. see note above
    0x1c: 0x00000040,
    0x20: 0x00000040,
    0x24: 0x00000001,
    0x28: IMAGE_OFF,     # offset from header start to the bzImage
    0x2c: SIG_OFF,       # offset to the signature block
}
# A lone 0x01 sits immediately after the first signature region on stock images.
BYTE_0x196 = 0x01


def build_header(build_date=None):
    """Synthesise the 0x580-byte container header. Signature regions stay zero."""
    hdr = bytearray(HDR_LEN)
    if build_date is None:
        # Stored BCD-style — the stock header holds 0x20220525 for 2022-05-25 —
        # so parse today's yyyymmdd as hex. Ours, not the vendor's.
        build_date = int(time.strftime("%Y%m%d"), 16)
    for off, val in FIELDS.items():
        struct.pack_into("<I", hdr, off, build_date if val is None else val)
    hdr[0x196] = BYTE_0x196
    return hdr


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("bzimage")
    ap.add_argument("out")
    ap.add_argument("--header", help="use an extracted header instead of generating one "
                                     "(for a unit that enforces signatures; keep it out of git)")
    a = ap.parse_args()

    bz = pathlib.Path(a.bzimage).read_bytes()
    if bz[0x1fe:0x200] != b"\x55\xaa" or bz[0x202:0x206] != b"HdrS":
        sys.exit("input is not a bzImage (missing 0x55AA / 'HdrS')")

    if a.header:
        hdr = bytearray(pathlib.Path(a.header).read_bytes())
        if len(hdr) != HDR_LEN:
            sys.exit(f"header is {len(hdr)} bytes, expected {HDR_LEN}")
        print(f"wrap: using extracted header {a.header}")
    else:
        hdr = build_header()
        print("wrap: generated header (signature regions zero; "
              "SEC_BOOT is clear on this hardware)")

    total = HDR_LEN + len(bz)
    print("wrap: header words (offset: value):")
    for off in sorted(FIELDS):
        print(f"  +0x{off:02x}: 0x{struct.unpack_from('<I', hdr, off)[0]:08x}")
    print(f"wrap: bzImage {len(bz)} bytes, container {total} bytes")

    out = pathlib.Path(a.out)
    out.write_bytes(bytes(hdr) + bz)
    # The eMMC boot path reads a big-endian total size at raw offset 0x200 of the
    # *device* (not this file); tools/ea-emmc-install.py writes it.
    print(f"wrap: wrote {out}  (eMMC size word for the flasher: 0x{total:08x} BE)")


if __name__ == "__main__":
    main()
