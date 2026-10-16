# EA-family kernel patches (Buildroot `BR2_GLOBAL_PATCH_DIR`)

Applied to vanilla Linux 7.1.8 for every `ea*` board.

There are three kinds of EA kernel source, and each has one home:

| Where | What belongs there |
|---|---|
| `board/ea/common/kernel/arch/x86/boot/dts/` + `board/ea/<board>/<board>.dts` | **board description** — which device sits where (shared `.dtsi` + the board's `.dts`) |
| `board/ea/common/kernel/drivers/`, `sound/` | new drivers for the SoC's own PCI functions (a copy) |
| `board/ea/common/patches/linux/` | edits to existing upstream files (a diff) |

The first two are copied into the kernel tree by `OHC_KERNEL_MIRROR_HOOK`
(`board/external.mk`) and registered through `kernel/objs.mk`. Copies beat
patches whenever a file is new: the source stays a normal file you can edit,
grep and compile-check, with no diff context to go stale on a kernel bump.

## The short version

| # | Patch | Why it must stay a patch |
|---|---|---|
| 0001 | i2c-pxa CE5300 | 4th controller (the codec's bus); BAR-indexed DT nodes; CE4100 compatible; optional clock |
| 0002 | x86 CE5300 serial clock | legacy-table BASE_BAUD at 14.7456 MHz, only so the boot console is readable before 8250_of takes the port over |
| 0003 | e1000 fake PHY | changes upstream probe behaviour |
| 0007 | ASoC CE5300 | **not applied** (`.disabled`) — WIP, see below |
| 0015 | x86 built-in DTB | x86 has no way to carry its own device tree; adds `BUILTIN_DTB` |
| 0016 | ehci-pci VBUS supply | rear-USB power (gpio44) follows root-hub port power, so the port is switchable from Linux |

Retired by the device tree: **0006** (force the GEN3 UARTs to XScale — now
`compatible = "intel,xscale-uart"`), **0011** (keep 8250_pci off the 0x2e66 UART
— now `status = "disabled"` on that PCI node), and the IRQ half of **0002** (the
tree gives the UARTs no interrupt, so they run polled).

New-file drivers in `board/ea/common/kernel/drivers/`:

| Driver | Lands at | Registered as |
|---|---|---|
| `gpio/gpio-intelce.c` | `drivers/gpio/` | `obj-y` |
| `pwm/pwm-ce5300.c` | `drivers/pwm/` | `obj-y` |
| `spi/spi-ea-ce5xx.c` | `drivers/spi/` | `obj-y` |

---

## The board device tree

CEFDK's `bootlinux` hands the kernel no DTB, so for a long time everything ACPI
does not describe was board description written in C: `leds-ea-board.c`,
`gpio-ea-board.c`, `spi-ea-b53-board.c`, `ce5300-ea-board.c` and `ce5300-fb.c`.
They are gone. Each board's tree is now linked into the kernel:

* **0015** adds `CONFIG_BUILTIN_DTB` / `CONFIG_BUILTIN_DTB_NAME` to x86 (the
  pair loongarch has, on top of `GENERIC_BUILTIN_DTB`), and
  `x86_flattree_get_config()` uses it when no `SETUP_DTB` came from the boot
  loader. `common.fragment` turns it on; each board's fragment names its tree.
* **ACPI keeps the platform.** x86 only takes SMP/IRQ config from a tree when
  `acpi_disabled`, so CPUs, interrupts and PCI routing stay ACPI's. The tree
  only hangs board devices off PCI functions. A wrong tree loses those devices;
  it does not lose the boot.
* **Matching.** `pcibios_get_phb_of_node()` finds the root bus node by
  `device_type = "pci"` + `bus-range`; `pci_set_of_node()` finds each device by
  the devfn in `reg`. The layout follows `arch/x86/platform/ce4100/falconfalls.dts`.
  A described PCI device gets an OF fwnode *instead of* its ACPI companion
  (`pci_set_acpi_fwnode()` skips devices that already have one). None of the
  drivers involved use ACPI.
* **Iterating without a rebuild.** A DTB from the boot loader wins over the
  built-in one, and kexec-tools passes one on x86:
  `kexec -l bzImage --dtb=test.dtb --command-line="$(cat /proc/cmdline)"`.
  `/sys/firmware/fdt` holds the tree the running kernel used.

| File | Describes |
|---|---|
| `ce5300.dtsi` | the PCI functions used (audio, GPIO, 4×I2C, SSP, GbE) + `i2cN` aliases |
| `ea.dtsi` | every EA: LEDs, common reset/enable hogs (IO-MCU, Zigbee, USB-serial, LDO, FPGA), rear-USB VBUS (gpio44 regulator-fixed, PR #51) as the EHCI vbus-supply, LM75, CEFDK framebuffer |
| `ea3.dtsi` | EA3 GPIO block (`setup_gpio_ea3()`) |
| `ea-wifi.dtsi` | the `wifi` feature: `wlan_disable` |
| `ea-switch.dtsi` | the `switch` feature: BCM53125 on SSP CS1 + `gb_sw_reset`/`gb_eth_reset` |
| `ea-audio-dsp.dtsi` | the `audio-dsp` feature: ADAU1451 on i2c-3, the card node, `codec_reset`/`dsp_reset` |
| `ea-gpio-names.dtsi` | `gpio-line-names`, included last; feature lines are named only when the feature is included |
| `board/ea/<board>/<board>.dts` | the board: includes matching its `ohc.features` |

A GPIO line is hogged by the file for the hardware it serves, so a board without
that hardware never drives it; an unhogged line keeps whatever CEFDK left it at.

What the move changed, beyond deleting C:

* **GPIO lines have names now** (`gpio-line-names`), so `gpiofind dsp_reset`
  works. The old file only set consumer labels, which no by-name tool sees.
  Inputs (board-id straps, the ID button) are named but not claimed.
* **Each line is driven only where its hardware is.** `gpio-ea-board.c` drove
  every line on every board, including the EA3 block (75, 121) on EA1s. Now the
  DSP/codec resets come with `audio-dsp`, the switch/GbE resets with `switch`,
  `wlan_disable` with `wifi`.
* **The switch gate moved.** `ohc.switch=1` was read by the C file that
  registered the switch. The device now always exists, and `S39ea-switch`
  decides whether `b53_spi` loads — same flag, same default (off).
* **The SoC UARTs are device-tree ports.** 8250_of probes 0x3f8/0x2f8/0x2e8
  as XScale at 14.7456 MHz with no IRQ (polled), each taking over the legacy
  slot at its address so ttyS numbers stay put; the 0x2e66 PCI face of UART1 is
  disabled so it cannot steal the IO-MCU's 0x2f8 decode. 0010 (TX from the LSR)
  and 0012 (GEN3 FCR/FIFO) are still driver changes.
* **The switch is described the supported way.** DSA's platform_data path is
  legacy and silently needs `dsa_chip_data.netdev[cpu_port]`; the OF path
  resolves the conduit from `ethernet = <&gbe>`. The CPU port carries no
  `phy-mode`/`fixed-link` on purpose: b53 is on DSA's workaround list, so DSA
  skips phylink for it and leaves firmware's forced 1G link alone, exactly as
  before.

---

## Per-patch detail

### 0001 — `i2c-pxa`: four controllers, DT nodes, optional clock

**Necessary as a patch: yes.** It edits two upstream files.

* **`CE4100_PCI_I2C_DEVS` 3 → 4.** The CE5300 has FOUR I2C controllers, not the
  CE4100's three. The fourth BAR sits at `0xdffe0e00`, discontiguous from the
  other three (`0xdffe0500/0600/0700`), which is why it went unnoticed for so
  long. Measured on a live EA3: BAR3's registers at `+0x14`/`+0x18` read
  `0x03`/`0x12`, byte-identical to the three working controllers. That fourth bus
  is where the **ADAU1451 audio codec (0x38)** lives — Control4's own ASoC device
  name `"adau1451.3-0038"` says bus 3. **Audio cannot work without this line.**
* **Child nodes are matched by BAR index** (`reg = <bar>`), not by translated BAR
  address as upstream did: with `pci=realloc` a BAR can move, its index cannot.
  A BAR with no node still registers, bare, so a device-tree mistake never takes
  the box's i2c — and with it the LM75 the fan is driven from — away.
  (`ohc-ea-hw` still instantiates the LM75 by hand if `0-0048` is missing.)
* **`"intel,ce4100-i2c"` compatible in i2c-pxa.** Upstream's CE4100 DT path has
  rotted: a DT-probed adapter matched no compatible and fell back to
  `REGS_PXA2XX`, the wrong register offsets.
* **`devm_clk_get_optional()`.** With `CONFIG_COMMON_CLK=y` (needed for
  `spi-pxa2xx-pci`) a required clock is a real lookup, and this SoC has no clock
  provider, so every adapter failed with
  `pxa2xx-i2c ce4100-i2c.0: error -ENOENT: failed to get the clk`. The rate is
  only read under `high_mode`, so the clock is optional; this replaces the
  placeholder fixed-rate clock the old patch registered per adapter.

### 0015 — x86: built-in device tree

**Necessary as a patch: yes** — it adds Kconfig symbols and an early-boot hook
to `arch/x86`. See "The board device tree" above.


### 0016 — ehci-pci: board VBUS supply follows root-hub port power

**Necessary as a patch: yes** — it adds a `.port_power` hook to ehci-pci.
The rear USB port's 5 V is a load switch on gpio44 that the controller knows
nothing about (stock drove it high from a closed board driver; PR #51 found it).
The board DTB makes it a `regulator-fixed` and the controller's `vbus-supply`;
the hook switches it from root-hub port power, the way chipidea drives its VBUS
regulator, so the port's `disable` attribute and uhubctl really cut VBUS. The
root hub has to advertise per-port power switching for usbcore to send
PORT_POWER at all, so the patch sets `HCS_PPC` in the cached `hcs_params` when a
supply is present (single-port controllers only). The regulator core is only in
the fat kernel (`linux/real-kernel.fragment`); the bootstrap kernel has none and
leaves the rear port unpowered.

### 0003 — e1000 CE5300 fake PHY

**Necessary as a patch: yes, unavoidably.** Modifies three upstream files in
`drivers/net/ethernet/intel/e1000/`. Mainline claims `8086:2e6e` as
`e1000_ce4100` and then fails PHY detection with `-EIO`, because the EA1/EA3
have **no PHY on the GbE MDIO bus** — the MAC is wired to the switch's port 5 on
a fixed forced link.

This is a behavioural change inside upstream probe logic; no config, DT, or
out-of-tree file can express it. Proven on hardware:
`CE5300: no GbE PHY, using fake-PHY mode` → link up at 1 Gbps.

**Getting the switch working does NOT retire this patch — it makes it load
bearing.** The hook is in `e1000_probe()`, not in link management: without it
the CE4100 PHY-address autodetect scans all 32 MDIO addresses, finds nothing,
and `e1000_probe()` returns a bare `-EIO`, so there is **no `eth0` netdev at
all**. DSA then has nothing to attach to — `eth0` is the conduit, and
`dsa_port_parse()` resolves the CPU port by finding a net-class device under the
e1000. The switch cannot stand in for the PHY because it hangs off **SPI**, not
the GbE MDIO bus; it never appears to e1000 as a PHY however DSA is configured.
That is also why the patch reports `BCM53125_PHY_ID` as a canned constant rather
than reading it off the wire.

A DTB does not help here either: e1000 is a pre-phylink driver with its own PHY
code in `e1000_hw.c` and never consults DT, so a `fixed-link` node is ignored.

### 0007 — ASoC CE5300 AUD_IO (**disabled**)

Not applied: the filename ends in `.disabled`. Skeleton for the one piece of the
EA audio path missing from Control4's GPL drop (the PCM/DMA platform component).

Deliberately probe-only: it maps BAR0 and logs, but does **not** register an
ASoC component, because a PCM device whose trigger cannot move samples is worse
than no sound card at all. Register map in `https://the1truejoe.github.io/openHC/ea/audio-regmap/`.

*Could be an out-of-tree module* once it does something.

---

### 0014 — EHCI CE5300/CE2600 host-controller bring-up

The EA's three USB controllers (PCI 192e:0101, a TransDimension/TDI core, rev 07)
all die at probe with `can't setup: -110` — ehci_setup()'s reset handshake times
out — so there is no USB at all. The silicon is fine (registers read back sanely;
a hand-poked HCRESET completes). The core is an OTG part with Moorestown-style
per-port HOSTPC registers, and until `ehci->has_hostpc` is set the driver uses the
wrong register path and the controller never enters host mode. Patch folds Intel's
GPL `CONFIG_GEN3_USB` quirk (has_hostpc for device 0x0101 rev >= 6, plus
`has_tdi_phy_lpm = 0` for its LPM-less PHY) into the existing `PCI_VENDOR_ID_TDI`
case. has_hostpc also fixes PORTSC line-status decoding, which is what lets a
full-speed device — the EM357 Zigbee module's CP2104 on 01:0d.2 — enumerate. Needs
`CONFIG_USB_EHCI_ROOT_HUB_TT` (common.fragment) because has_tt=1 and there is no
companion controller. **Upstreamable** on its own; kept as a patch because it edits
`drivers/usb/host/ehci-pci.c`.

---

## Per-driver notes — `board/ea/common/kernel/drivers/`

These are plain `.c` files copied into the kernel tree by
`OHC_KERNEL_MIRROR_HOOK` and registered through `kernel/objs.mk`. Each binds one
of the SoC's PCI functions by ID; none of them describes the board.

### `gpio/gpio-intelce.c`

**Necessary as a patch: mostly no**, with one caveat. New standalone driver:
128 lines across 4 banks × 32, reconstructed from Control4's GPL drop.

The caveat is that mainline's `gpio-sodaville` binds the **same** PCI ID
(`8086:2e67`) with the wrong CE4100 layout (12 lines instead of 128), so it must
be disabled — `# CONFIG_GPIO_SODAVILLE is not set` lives in
`board/ea/common/linux/common.fragment`. An out-of-tree module would still work,
but only if that config stays set, so the two are coupled.

Confirmed: `intelce-gpio 0000:01:0b.1: CE5300 GPIO: 128 lines on BAR0`.

Note the 7.1.8 API detail: `gpio_chip.set` returns `int`, not `void`.

### `pwm/pwm-ce5300.c`

**Necessary as a patch: no.** Entirely new driver binding PCI `8086:089f` for
the fan. Touches only `drivers/pwm/{Kconfig,Makefile}` plus its own new file.
No upstream driver exists.

*Could be an out-of-tree module.* It is a patch mainly so `CONFIG_PWM_CE5300=y`
works like any other kernel symbol.

### Switch bring-up traps (now `ea-switch.dtsi`)

The switch burned the most time of anything here, so the traps stay recorded:

1. **`CONFIG_SPI_PXA2XX_PCI` cannot be set directly.** It is
   `def_tristate SPI_PXA2XX && PCI && COMMON_CLK` — a *derived* symbol, so a
   fragment assignment is silently dropped. `COMMON_CLK` is the term that was
   missing; without it the PCI glue is never compiled and `01:0b.4` sits
   unbound with an empty `/sys/class/spi_master`.
2. **SPI mode 3, not 0.** Mode 0 transfers complete and the controller's IRQ
   counts up, but the switch never answers and b53 probes `-EINVAL` — which
   looks exactly like a wrong chip select. The vendor's `spiMasterConfig()` sets
   SPH=1 + SPO=1.
3. **The b53 config symbols are `CONFIG_B53` and `CONFIG_B53_SPI_DRIVER`** —
   *not* `CONFIG_NET_DSA_B53*`, which do not exist and which kconfig discards
   without a word.
4. (Platform-data era, kept for history.) The SPI bus number is the SSP's devfn
   (`spi92`), and DSA's platform_data needed `netdev[cpu_port]` set by hand.
   Neither matters with a device tree: the switch is a child of the SSP node and
   the conduit is `ethernet = <&gbe>`.

Port map (measured, see `ea-switch.dtsi`): CPU = port 5, jacks = ports 1 and
2, and port 1 is isolated in the stock configuration.

---

## Rules of thumb

* **Changing an upstream file → patch.** You cannot express an edit as a copy.
* **Saying which device sits where → the device tree**, never a C board file.
* **Adding a new driver → `board/ea/common/kernel/drivers/` + a line in `objs.mk`.**
  No diff context to rot, and the source stays greppable and editable.
* **Do not invent a CONFIG symbol.** A copied-in driver has no Kconfig hunk, so
  `obj-$(CONFIG_MY_DRIVER)` expands to nothing and the driver silently is not
  built. Use plain `obj-y`, or gate it behind a real upstream symbol.
* **Check how a symbol is DEFINED before putting it in a fragment.** Assignments
  to `def_tristate`/`select`-only symbols, and to symbols that do not exist at
  all, are discarded in silence. Four separate bugs in the switch bring-up were
  this one mistake wearing different hats:
  `CONFIG_SPI_PXA2XX_PCI` (derived), `CONFIG_NET_DSA_B53*` (never existed),
  `CONFIG_PWM_CE5300` (existed only in a patch hunk that was later deleted),
  and `COMMON_CLK` missing (which silently disabled the SPI glue *and* broke i2c).
