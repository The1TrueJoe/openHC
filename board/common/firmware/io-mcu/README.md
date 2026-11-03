# io-mcu — openHC's IO microcontroller firmware

One image per part, configured at run time by the host:

| Part | Boards | Image |
|---|---|---|
| TI TM4C1231D5 (Cortex-M4) | EA-1, EA-3, EA-5 | `/lib/firmware/openhc/io-mcu-tm4c1231d5.bin` |
| TI LM3S1162 (Cortex-M3) | HC-800 | `/lib/firmware/openhc/io-mcu-lm3s1162.bin` |

It speaks Control4's DLE/STX protocol, so the kernel driver
(`board/common/kernel/drivers/gpio/gpio-ohc-iomcu.c`) is the same against it and
against stock. Starts with every pin untouched; the driver's `CONFIG_SET` frame,
built from the board's device tree (or ACPI `_DSD`), says which pin is which.

Docs: https://the1truejoe.github.io/openHC/shared/io-mcu-firmware/

## Layout

```
core/          hardware-free: protocol codec, config validation, IR engine, the app
parts/         each part's pin functions (data; also compiled into the tests)
hal/<part>/    registers, startup, linker script
hal/common/    Cortex-M and PL011 UART helpers, shared sections
test/          host tests: fake HAL, frames captured off live units
tools/mkfw.py  image header, stock-image wrapping, version hash
```

Constants shared with the driver live in
`board/common/kernel/drivers/gpio/ohc-iomcu-proto.h`.

## Build and test

```sh
make            # both images, build/<part>/io-mcu-<part>.bin (needs arm-none-eabi-gcc)
make test       # host tests, any C compiler; ASan + UBSan by default
make version    # the version string the images report
```

On macOS, ASan's runtime can hang at start-up; use
`make test TEST_CFLAGS="-g -O1 -fsanitize=undefined"`.

Buildroot builds the images with `host-arm-gnu-toolchain` (`io-mcu.mk`) whenever
the `iomcu` feature is on. CI runs `make test`.

The version is `openhc-<VERSION>+<hash of the sources>`, so any source change
gives a new version and the driver rewrites the part.
