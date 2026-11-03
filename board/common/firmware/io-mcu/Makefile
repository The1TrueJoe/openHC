# openHC IO-MCU firmware: one image per part, configured at run time by the
# host's CONFIG_SET. See README.md.
#
#   make                    every part, into build/<part>/
#   make PARTS=lm3s1162     one part
#   make test               host tests: no MCU, no cross toolchain
#   make clean

PARTS  ?= tm4c1231d5 lm3s1162
CROSS  ?= arm-none-eabi-
HOSTCC ?= cc
PYTHON ?= python3
# ohc-iomcu-proto.h lives with the driver, which speaks the same protocol
PROTO  ?= ../../kernel/drivers/gpio
BUILD  ?= build

CORE  := core/proto.c core/ir.c core/config.c core/app.c
PARTC := parts/tm4c1231d5.c parts/lm3s1162.c
SRCS  := $(sort $(wildcard core/*.c core/*.h parts/*.c hal/*/*.c hal/*/*.h hal/*/*.ld) \
                $(PROTO)/ohc-iomcu-proto.h VERSION)

VERSION := $(shell cat VERSION)
FW_VERSION := openhc-$(VERSION)+$(shell $(PYTHON) tools/mkfw.py hash $(SRCS))

WARN := -Wall -Wextra -Wno-override-init
INC  := -Icore -I$(PROTO)

cpu_tm4c1231d5 := -mcpu=cortex-m4 -mfloat-abi=soft
cpu_lm3s1162   := -mcpu=cortex-m3
uarts_tm4c1231d5 := 7
uarts_lm3s1162   := 2
sram_tm4c1231d5  := 0x20006000
sram_lm3s1162    := 0x20004000

.PHONY: all test version clean $(PARTS)
all: $(PARTS)

version:
	@echo $(FW_VERSION)

define part_rule
$(1): $(BUILD)/$(1)/io-mcu-$(1).bin

$(BUILD)/$(1)/io-mcu.elf: $(SRCS)
	@mkdir -p $$(@D)
	$(CROSS)gcc -std=gnu11 -Os -mthumb $(cpu_$(1)) -ffreestanding -fno-common \
	  -ffunction-sections -fdata-sections $(WARN) -Werror $(INC) -Ihal/$(1) \
	  -DHAL_USER_UARTS=$(uarts_$(1)) -DOHC_FW_VERSION='"$(FW_VERSION)"' \
	  -nostdlib -Wl,--gc-sections -Lhal/common -T hal/$(1)/link.ld \
	  -Wl,-Map,$$(@D)/io-mcu.map \
	  $(CORE) core/main.c core/libc_min.c parts/$(1).c hal/$(1)/hal.c hal/$(1)/startup.c \
	  -lgcc -o $$@

$(BUILD)/$(1)/io-mcu-$(1).bin: $(BUILD)/$(1)/io-mcu.elf tools/mkfw.py
	$(CROSS)objcopy -O binary $$< $$(@D)/io-mcu.raw
	$(CROSS)size $$<
	$(PYTHON) tools/mkfw.py image --part $(1) --version '$(FW_VERSION)' \
	  --sram-top $(sram_$(1)) $$(@D)/io-mcu.raw $$@
endef
$(foreach p,tm4c1231d5 lm3s1162,$(eval $(call part_rule,$(p))))

TEST_SRC := $(CORE) $(PARTC) $(wildcard test/*.c)
TEST_CFLAGS ?= -g -O1 -fsanitize=address,undefined -fno-sanitize-recover=all

test: $(BUILD)/test/run
	$(BUILD)/test/run
	$(PYTHON) tools/test_mkfw.py

$(BUILD)/test/run: $(TEST_SRC) $(wildcard core/*.h test/*.h) $(PROTO)/ohc-iomcu-proto.h
	@mkdir -p $(@D)
	$(HOSTCC) -std=gnu11 $(WARN) -Werror $(TEST_CFLAGS) $(INC) -Itest \
	  -DHAL_USER_UARTS=7 -DOHC_FW_VERSION='"openhc-test"' $(TEST_SRC) -o $@

clean:
	rm -rf $(BUILD)
