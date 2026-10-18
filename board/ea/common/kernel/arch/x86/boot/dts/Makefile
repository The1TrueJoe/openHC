# SPDX-License-Identifier: GPL-2.0
# Control4 EA-series device trees. The shared .dtsi files are mirrored in from
# here; the board's own board/ea/<board>/<board>.dts is copied in beside them
# by OHC_EA_BOARD_DTS_HOOK (board/external.mk). The board fragment
# names it in CONFIG_BUILTIN_DTB_NAME and patches/linux/0015 links it in.
dtb-$(CONFIG_BUILTIN_DTB) += $(CONFIG_BUILTIN_DTB_NAME:"%"=%).dtb
