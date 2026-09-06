# Secondary firmware

The x86 SoC is not the only processor on these boards. Each controller carries
one or more auxiliary chips that run their own firmware, and they differ by
model — These now live under the board tree that owns them, alongside everything else
that is board-specific. Zigbee is the exception and sits in board/common,
because every controller except the IOX carries an EM357.

```
board/ea-common/firmware/
  io-mcu/tm4c1231d5/   EA1 / EA3 / EA5  — TI Tiva, Cortex-M4F      (implemented)
  dsp/adau1451/        EA3 / EA5 Analog Devices SigmaDSP           (placeholder)
board/hc800/firmware/
  io-mcu/lm3s1162/     HC800 / HC250    — TI Stellaris, Cortex-M3  (placeholder)
board/common/firmware/
  zigbee/em357/        every board except the IOX                  (host-side tools)
```

## The caveat this layout accepts

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


## Note on the earlier layout

This used to be one top-level `firmware/` tree organised by *component* rather
than by board, on the argument below: the IO MCU speaks the same DLE/STX host
protocol across families, so keeping the variants together kept that shared
knowledge in one place.

Moving to per-board grouping trades that away, and the trade is real: if the
LM3S1162 is ever implemented for the HC family it will need `ohc_proto.h`, which
now lives in the EA tree at
`board/ea-common/firmware/io-mcu/tm4c1231d5/include/`. Copy it or factor it out
at that point -- do not let the two drift, because the whole reason the host
side needs no changes is that both speak the identical protocol.
