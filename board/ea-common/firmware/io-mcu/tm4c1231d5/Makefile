# io-mcu — openHomeController IO-processor firmware.
#
#   make test              protocol tests on the host (no MCU, no cross toolchain)
#   make fw                cross-build a flashable image for the TM4C1231D5
#   make fw BOARD=ea3      ... for an EA3 instead of the default EA1
#   make all-boards        build every supported board in one go
#   make clean
#
# The protocol core is hardware-free on purpose so `make test` is the fast loop.
#
# BOARD selects a compile-time board profile (include/board_profile.h): how many
# IR outputs, user serial ports, relays and contacts the board populates. Unlike
# the stock firmware — which carries all six profiles and picks one at runtime
# from an ADC strap — we emit one image per board, because the ADC values that
# drive that selection have not been measured yet. Images are therefore NOT
# interchangeable between an EA1 and an EA3.

BOARD   ?= ea1
CC      ?= cc
CFLAGS  ?= -std=gnu11 -Wall -Wextra -Iinclude -Itest
CROSS   ?= arm-none-eabi-

BOARDS := ea1 ea3
ifeq ($(filter $(BOARD),$(BOARDS)),)
  $(error BOARD must be one of: $(BOARDS)  (got '$(BOARD)'))
endif
BOARD_DEF := -DOHC_BOARD=OHC_BOARD_$(shell echo $(BOARD) | tr a-z A-Z)

BUILD    = build/$(BOARD)
MCUFLAGS = -std=gnu11 -mcpu=cortex-m4 -mthumb -Os -ffunction-sections -fdata-sections \
           -Wall -Wextra -Iinclude -ffreestanding $(BOARD_DEF)
LDFLAGS  = -T tm4c.ld -nostdlib -Wl,--gc-sections -Wl,-Map,$(BUILD)/io-mcu.map

FW_SRC = src/startup.c src/libc_min.c src/hal_tm4c.c src/ohc_proto.c \
         src/ir_engine.c src/main.c

.PHONY: fw clean size all-boards
fw: | $(BUILD)
	@command -v $(CROSS)gcc >/dev/null 2>&1 || { \
	  echo "need $(CROSS)gcc (brew install --cask gcc-arm-embedded)"; exit 1; }
	$(CROSS)gcc $(MCUFLAGS) $(LDFLAGS) $(FW_SRC) -o $(BUILD)/io-mcu.elf
	$(CROSS)objcopy -O binary $(BUILD)/io-mcu.elf $(BUILD)/io-mcu.bin
	@$(MAKE) --no-print-directory size BOARD=$(BOARD)

all-boards:
	@for b in $(BOARDS); do $(MAKE) --no-print-directory fw BOARD=$$b || exit 1; done

size:
	@echo "board: $(BOARD)"
	@$(CROSS)size $(BUILD)/io-mcu.elf
	@echo "image: $$(wc -c < $(BUILD)/io-mcu.bin) bytes  (app slot is 60 KB at 0x1000)"
	@python3 -c "import struct,sys; \
d=open('$(BUILD)/io-mcu.bin','rb').read(); N=16+107; \
sp,pc=struct.unpack_from('<II',d,0); \
print(f'  vector table: SP={sp:#010x} PC={pc:#010x} ({N} entries)'); \
ok = 0x20000000 <= sp <= 0x20006000 and 0x1000 <= pc < 0x10000 and len(d) >= N*4; \
bad=[i for i in range(2,N) for v in [struct.unpack_from('<I',d,i*4)[0]] if v and (not (v&1) or not 0x1000 <= (v&~1) < 0x10000)]; \
ok = ok and not bad; \
print('  all 123 vectors valid' if ok else f'  INVALID: {bad[:6]}'); \
sys.exit(0 if ok else 1)" \
	  || { echo "  BAD vector table — refusing to call this flashable"; exit 1; }

$(BUILD):
	@mkdir -p $(BUILD)

clean:
	rm -rf build
