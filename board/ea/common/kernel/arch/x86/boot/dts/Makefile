# SPDX-License-Identifier: GPL-2.0
# Control4 EA-series device trees. The shared .dtsi files are mirrored in from
# here; the board's own <board>.dts from board/ea/<board>/kernel/ (same
# OHC_KERNEL_MIRROR_HOOK, BR2_OHC_EA_BOARD picks the board). The board fragment
# names it in CONFIG_BUILTIN_DTB_NAME and patches/linux/0015 links it in.
dtb-$(CONFIG_BUILTIN_DTB) += $(CONFIG_BUILTIN_DTB_NAME:"%"=%).dtb
