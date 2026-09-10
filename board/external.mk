# Buildroot packages live in <repo>/packages/, alongside the Rust workspace —
# BR2_EXTERNAL_OPENHC_PATH points at board/, so reach up one level.
include $(sort $(wildcard $(BR2_EXTERNAL_OPENHC_PATH)/../packages/*/*.mk))
#
# ...and every package that lives WITH A BOARD instead. This glob covers
# board/<anything>/packages/, so ea-common (the CE5300 SGX/WPE stack), hc800
# (ths8200 — that DAC is HC-800 silicon) and common (splash, a base
# firmware feature) are all picked up without naming each one here.
#
# The split is deliberate: `packages/` at the repo root is for SERVICES — the
# daemons with APIs. Anything that is board silicon or a base firmware feature
# belongs with the board tree that owns it, and lands here.
include $(sort $(wildcard $(BR2_EXTERNAL_OPENHC_PATH)/*/packages/*/*.mk))

# ── openHC's own kernel drivers: copied in, not patched in ─────────────────
#
# board/ea-common/kernel/drivers/ mirrors the kernel's drivers/ subtree, so a file at
# video/fbdev/foo.c lands at $(LINUX_DIR)/drivers/video/fbdev/foo.c, and
# drivers/objs.mk says which kernel Makefile each one is registered in.
#
# This is strictly better than carrying a driver inside a .patch: the source
# stays a normal .c file you can edit, grep and compile-check, and there are no
# context lines to go stale on a kernel bump. Patches remain the only option for
# CHANGING existing upstream files — you cannot express an edit as a copy — so
# 0001 (i2c-pxa-pci) and 0003 (e1000 fake PHY) stay patches and everything that
# is merely a NEW file lives here instead.
#
# This hook also replaced three near-identical per-driver hooks (SPI/GPIO/LED)
# that each appended one obj-y line. objs.mk is the table those became.
#
# The mirror covers more than drivers/ now: ASoC lives in sound/, which is not
# under drivers/, so the hook walks a list of kernel subtrees instead of a single
# hardcoded one. objs.mk needs no change for this — its left-hand column was
# always a full kernel-relative Makefile path.
# board/ea-common/kernel/ is a MIRROR OF THE KERNEL SOURCE TREE: a file at
# kernel/sound/soc/ce5300/foo.c lands at $(LINUX_DIR)/sound/soc/ce5300/foo.c.
# That is why sound/ sits BESIDE drivers/ rather than under it — the kernel has
# them as siblings, and the mirror has to match or the paths stop being obvious.
# The `kernel/` parent exists so the two are visibly a mirror instead of two
# bare directories that look like peers of packages/ and firmware/.
#
# objs.mk lives at the TOP of the mirror, not inside drivers/, because it
# registers entries in both subtrees — it always had a
# `sound/soc/codecs/Makefile|...` line while sitting in drivers/, which was the
# giveaway that it belonged one level up.
# A mirror is a directory holding kernel-source subtrees plus the objs.mk that
# registers them. There are two, and ONE hook walks both — the difference
# between them is which list they are in, not a second copy of the loop. (This
# file already collapsed three near-identical per-driver hooks into one once;
# a fourth pasted copy is how that happens again.)
#
#   common/kernel     every board. Not tied to any SoC.
#   ea-common/kernel  EA FAMILY ONLY. Everything in it is Intel CE5300 silicon
#                     and is registered obj-y, so on another board it is at best
#                     dead weight in the kernel and at worst a link failure —
#                     which is what it was: hc800 died at `LD vmlinux` on the
#                     ASoC machine drivers, because it uses SND_HDA_INTEL and
#                     defines no CONFIG_SND_SOC. Gating by list membership keeps
#                     that explicit and greppable.
#                     BR2_OHC_EA_KERNEL_DRIVERS is set by ea-common_defconfig
#                     and by nothing else.
OHC_KERNEL_SUBTREES = drivers sound
# NOT unconditional, despite living under common/. The only thing in that mirror
# is the IO-MCU gpiochip driver, and two boards have no IO microcontroller —
# mirroring it there cost them a vmlinux link failure on rc-core. See
# BR2_OHC_IOMCU_KERNEL_DRIVER in board/Config.in.
OHC_KERNEL_MIRRORS =
ifeq ($(BR2_OHC_IOMCU_KERNEL_DRIVER),y)
OHC_KERNEL_MIRRORS += $(BR2_EXTERNAL_OPENHC_PATH)/common/kernel
endif
ifeq ($(BR2_OHC_EA_KERNEL_DRIVERS),y)
OHC_KERNEL_MIRRORS += $(BR2_EXTERNAL_OPENHC_PATH)/ea-common/kernel
endif
ifeq ($(BR2_OHC_HC800_KERNEL_DRIVERS),y)
OHC_KERNEL_MIRRORS += $(BR2_EXTERNAL_OPENHC_PATH)/hc800/kernel
endif

define OHC_KERNEL_MIRROR_HOOK
	for m in $(OHC_KERNEL_MIRRORS); do \
		who=$${m##*/board/}; \
		for t in $(OHC_KERNEL_SUBTREES); do \
			[ -d "$$m/$$t" ] || continue; \
			( cd "$$m/$$t" && \
			  find . \( -name '*.c' -o -name '*.h' -o -name 'Makefile' \) | sed 's|^\./||' ) | while read -r f; do \
				install -D -m644 "$$m/$$t/$$f" "$(LINUX_DIR)/$$t/$$f"; \
				echo "openHC: installed $$t/$$f ($$who)"; \
			done; \
		done; \
		[ -f "$$m/objs.mk" ] || continue; \
		while IFS='|' read -r mk line; do \
			case "$$mk" in ''|\#*) continue;; esac; \
			obj=$${line##*+= }; \
			if ! grep -q "$$obj" "$(LINUX_DIR)/$$mk"; then \
				echo '' >> "$(LINUX_DIR)/$$mk"; \
				echo '# openHC driver, registered by OHC_KERNEL_MIRROR_HOOK.' \
					>> "$(LINUX_DIR)/$$mk"; \
				echo "$$line" >> "$(LINUX_DIR)/$$mk"; \
				echo "openHC: appended $$obj to $$mk ($$who)"; \
			fi; \
		done < "$$m/objs.mk"; \
	done
endef
LINUX_POST_PATCH_HOOKS += OHC_KERNEL_MIRROR_HOOK
