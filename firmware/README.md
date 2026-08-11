# Secondary firmware

The x86 SoC is not the only processor on these boards. Each controller carries
one or more auxiliary chips that run their own firmware, and they differ by
model — so this tree is organised by *component*, with a directory per device
variant rather than one blob per board.

```
firmware/
  io-mcu/          the front-panel / IO microcontroller (IR, contacts, relays, serial)
    tm4c1231d5/    EA1 / EA3 / EA5  — TI Tiva, Cortex-M4F         (implemented)
    lm3s1162/      HC800 / HC250    — TI Stellaris, Cortex-M3     (placeholder)
  fpga/            EA3 / EA5 audio-path FPGA                      (placeholder)
  dsp/
    adau1451/      EA3 / EA5 Analog Devices SigmaDSP              (placeholder)
```

## Why per-device, not per-board

The IO MCU speaks the same host protocol (DLE/STX over a UART) across the whole
line, but the silicon underneath is not the same part — a Cortex-M4F on the EA
family, a Cortex-M3 on the HC family — so the linker script, startup, and
peripheral map differ and cannot share a build. Keeping each MCU in its own
directory lets the shared protocol live in one place while the hardware layer
stays honest about which chip it targets.

The EA1 — the current target of this repo — only populates `io-mcu/tm4c1231d5`.
The other directories are stubs that record what a given board needs, so the
structure is ready when those boards are.

## Relationship to the main firmware

None of this is the Linux image. The kernel/rootfs build (top-level Makefile,
`board/ea1`) produces what runs on the x86 SoC. This tree is the code that runs
*beside* it on the auxiliary processors, flashed over their own links (the TI
serial bootloader for the IO MCU; I2C for the DSP; the FPGA's own load path).
