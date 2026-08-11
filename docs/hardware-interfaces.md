# EA1 hardware interfaces (measured on a live unit)

What each subsystem is, how to reach it, and whether we own it yet.

| Subsystem | Interface | Open? |
|---|---|---|
| Display / HDMI | Intel GDL planes (`tvmode`, UPP_A..UPP_D), PowerVR SGX | closed blob (no mainline driver) |
| Fan / thermal | LM75 over i2c + sysfs PWM | open (standard drivers) |
| HW watchdog | PIC24 on `/dev/ttyS2`, DLE/STX framing | reverse-engineered — see below |
| Zigbee | EM357 NCP behind a CP2104, EZSP | open (standard USB-serial) |
| IO MCU | TM4C1231D5 on `/dev/ttyS1` @ **460800** | ours (clean-room; see io-mcu-firmware) |
| Wi-Fi | Atheros AR9485 / ath9k, `wlan0` | open (mainline) |
| Ethernet | Intel CE e1000 variant + BCM53125 switch PHY | vendor patches (not mainline) |
| eMMC | SEM08G, `/dev/mmcblk0` | vendor MMC-v51 patch (not mainline) |

## Fan / thermal — fully open

Both ends are standard interfaces; no vendor code:

```
temp in : /sys/class/hwmon/hwmon0/device/temp1_input   LM75 on I2C, milli-C
          temp1_max = 80000  (the vendor's own critical limit)
fan out : /sys/class/pwm/pwm0:2/{period_ns,duty_ns,run}
          period_ns = 40000 (25 kHz); duty_ns = period * pct/100
          `run` is WRITE-ONLY — reading it returns EACCES, that is normal
          the channel must be CLAIMED first: `echo 1 > request` (reads back
          "sysfs <pid>"). Until claimed, writes to period_ns are silently
          dropped, period stays 0, and the fan cannot spin at any duty.
          The claim is PER-PROCESS: it dies with the owner and period resets to
          0. ohc-fand re-claims each loop, so it self-heals within ~15s; but a
          transient `ohc-fand once` will briefly steal it from the daemon.
```

`ohc-fand` reproduces the stock `[ea_fan]` curve from `/etc/c4faultd.conf`:
`on_temp 50, min_on_pcnt 10, pcnt_per_degree 10, run_time_after_cool_min 60`,
plus a 80 °C critical override and **fail-safe to 100 % if the sensor read fails**.

```sh
ohc-fand once        # one pass, prints temp/duty
ohc-fand set 60      # force 60 % (testing)
ohc-fand off
```

Idle reference: heatsink ~42 °C, cpu0 ~51 °C, fan 0 % (correct — below 50 °C).

Note the CPU temps come from `/dev/thermal` (Intel CE char device, ioctl-based) and
are *not* what the fan curve uses. Mainline `coretemp` and `x86_pkg_temp_thermal`
both fail to bind on the CE5310, so the LM75 heatsink sensor is the open path.

## The PIC24 watchdog

`/dev/ttyS2` goes to a **Microchip PIC24** — a discrete microcontroller on the
board, not part of the SoC. Intel's own diagnostic tool settles the naming:

```
$ strings /usr/dtsbin/pic24
The uart device is wrong, use '-dev /dev/ttyS1' parameters for CE3100 and CE4100
The uart device is wrong, use '-dev /dev/ttyS2' parameters for CE4200 and CE5300 and CE2600
InitPIC24()
PIC24 Version:%s
```

This matters because two different parts get called the same thing. CEFDK's boot
banner reports `8051 Firmware : C0-1.0.53` — that is a power-management core
*inside* the CE53xx, and it is **not** what `ttyS2` talks to. Control4's
`/etc/rc.d/99control4` comments the port as "8051 Power Management Inside
CE53xx", which is wrong for this board; their own `watchdogd` prints
`PIC24 Version`. The kernel agrees something is different about the port: it
enumerates as a plain `8250` at `0x3e8` while `ttyS0/1/3` are all `GEN3_serial`,
the SoC's own UART block.

The PIC24 owns more than the watchdog. `libpicuart.so` — Intel's, banner
`36.0.14495.347773` — exposes `setGPIOValue`, `setPWM`, `setIrRepeatMode`, a
`PicBufferIR`, and a full HDMI-CEC message class. Framing is ASCII-hex with an
XOR checksum:

```
AA <n> <body as hex> <xor as hex>     n = hex chars in body
ack  AA 02 "0606"     nak  AA 02 "0707"     CEC ack  AA 04 "050005"
```

Traffic is one `cmd(26)` heartbeat every ten seconds, and nothing else — see
[HDMI CEC](hdmi-cec.md) for what that rules out.

