# IO Extender — overnight status

## Working: relays (8), contacts (8), front LEDs, webd, MQTT iod — all end-to-end.
## FPGA (serial + IR): NOT working. Root cause narrowed to a hardware-level
## startup step that needs JTAG/scope to resolve. Everything softwareaddressable
## has been ruled out. Details below.

---

## Autonomy set up tonight (reusable)

- **Kasa "Bench" plug** power-cycles the IOX. Controlled with no cloud creds
  (HS110, local): `~/openhc-iox-netboot/.venv/bin/python
  ~/openhc-iox-netboot/kasa_ctl.py {on|off|cycle|status}`. (Not in git.)
- **Persistent flasher supervisor** (`flash-supervisor.sh`, run once with sudo):
  serves netboot without re-sudo. Control it with no sudo:
  `echo openhc|vendor|off > /tmp/ohc-flash-ctl/mode`. Log at
  `/tmp/ohc-flash-ctl/flasher.log`. NOTE: to serve a freshly-built image you
  must toggle the mode (`off` then `openhc`) so the flasher re-opens the file —
  a plain `cp` over the image is not picked up by the running flasher.
- Booting the **vendor OS** (to read reference registers): `echo vendor > mode`,
  power-cycle. It netboots `vendor-uImage`, mounts NAND root, comes up ~10.0.0.113,
  SSH `root` / `t0talc0ntr0l4!` (legacy: `-o HostKeyAlgorithms=+ssh-rsa
  -o KexAlgorithms=+diffie-hellman-group1-sha1`), tool is `peeknpoke`.

## The FPGA problem, precisely

The bitstream loads with **zero CRC errors** — INIT_B stays high through all
169 KB (now verified by an explicit per-byte INIT_B gate). But **DONE never
releases** and the version register floats at `0x0202` (a configured part reads
`0x0400`). So the data is accepted but the **start-up sequence never completes**.

## What was DISPROVEN (don't re-chase these)

1. **It is NOT the video/VENC clock.** Earlier theory (and the first commit) said
   the FPGA's DCM needs the DM355 VENC clock. **Wrong** — proven on the live
   vendor OS, where the FPGA works with `VPSS_CLKCTL = 0` and the VENC block
   floating/unclocked, identical to openHC. That code was removed.
2. **It is NOT the load sequence / driver.** A standalone **userspace bit-bang**
   (mmap /dev/mem, vendor-exact sequence, bypassing the kernel driver entirely)
   fails **identically**. So the driver is not at fault.
3. **It is NOT the bitstream.** `fpga_fw.bin` md5 matches the vendor's file byte
   for byte (`096f05cc…`).
4. **It is NOT any SoC register.** openHC vs the working vendor OS were compared
   register-for-register and are **identical**: the whole system module
   (0x01c40000–70), PLLC1 and PLLC2 in full, all 42 PSC MDSTAT modules, PINMUX0–4,
   and the async-EMIF CS1 timing (`A2CR = 0x00a00505`, where the FPGA lives).
5. **It is NOT PROG polarity/timing.** Verified on hardware: PROG high → INIT_B=0
   (reset), PROG low → INIT_B=1 (ready). Matches the vendor; 5 ms settle added.
6. **It is NOT CCLK regularity / preemption.** Clocking the whole stream with
   IRQs disabled (gap-free CCLK) did not help.
7. **It is NOT VPSS PSC local-reset.** Deasserting LRST to match the vendor
   (MDSTAT 0x1F03) did not help.

## The remaining mystery

Same board, same bitstream, byte-identical SoC register state, same load
sequence, clean data — yet the vendor's `c4fpga.ko` completes start-up (DONE
high, `0x0400`) and openHC does not (DONE low, `0x0202`). The vendor configures
the part **in Linux** (`c4fpga.ko` + `request_firmware`), fresh from a cold
power-cycle — so it is not a U-Boot/warm-state artifact.

Every remotely-observable difference has been eliminated. What's left is only
visible at the pins: whether the FPGA's DCM reference clock is actually present
and what DONE/INIT_B/CCLK do during start-up. **This is the JTAG/scope step.**

## Concrete next steps

1. **Scope or JTAG the FPGA** (J18 header): watch CCLK, DONE, INIT_B and hunt for
   the DCM reference clock during a load. Compare vendor vs openHC at the pins.
   This is the only way left to see what differs.
2. Optional software check to fully close the premise: on the vendor OS, force
   `c4fpga` to re-load the part in Linux (rmmod its dependents + c4fpga, insmod)
   and confirm it reconfigures fresh — verifying the vendor really does a
   cold-equivalent Linux load (I'm ~95% sure it does; this removes all doubt).

## Where things are

- Box: running openHC (netbooted), core IO working. FPGA blank.
- Driver: `board/ioxv1/kernel/drivers/misc/ohc-iox-fpga.c` — vendor-faithful
  loader (INIT_B gate, 5 ms PROG, tried IRQ-off). Loads cleanly; startup stalls.
- Bitstream + vendor GPL source + tools: `/private/tmp/.../c4gpl/`.
