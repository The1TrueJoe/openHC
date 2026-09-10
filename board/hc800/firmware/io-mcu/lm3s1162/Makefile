# io-mcu — openHC IO-processor firmware for the HC-800's LM3S1162.
#
#   make fw       cross-build the application ELF + raw .bin
#   make image    ... and wrap it in Control4's flashable container
#   make verify   check a container's CRC (IMAGE=path, defaults to ours)
#   make clean
#
# Needs arm-none-eabi-gcc (brew install --cask gcc-arm-embedded).
#
# ── Why three sources come from the EA tree ───────────────────────────────
#
# ohc_proto.[ch], ir_engine.[ch] and libc_min.c are hardware-free by design —
# they include nothing but <stdint.h>/<stdbool.h>/<stddef.h>/<string.h> — and
# the protocol they implement is the SAME on both parts. That is not an
# assumption: a live HC-800 answered the EA's frame encoder byte for byte (see
# the header of src/main.c). Copying them here would fork a tested
# implementation to no purpose.
#
# They should really live in a shared io-mcu/common/ directory. Pointing at the
# EA tree is the smaller change today; make that move when a third part appears.
SHARED = ../../../../ea-common/firmware/io-mcu/tm4c1231d5

BOARD  ?= hc800
CROSS  ?= arm-none-eabi-
VERSION ?= 0.1.0

BOARDS := hc800
ifeq ($(filter $(BOARD),$(BOARDS)),)
  $(error BOARD must be one of: $(BOARDS)  (got '$(BOARD)'))
endif
BOARD_DEF := -DOHC_BOARD=OHC_BOARD_$(shell echo $(BOARD) | tr a-z A-Z)

BUILD = build/$(BOARD)

# cortex-m3, NOT the EA's cortex-m4: the LM3S1162 is a Stellaris Cortex-M3 with
# no FPU and no DSP extensions. Building m4 code for it produces instructions
# the part will fault on.
MCUFLAGS = -std=gnu11 -mcpu=cortex-m3 -mthumb -Os \
           -ffunction-sections -fdata-sections -ffreestanding \
           -Wall -Wextra -Iinclude -I$(SHARED)/include $(BOARD_DEF)
LDFLAGS  = -T lm3s.ld -nostdlib -Wl,--gc-sections -Wl,-Map,$(BUILD)/io-mcu.map

FW_SRC = src/startup.c src/hal_lm3s.c src/main.c \
         $(SHARED)/src/ohc_proto.c $(SHARED)/src/ir_engine.c $(SHARED)/src/libc_min.c

.PHONY: fw image verify size clean
fw: | $(BUILD)
	@command -v $(CROSS)gcc >/dev/null 2>&1 || { \
	  echo "need $(CROSS)gcc (brew install --cask gcc-arm-embedded)"; exit 1; }
	$(CROSS)gcc $(MCUFLAGS) $(LDFLAGS) $(FW_SRC) -o $(BUILD)/io-mcu.elf
	$(CROSS)objcopy -O binary $(BUILD)/io-mcu.elf $(BUILD)/io-mcu.bin
	@$(MAKE) --no-print-directory size BOARD=$(BOARD)

image: fw
	python3 tools/mkimage.py $(BUILD)/io-mcu.bin $(BUILD)/io-mcu-fw.bin \
	  --version $(VERSION)
	@python3 tools/mkimage.py --verify $(BUILD)/io-mcu-fw.bin

IMAGE ?= $(BUILD)/io-mcu-fw.bin
verify:
	@python3 tools/mkimage.py --verify $(IMAGE)

# The vector table check the EA firmware does, adjusted for this part: the table
# is 16 system entries + 46 IRQs, the app is linked at 0x1000, and flash tops
# out at 0x10000. A bad table is not a warning, it is a board that boots into
# nothing — so refuse to call the output flashable.
size:
	@echo "board: $(BOARD)"
	@$(CROSS)size $(BUILD)/io-mcu.elf
	@echo "image: $$(wc -c < $(BUILD)/io-mcu.bin) bytes  (app slot is 60 KB at 0x1000)"
	@python3 -c "import struct,sys; \
d=open('$(BUILD)/io-mcu.bin','rb').read(); N=16+46; \
sp,pc=struct.unpack_from('<II',d,0); \
print(f'  vector table: SP={sp:#010x} PC={pc:#010x} ({N} entries)'); \
ok = 0x20000000 <= sp <= 0x20008000 and 0x1000 <= pc < 0x10000 and len(d) >= N*4; \
bad=[i for i in range(2,N) for v in [struct.unpack_from('<I',d,i*4)[0]] if v and (not (v&1) or not 0x1000 <= (v&~1) < 0x10000)]; \
ok = ok and not bad; \
print('  all vectors valid' if ok else f'  INVALID: {bad[:6]}'); \
sys.exit(0 if ok else 1)" \
	  || { echo "  BAD vector table — refusing to call this flashable"; exit 1; }

$(BUILD):
	@mkdir -p $(BUILD)

clean:
	rm -rf build