Replacing `watchdogd` means owning that link: heartbeat on time, every time,
multiplexed with whatever else we want the PIC to do. Miss it and the board
resets within a minute. Until then we keep the vendor daemon; it is the last
Control4 process running.

## Zigbee — WORKING (EM357 running EmberZNet, reachable over TCP)

`tcp://<host>:6638` — attach zigbee2mqtt (`port: tcp://...`) or ZHA
(`socket://...`). Served by `ohc-serialbridge`, managed by `/etc/init.d/ohc`.

**The radio boots into its bootloader and must be told to run the app.** Sequence
that works (all of it matters):

1. Open the port at **115200 8N1, NO RTS/CTS**. The vendor `stty 1cb2` decodes to
   `B115200|CS8|CREAD|CLOCAL|HUPCL` — no `CRTSCTS`. With RTS/CTS on, the kernel
   blocks every write waiting for CTS and the radio looks dead.
2. Pulse reset: `echo 0 > /dev/gpio/zigbee_reset; sleep 1; echo 1 > ...`
3. Send `\r\n` a few times → the bootloader menu appears:
   `EM357 Serial Bootloader v45 bA8 / 1. upload ebl / 2. run / 3. ebl info / BL >`
4. Send **`2`** → the app starts and immediately emits an ASH RSTACK.

Verified:

```
'2'        -> ff 1a c1 02 09 2a 10 7e     app booted, RSTACK
ASH reset  -> 1a c1 02 0b 0a 52 7e        0xC1 = RSTACK, ash v2, reset code 0x0b
```

Firmware on disk is `em357-uart-rts-cts-use-with-serial-uart-bootloader_4720.ebl`
(105,344 B) — Silicon Labs' standard NCP image naming, EBL header carries
`ZNCPVer:4720`. It looks like a **stock Silabs NCP build with a Control4 version
tag**, not custom silicon firmware. `zap` uploaded it via ZMODEM (`/sbin/lsz`)
using bootloader option `1` whenever versions mismatched — so option 1 + that file
is the reflash path if the app is ever lost.

TODO: make `ohc` run step 1-4 automatically at boot so the radio comes up in
application mode without manual intervention.

### Redistribution: we never need to ship the `.ebl`

Worth being explicit, because it removes a licensing problem entirely: **the NCP
firmware is already flashed on every unit's radio.** Our software only tells the
existing image to *run* (bootloader option `2`). We do not copy, bundle, or
redistribute `em357-...-4720.ebl` — it stays on the user's own device, exactly like
the graphics blobs.

The only case that would need an image is re-flashing a radio whose app is lost or
corrupt. Options there, in order of preference:
1. use the `.ebl` already present on that same device (`/control4/firmware/pro/`) —
   the user's own file, no redistribution;
2. a stock Silicon Labs EM357 NCP image from the EmberZNet SDK, under SiLabs' terms;
3. never Control4's copy bundled into our releases.

Whether the on-device `.ebl` is byte-identical to a stock SiLabs build is still
unconfirmed — the filename follows SiLabs' convention and the header says
`ZNCPVer:4720`, but we have no reference copy to diff against. Since path 1 needs no
redistribution, that question does not block anything.

### How it was found (the bootloader trap)

`ohc-serialbridge` puts the UART on the network and works, but the NCP answers
nothing — not EZSP/ASH, not at any baud, with or without RTS/CTS, not even after a
hardware reset with the port already open.

Running the vendor daemon (`zap`) explained it:

```
ZbRadio::hardwareResetPanelToBootloader: Stopping EZSP interface ...
ERROR: Closing EZSP interface: status: 0xdd EZSP_UKNOWN_STATUS ashError: 0xff ncpError: 0xff
NcpInterface::executeResetScript: Hardware reset of the front-panel
ZapServer::shutdownNcp: NCP should be in bootloader
```

**The radio is in the EM357 serial bootloader, not running EmberZNet**, so there is
no EZSP to talk to. `zap` puts it there deliberately (and would normally flash/boot
it back). Mode selection lives in **`/control4/share/reset-frontpanel`**; the
`ninjago` branch is:

```sh
stty -F /dev/ttySZigbee 5:0:1cb2:0:0:...    # 115200 8N1, CREAD|CLOCAL|HUPCL
echo 0 > /dev/gpio/zigbee_reset ; sleep 1
echo 1 > /dev/gpio/zigbee_reset ; sleep 1
echo "" > /dev/ttySZigbee ; sleep 1         # two newlines = bootloader wake
echo "" > /dev/ttySZigbee
```

