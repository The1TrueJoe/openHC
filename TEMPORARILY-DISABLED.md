# Temporarily disabled to get Home Assistant booting (2026-08-31)

Re-enable these once HA is up. Nothing here is deleted, only switched off.

## board/ea3-v2/ohc.features
Was:  emmc switch audio
Now:  emmc

* **switch** — BCM53125/DSA. Not needed to reach the network: the e1000
  fake-PHY patch (always applied, not part of this feature) gives eth0 on the
  primary jack, and S40net falls back to the master interface when DSA does not
  attach. Costs kernel size and build time.
* **audio** — our new CE5300 I2S + ADAU1451 drivers. They compile on i686 but
  have never been built for x86_64, and the platform driver is probe-only
  anyway (enable_pcm=0), so it can contribute nothing right now except size and
  a compile risk.
* **sgx** — already removed earlier in the session (WPE WebKit dominated build
  time). Also note the SGX userspace blobs are 32-bit only, so it cannot work on
  x86_64 without i386 multiarch.

## Why size matters here — now MEASURED, and it is worse than assumed

CEFDK's bootlinux copy window is 7,417,856 bytes. The prediction above was
right, and the numbers are now real:

| build | bzImage | headroom |
|---|---|---|
| x86_64, features = `emmc` | 7,169,024 | 248,832 (3.4%) |
| + unused NIC and ISO9660 trims | 7,037,952 | 379,904 (5.1%) |

`.text` is 9.6 MB, `.rodata` 3.4 MB. The config is ALREADY lean — no netfilter,
IPv6, Bluetooth, SCSI/ATA/MD, DRM, sound, media, ftrace or debug info. There is
no fat left to cut: **a 64-bit kernel on this board simply costs about 7 MB.**

### So these features cannot come back as they are

All three fragments are written `=y`. `CONFIG_MODULES` is on and exactly ONE
symbol in the whole config is `=m`, against 1370 that are `=y` — every one of
them in the image whether the board uses it or not.

**Re-enable them as modules (`=m`), not built in.** Modules live on the p1
rootfs, load after it mounts, and cost nothing against the window. Only what is
needed to REACH the rootfs must be built in: e1000, sdhci/mmc, ext4, serial,
PCI, ACPI.

`audio` has a second blocker regardless of size: `ce5300-i2s.c` reading TX
register 0x2004 HANGS the SoC, so `dump_regs` must stay off.

`sgx` has a third: its userspace blobs are 32-bit only, so it cannot work on
x86_64 without i386 multiarch.

If a genuinely large kernel is ever unavoidable, the escape hatch is a small
bootstrap kernel that `kexec`s the real one from p1 — kexec has no such limit.

## Since added back (not disabled — recorded so the list stays honest)

* `COMMON_CLK`, `SPI`, `SPI_PXA2XX` — needed for `MTD_SPI_NOR`, which is what
  lets the boot autoscript be rewritten to whole-slot reads. `COMMON_CLK` no
  longer breaks i2c: patch 0001 registers a fixed-rate clock per adapter.
* `SERIAL_8250_NR_UARTS` 4 -> 8 — the four legacy ISA declarations were
  consuming every 8250 slot, so the REAL PCI UARTs (three BARs on 8086:2e66)
  could not register at all: `Couldn't register serial port 0, irq 25 ... -28`
  (ENOSPC). This is why every hunt for the Zigbee radio on ttyS1..3 came back
  empty — those are the legacy ports, they enumerate `irq = 0`, and a port with
  no IRQ transmits fine but never receives.

Backups: scratchpad/ea-common_defconfig.i386.bak (the i386 arch lines),
scratchpad/ohc.features.bak (the original feature list). NOTE: /private/tmp was
cleared by a reboot on 2026-08-31, so those backups are gone — the current
values are in git, and the original feature list is recorded at the top of this
file.
