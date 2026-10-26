# em357 — the Zigbee radio's NCP firmware

Every openHC board with Zigbee (all of them except the IOX V1) carries a Silicon
Labs EM357 running as an EZSP network co-processor. Control4 ships it on
EmberZNet 4.7.2 (EZSP v4), which zigbee2mqtt can't speak. openHC reflashes it at
boot to **EmberZNet 6.7.8 (EZSP v8)**, the last stack built for the EM35x. That
is the same thing Control4's own `zap` daemon does with its image.

| File | What |
|---|---|
| `em357-ncp.source` | The pinned image: URL at a fixed commit, sha256, stack version |
| `install.sh` | Fetches it into `dl/`, checks the sha256, installs it into the target |

The binary is not checked in. `board/common/post-build.sh` runs `install.sh` on
any image that ships `ohc-zigbee` (the `zigbee` feature). The script installs:

```text
/usr/share/ohc/zigbee/em357-ncp.ebl           the image
/usr/share/ohc/zigbee/em357-ncp.ebl.version   6.7.8.0
```

To bump the image, change all four values in `em357-ncp.source` together.

## The image

`em357-v678-ncp-uart-sw.ebl` is Silicon Labs' build, mirrored in
[walthowd/husbzb-firmware](https://github.com/walthowd/husbzb-firmware) (the
HUSBZB-1 stick is the same EM357):

- UART on SC1 (PB1/PB2), the only serial controller on the part that can do a
  UART, so one image fits every board.
- **57600 baud, software (XON/XOFF) flow control.** Control4's 4.7.2 image runs
  at 115200 with RTS/CTS.

## How the reflash works

The flash lives in `packages/zigbee/src/` (`ash.rs`, `bootloader.rs`,
`ncp.rs`), and `ohc-zigbee` runs it before it serves or holds the port:

1. Find the app: try the bootloader menu first (`\r\n`, then `2. run`), then an
   ASH reset and EZSP `version` at each app baud (`OHC_ZIGBEE_BAUD`, 115200,
   57600).
2. If the stack is older than `em357-ncp.ebl.version` (and
   `OHC_ZIGBEE_NCP_UPDATE` isn't 0), send EZSP `launchStandaloneBootloader`
   (0x8F) and upload the `.ebl` over XMODEM-CRC through the bootloader's `1.
   upload ebl`, then `2. run`.
3. Re-read the version. The result goes to `/run/ohc-zigbee.ncp`.

A flash that dies part-way leaves no valid app, so the chip stays in its
bootloader. The next boot finds it there and finishes the job. The bootloader
itself sits in protected flash and an upload never touches it.

By hand:

```sh
ohc-zigbee --info          # what the radio runs now
ohc-zigbee --flash <ebl>   # force a flash
```

Stop `S91ohczigbee` first. A second process on the tty steals the replies.

## Protocol notes (measured on an HC-800)

- **Bootloader**: `EM357 Serial Bootloader v43 b27` at 115200 8N1, with the menu
  `1. upload ebl / 2. run / 3. ebl info / BL >`. The XMODEM-CRC transfer uses
  128-byte blocks padded with 0x1A and CRC-16/XMODEM (init 0).
- **ASH**: CRC-CCITT (init 0xFFFF). Data is randomized with seed 0x42 and
  polynomial 0xB8. The bytes 7E/7D/11/13/18/1A are stuffed. RST is
  `1A C0 38 BC 7E`; RSTACK's control byte is 0xC1.
- **EZSP**: the first `version` goes out in the legacy header `[seq, fc, id]`.
  Once the NCP says v8, the renegotiating `version` **must** use the extended
  header `[seq, fcLo, 0x01, idLo, idHi]`. The NCP misparses a legacy one and the
  radio looks dead.
- **The HC-800 radio comes out of reset in its bootloader**, so it needs `2`
  before the app answers. Control4's `zap` also parks it there when it stops.

## Control4's side, for reference

- `/control4/bin/zap` reflashes from its bundled
  `em357-uart-rts-cts-use-with-serial-uart-bootloader_4720.ebl`.
- `/etc/zap.conf` sets `allow_ncp_update=1` and `allow_ncp_downgrade=1`, so
  **running `zap` on a stock boot puts the radio back on 4.7.2**. On a stock
  HC-800 it's disabled in `/etc/sysman.d` (`enabled="0"`). After a return to
  stock, openHC simply flashes it again on its next boot.
- `/control4/share/reset-frontpanel` sets 115200 with CRTSCTS, then pulses
  `zigbee_reset` (HC-800: gpio206, ICH GPIO offset 0) low for 1 s.
