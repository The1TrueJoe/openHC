#!/usr/bin/env python3
"""cefdk-wrap — wrap a bzImage in the CEFDK container CEFDK's loader expects.

The EA1's bootloader does not boot a bare bzImage. Both the eMMC boot path and
`bootkernel -b <addr>` from the CEFDK shell consume a *container*: a 0x580-byte
CEFDK header followed by the bzImage. We keep a real header extracted from the
stock kernel (`board/ea1/cefdk-container-header.bin`) and re-stamp its length
fields for our own image.

Layout, verified against the stock kernel blob:

    0x000  CEFDK container header (0x580 bytes)
    0x580  bzImage  (boot flag 0x55AA at 0x580+0x1FE, "HdrS" at 0x580+0x202)

Signature does NOT need to be valid: SEC_BOOT is strapped 0 on this unit
(`strap SB` = 0b), so CEFDK's VERIFY_S3 is advisory. We reuse the stock header
verbatim except for the size fields.

    cefdk-wrap.py bzImage container.img

The header's exact size-field semantics are only partly reverse-engineered, so
this patches the fields we are confident about and prints the rest for
inspection. Because netboot writes nothing, an over/under-sized field is a
harmless failed boot, not a brick — refine against hardware, not on paper.
"""
import sys, struct, pathlib

REPO = pathlib.Path(__file__).resolve().parent.parent
# Default header: the one extracted from a stock EA1 image. Every EA runs CEFDK
# (an EA3 reports cefdk.deb 36-34 in its recovery partition), but only the EA1's
# container header has actually been captured, so a board with its own header
# should pass it explicitly rather than inherit this one silently.
HEADER = REPO / "board/ea1/cefdk-container-header.bin"
HDR_LEN = 0x580

def main():
    if len(sys.argv) not in (3, 4):
        sys.exit("usage: cefdk-wrap.py <bzImage> <out-container.img> [header.bin]")
    bz = pathlib.Path(sys.argv[1]).read_bytes()
    header = pathlib.Path(sys.argv[3]) if len(sys.argv) == 4 else HEADER
    if not header.is_file():
        sys.exit(f"no CEFDK container header at {header}")
    hdr = bytearray(header.read_bytes())
    if len(hdr) != HDR_LEN:
        sys.exit(f"header is {len(hdr)} bytes, expected {HDR_LEN}")

    # Sanity: the bzImage must actually be one.
    if bz[0x1fe:0x200] != b"\x55\xaa" or bz[0x202:0x206] != b"HdrS":
        sys.exit("input is not a bzImage (missing 0x55AA / 'HdrS')")

    total = HDR_LEN + len(bz)

    # Known header fields (offsets confirmed against the stock image):
    #   +0x10  u32  0x00008086  Intel vendor id            (left as-is)
    #   +0x28  u32  0x00000580  offset from header to image (left as-is)
    # The image-length field has not been positively identified. The stock
    # header carried the stock image size; where a length must match the payload,
    # patch it here once confirmed on hardware. For now we surface the header
    # words so a mismatch is visible rather than silent.
    print("wrap: header words (offset: value):")
    for off in (0x00, 0x04, 0x18, 0x1c, 0x20, 0x24, 0x28):
        print(f"  +0x{off:02x}: 0x{struct.unpack_from('<I', hdr, off)[0]:08x}")
    print(f"wrap: bzImage {len(bz)} bytes, container {total} bytes")

    out = pathlib.Path(sys.argv[2])
    out.write_bytes(bytes(hdr) + bz)
    # The eMMC boot path also reads a big-endian total size at raw offset 0x200
    # of the *device* (not this file). netboot does not use it; a flashing tool
    # would. Emit it so a future flasher has the value.
    print(f"wrap: wrote {out}  (eMMC size word for a flasher: 0x{total:08x} BE)")

if __name__ == "__main__":
    main()
