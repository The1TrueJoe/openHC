# LM3S1162 IO MCU (HC800 / HC250)

Placeholder. The HC family uses a Stellaris **LM3S1162** (Cortex-M3) for the IO
MCU instead of the EA family's TM4C1231D5 (Cortex-M4F). The wire protocol to the
host is the same DLE/STX framing (see `../tm4c1231d5`), but the core, linker
script, and peripheral map differ. Not implemented — the EA1 is the current
target.
