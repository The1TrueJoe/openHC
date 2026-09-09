# EA-family kernel patches (Buildroot `BR2_GLOBAL_PATCH_DIR`)

Applied to vanilla Linux 7.1.8 for every `ea*` board.

**Only two patches remain, and both exist for the same reason: they CHANGE an
existing upstream file.** Everything that was merely a *new* driver now lives as
a plain `.c` in `board/ea-common/kernel/drivers/`, copied into the kernel tree and
registered by `OHC_KERNEL_MIRROR_HOOK` (see `drivers/objs.mk`). That is
strictly better — the source stays a normal file you can edit, grep and
compile-check, with no diff context to go stale on a kernel bump.

| Where | What belongs there |
|---|---|
| `board/ea-common/kernel/drivers/` | new drivers and board glue (a copy) |
| `board/ea-common/patches/linux/` | edits to existing upstream files (a diff) |

## The short version

| # | Patch | Why it must stay a patch |
|---|---|---|
| 0002 | x86 CE5300 serial | UART clock is 14.7456 MHz, not 1.8432 -- and ISA IRQ 4/3 are not routed (use 38) |
| 0001 | i2c-pxa-pci without DT | deletes upstream DT-matching code; adds the clock; enumerates the 4th controller (the audio codec's bus) |
| 0003 | e1000 fake PHY | changes upstream probe behaviour |
| 0007 | ASoC CE5300 | **not applied** (`.disabled`) — WIP, see below |

Moved out of patches and into `board/ea-common/kernel/drivers/`:

| Driver | Lands at | Registered as |
|---|---|---|
| `gpio/gpio-intelce.c` | `drivers/gpio/` | `obj-y` |
| `pwm/pwm-ce5300.c` | `drivers/pwm/` | `obj-y` |
| `leds/leds-ea-board.c` | `drivers/leds/` | `obj-y` |
| `spi/spi-ea-b53-board.c` | `drivers/spi/` | `obj-y` |
| `video/fbdev/ce5300-fb.c` | `drivers/video/fbdev/` | `obj-$(CONFIG_FB_SIMPLE)` |

---

## The device-tree question

Most of what is here is board description written in C because CEFDK's
`bootlinux` hands the kernel no DTB. That is a real constraint, but it is *not*
the same as "x86 cannot do device tree". Checked against 7.1.8:

* `X86_INTEL_CE` still exists and does `select OF` + `select OF_EARLY_FLATTREE`.
* `arch/x86/platform/ce4100/{ce4100.c,falconfalls.dts}` are still in-tree — a
  working device tree for this exact SoC family.
* `arch/x86/kernel/devicetree.c` still has `x86_flattree_get_config()` and
  `of_platform_bus_probe()` with `intel,ce4100-cp`.

The obstacle is only *delivery*: `initial_dtb` is set by `add_dtb()`, which is
called from the `SETUP_DTB` boot-protocol path — the bootloader chains it
through `setup_data`, and CEFDK will not. But we build the bzImage, so embedding
a blob and pointing `initial_dtb` at it is a small patch.

What a DTB would replace:

* **`spi-ea-b53-board.c` and `leds-ea-board.c` entirely** — the switch port
  map and the LED map become DT nodes.
* The `fixed-clock` problem in **0001** (below) — a `fixed-clock` node
  instead of `clk_register_fixed_rate()` in C.
* DSA's `of_node` path is far better travelled than `platform_data`. The
  platform_data path is legacy, and we hit one of its traps: it silently
  requires `dsa_chip_data.netdev[cpu_port]`, and a NULL there makes the CPU port
  permanently unresolvable.

The catch, and it is real: our peripherals are **PCI-enumerated**, so DT nodes
must be *matched* to PCI devices through the host-bridge/bus hierarchy the way
`falconfalls.dts` does. Ours sit behind a PCIe bridge at `0000:01:xx`, which is
fiddlier than falconfalls' flat layout, and a wrong DTB is a non-booting kernel
— which on a secure-boot EA3 costs an ID-button recovery cycle.

**Verdict: DT is the better architecture and is viable. It is a consolidation
to do deliberately, against a known-good build, not while also debugging a
driver.**

---

## Per-patch detail

### 0001 — `i2c-pxa-pci`: enumerate without a device tree

**Necessary as a patch: yes.** It deletes DT-matching code from an existing
upstream file (`drivers/i2c/busses/i2c-pxa-pci.c`). Upstream requires
`dev->dev.of_node` and bails with "Missing device tree node"; we have none.

Also registers a **fixed-rate clock per adapter**. That is not cosmetic:
`i2c-pxa` calls `devm_clk_get()` unconditionally. With `CONFIG_COMMON_CLK=y` —
which the `switch` feature requires for `spi-pxa2xx-pci` — that becomes a real
lookup, and this SoC registers no clock provider, so every adapter fails:

```
pxa2xx-i2c ce4100-i2c.0: error -ENOENT: failed to get the clk
```

and the box comes up with **no i2c at all** (which would also silently block the
ADAU1451 audio codec on i2c-3). The rate is a marked placeholder: `i2c-pxa` only
reads it under `high_mode`, set from the DT property `mrvl,i2c-high-mode`.

**Third job: `CE4100_PCI_I2C_DEVS` 3 → 4.** The CE5300 has FOUR I2C controllers,
not the CE4100's three. The fourth BAR sits at `0xdffe0e00`, discontiguous from
the other three (`0xdffe0500/0600/0700`), which is why it went unnoticed for so
long. Measured on a live EA3: BAR3's registers at `+0x14`/`+0x18` read
`0x03`/`0x12`, byte-identical to the three working controllers, and zero
elsewhere because nothing had configured it.

That fourth bus is where the **ADAU1451 audio codec (0x38)** lives — Control4's
own ASoC device name `"adau1451.3-0038"` says bus 3, address 0x38. With DEVS=3
the codec is not merely misconfigured, it is unreachable: `i2cdetect` finds
nothing at 0x38 on any bus and i2c-1/i2c-2 come up entirely empty. **Audio cannot
work without this line.** See `https://the1truejoe.github.io/openHC/ea/audio/`.

*Replaceable by a DTB* — both halves (the node and a `fixed-clock`).

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

## Per-driver notes — `board/ea-common/kernel/drivers/`

These are plain `.c` files copied into the kernel tree by
`OHC_KERNEL_MIRROR_HOOK` and registered through `drivers/objs.mk`. They were
patches until they didn't need to be; nothing about the code changed in the
move. They are built **in**, not as modules, which matters: `gpio-intelce` has
to exist before `leds-ea-board` claims GPIOs, and `spi-ea-b53-board` registers
its board info at `subsys_initcall` — *before* `spi-pxa2xx-pci` registers the
controller. As loadable modules that ordering would have to be rebuilt by hand.

### `gpio/gpio-intelce.c`

**Necessary as a patch: mostly no**, with one caveat. New standalone driver:
128 lines across 4 banks × 32, reconstructed from Control4's GPL drop.

The caveat is that mainline's `gpio-sodaville` binds the **same** PCI ID
(`8086:2e67`) with the wrong CE4100 layout (12 lines instead of 128), so it must
be disabled — `# CONFIG_GPIO_SODAVILLE is not set` lives in
`board/ea-common/linux/common.fragment`. An out-of-tree module would still work,
but only if that config stays set, so the two are coupled.

Confirmed: `intelce-gpio 0000:01:0b.1: CE5300 GPIO: 128 lines on BAR0`.

Note the 7.1.8 API detail: `gpio_chip.set` returns `int`, not `void`.

### `pwm/pwm-ce5300.c`

**Necessary as a patch: no.** Entirely new driver binding PCI `8086:089f` for
the fan. Touches only `drivers/pwm/{Kconfig,Makefile}` plus its own new file.
No upstream driver exists.

*Could be an out-of-tree module.* It is a patch mainly so `CONFIG_PWM_CE5300=y`
works like any other kernel symbol.

### `leds/leds-ea-board.c`

**Necessary as a patch: no.** New file, `leds-gpio` platform data, no upstream
changes. Board description again.

Map extracted from the stock kernel binary:

| LED | GPIO | polarity |
|---|---|---|
| `c4::network` | 15 | active high |
| `warn::red` | 99 | active high |
| `warn::yellow` | 16 | active high |
| `warn::blue` | 100 | active high |
| `c4::4ball_red` | 102 | **active low** |
| `c4::4ball_blue` | 34 | **active low** |

Confirmed: `ea-leds: 6 front-panel LEDs registered`.

*Replaceable by a DTB* — this is textbook `gpio-leds` DT.

### `spi/spi-ea-b53-board.c`

**Necessary as a patch: no — this is pure board description.** One new file,
no upstream changes. It exists solely because there is no DTB.

It is also the patch that has burned the most time, so the traps are recorded
here:

1. **`CONFIG_SPI_PXA2XX_PCI` cannot be set directly.** It is
   `def_tristate SPI_PXA2XX && PCI && COMMON_CLK` — a *derived* symbol, so a
   fragment assignment is silently dropped. `COMMON_CLK` is the term that was
   missing; without it the PCI glue is never compiled and `01:0b.4` sits
   unbound with an empty `/sys/class/spi_master`.
2. **The SPI bus number is not 0.** `ce4100_spi_setup()` does
   `ssp->port_id = dev->devfn`, and `pxa2xx_spi_probe()` then does
   `controller->bus_num = ssp->port_id`. At `01:0b.4` that is
   `(0x0b << 3) | 4 = 92`, so the controller is **spi92** and a `board_info`
   queued for bus 0 never matches. The file derives it from `devfn` rather than
   hardcoding 92.
3. **DSA's platform_data path needs `netdev[cpu_port]`.** `dsa_port_parse()`
   resolves `"cpu"` with `dev_find_class(cd->netdev[port], "net")`; leaving it
   NULL makes the port permanently unresolvable no matter how correct the rest
   of the port map is. It is set to the e1000's `struct device`
   (`8086:2e6e`, `01:0c.0`); `-EPROBE_DEFER` covers ordering.
4. **The b53 config symbols are `CONFIG_B53` and `CONFIG_B53_SPI_DRIVER`** —
   *not* `CONFIG_NET_DSA_B53*`, which do not exist and which kconfig discards
   without a word.

Port map (measured, see the file's comments): CPU = port 5, jacks = ports 1 and
2, and port 1 is isolated in the stock configuration.

*This patch should be the first thing deleted if a DTB lands.*

### `video/fbdev/ce5300-fb.c`

Simple framebuffer glue. The only driver here gated by a real upstream symbol
(`CONFIG_FB_SIMPLE`), which is why its `objs.mk` line uses `obj-$(CONFIG_...)`
where the others use plain `obj-y`.

---

## Rules of thumb

* **Changing an upstream file → patch.** You cannot express an edit as a copy.
  That is 0001 and 0003, and nothing else currently qualifies.
* **Adding a new file → `board/ea-common/kernel/drivers/` + a line in `objs.mk`.**
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