(`1cb2` decodes to B115200 | CS8 | CREAD | CLOCAL | HUPCL — so 115200 was right.)

Next step: after that sequence the EM357 serial bootloader should present its menu
(`EM3xx Bootloader ... 1. upload ebl  2. run  3. ebl info  BL >`). Sending **`2`
(run)** boots the application, after which EZSP/ASH — and therefore zigbee2mqtt or
ZHA over `tcp://<host>:6638` — should work. If the app image is missing or corrupt,
`1` + the `.ebl` at `/control4/firmware/pro/em357-uart-rts-cts-...-4720.ebl` is the
recovery path (that is exactly what `zap` used `/sbin/lsz` for).

Until the radio is out of the bootloader, "stock vs custom firmware" stays
unanswered — the bootloader banner itself will give the first real version string.

## Zigbee — hardware reference

```
/dev/ttySZigbee -> /dev/ttyUSB0   (Silicon Labs CP2104 USB-UART, 10c4:ea60)
reset: /dev/gpio/zigbee_reset (gpio29)
```

From the stock `/etc/zap.conf`: EmberZNet NCP, firmware
`em357-uart-rts-cts-use-with-serial-uart-bootloader_4720.ebl` (**RTS/CTS** variant),
`security_level 5`, `tx_power -4`, `max_end_device_children 64`. Control4's `zap`
listened on port 7910 and flashed the NCP with `/sbin/lsz` (ZMODEM) when versions
mismatched.

The port is **free** now that `zap` is gone, so an external Zigbee stack
(zigbee2mqtt / ZHA) can drive it over a TCP serial bridge. That bridge is the next
piece of work — there is no `socat`/`ser2net`/python on the device, so it has to be
our own binary.

## IO MCU — working

TI **TM4C1231D5** on `/dev/ttySIO` → `/dev/ttyS1`, reset on `/dev/gpio/io_reset`
(gpio7, `active_low=0`). `1` is the running state.

**The link is 460800 baud, and the MCU boots into its bootloader — it has to be
told to run the application.** `overlay/services/ohc-io` does both; run it once
and the MCU answers:

```
$ ohc-io --force
ohc-io: booting the IO MCU out of its bootloader (460800 baud)
ohc-io: application running

$ ohc-iod read-once --device /dev/ttySIO --baud 460800 --no-serial-devices
product=c4:io_processor:c4-ir02
firmware=1.0.36
contact_mask=0x00000000
```

Every earlier probe failed purely because it used **115200**, which the app image
appears to endorse (the constant occurs 19 times) but which actually belongs to
the MCU's own user serial ports. The bootloader autobauds, so a wrong host speed
produces silence rather than garbage — indistinguishable from dead hardware.

Full bring-up handshake, opcode map, framing and IR findings:
**[docs/io-mcu-firmware.md](io-mcu-firmware.md)**.

## Wi-Fi — working, ours

Atheros **AR9485** at PCI `0000:03:00.0` (behind PCIe bridge `00:1c.1`), driven by
the in-tree open **ath9k** — no blob, no firmware file.

The stock image already loads ath9k and binds it at boot; `wlan0` simply sat DOWN
and unconfigured, because Control4 never used Wi-Fi on this model. There is no
vendor Wi-Fi script to replace — `overlay/services/ohc-wifi` is the whole of it.

```sh
ohc wifi --join "MySSID" "MyPassphrase"   # remember a network and connect
ohc wifi --status
ohc wifi --restart
ohc wifi --forget
```

The passphrase is stored **hashed** (via `wpa_passphrase`) in
`/opt/ohc/wpa_supplicant.conf`, mode 0600. `OHC_WIFI=1` in `board.env` brings it
up at boot; `install.sh` re-enables that flag whenever a stored config exists, so
a re-deploy cannot silently drop the box off Wi-Fi. Ethernet and Wi-Fi run
simultaneously; DHCP on `wlan0` installs a default route, so Wi-Fi becomes the
default path when both are up.

Three things worth knowing:

* **Wi-Fi bring-up does NOT trip the PMU watchdog.** An earlier note in this repo
  claimed every `wlan0` bring-up hard-rebooted the box. Each step was re-run
  post-clean-slate with an uptime guard around it — link up, scan, associate,
  DHCP — with zero reboots. The original symptom was most likely the vendor
  net-watchdog reacting to the route change, and clean-slate removes it.
* **`/dev/gpio/wlan_disable` is irrelevant.** It reads `1` at boot and the radio
  works anyway; it does not gate the radio.
* **There is no `udhcpc`.** The DHCP client is ISC `dhclient`, and its `-timeout`
  is a config-file option, not a CLI flag (`dhclient -4 -1 wlan0`).
