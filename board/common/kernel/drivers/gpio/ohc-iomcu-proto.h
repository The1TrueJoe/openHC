/* SPDX-License-Identifier: (GPL-2.0 OR MIT) */
/*
 * openHC IO microcontroller: the wire protocol, the config frame, the
 * firmware image header and the TI serial bootloader, shared by the kernel
 * driver (gpio-ohc-iomcu.c) and openHC's firmware (board/common/firmware/
 * io-mcu). Constants only, so it builds in both.
 *
 * https://the1truejoe.github.io/openHC/shared/io-mcu/
 */
#ifndef OHC_IOMCU_PROTO_H
#define OHC_IOMCU_PROTO_H

/* ---- framing: DLE STX | opcode seq flags len16be | payload | cksum ---- */
#define OHC_DLE			0x10
#define OHC_STX			0x02
#define OHC_HDR_LEN		5	/* opcode, seq, flags, len hi, len lo */
#define OHC_FLAG_RESPONSE	0x02
#define OHC_MAX_PAYLOAD		1040	/* 14 + 256 Pronto pairs, rounded up */

/* ---- opcodes: Control4's (reply = request + 1 unless noted) ---- */
#define OHC_OP_IR_PIN_STATE_SET		0x11
#define OHC_OP_IR_PIN_STATE_CLEAR	0x14
#define OHC_OP_PRODUCT_NAME		0x24
#define OHC_OP_FIRMWARE_VERSION		0x34
#define OHC_OP_IR_MODE_SET		0x41
#define OHC_OP_RELAY_GET		0x54	/* [index] -> 0x55 [index, on] */
#define OHC_OP_RELAY_STATE		0x55
#define OHC_OP_RELAY_TOGGLE		0x56	/* [index] -> 0x55 [index, on] */
#define OHC_OP_IROUT_SEND		0x66	/* -> 0x68 [status] */
#define OHC_OP_IROUT_STATUS		0x68
#define OHC_OP_IROUT_STOP		0x69
#define OHC_OP_CONTACT_GET		0x74	/* -> 0x75 u32 LE, 1 = closed */
#define OHC_OP_CONTACT_STATE		0x75
#define OHC_OP_IRIN_SET_CAPTURE		0x77
#define OHC_OP_IRIN_CAPTURED		0x97	/* unsolicited */
#define OHC_OP_UART_SET_CONTROL		0xa0	/* no reply */
#define OHC_OP_UART_SEND		0xa1	/* no reply */
#define OHC_OP_UART_RECEIVE		0xa2	/* unsolicited */
#define OHC_OP_UART_MODE		0xa3	/* [port, 0 serial / 1 null-modem], no reply */
#define OHC_OP_AUTO_BAUD_GET		0xd2	/* -> 0xd7 u32 BE baud */
#define OHC_OP_AUTO_BAUD_RESP		0xd7

/* ---- opcodes: openHC's own; stock firmware drops them unanswered ---- */
#define OHC_OP_CONFIG_SET		0xf0	/* -> 0xf1 */
#define OHC_OP_CONFIG_RESP		0xf1
#define OHC_OP_RESET			0xf2	/* -> 0xf3, then reset into the bootloader */
#define OHC_OP_RESET_ACK		0xf3

/* A bare 55 55 between frames: the app answers AUTO_BAUD_RESP. */
#define OHC_KNOCK		0x55

/*
 * FIRMWARE_VERSION from openHC's firmware starts with this; anything else is
 * stock. The rest is the image header's version string, byte for byte.
 */
#define OHC_FW_VERSION_PREFIX	"openhc-"

/* ---- IROUT_SEND: 14 fixed bytes, then u16 BE durations in carrier periods ---- */
#define OHC_IR_FIXED		14
#define OHC_IR_OFF_MODE		0	/* < 2: every word once; else repeat count, 0xff = until stop */
#define OHC_IR_OFF_MASK		1	/* u24 BE, bit n = output n */
#define OHC_IR_OFF_CARRIER	8	/* Pronto word: Hz = 4145146 / word; 0 is invalid */
#define OHC_IR_OFF_ONCE		10	/* pairs in the once sequence (mode >= 2) */
#define OHC_IR_OFF_REPEAT	12	/* pairs in the repeat sequence (mode >= 2) */
#define OHC_IR_PRONTO_HZ	4145146
#define OHC_IR_CARRIER_MIN	20000
#define OHC_IR_CARRIER_MAX	460000
#define OHC_IR_DURATION_MAX	0x7fff
#define OHC_IR_MAX_OUTPUTS	24

#define OHC_IR_ST_OK		0	/* accepted (stock: sent) */
#define OHC_IR_ST_STOPPED	1	/* async: IROUT_STOP ended it */
#define OHC_IR_ST_REJECTED	2	/* malformed, or no configured output in the mask */
#define OHC_IR_ST_BUSY		3	/* another code is still going out */
#define OHC_IR_ST_DONE		4	/* async: transmitted */

/* ---- IRIN_CAPTURED: u16 BE carrier period, then u16 BE bursts ---- */
#define OHC_IRIN_TICK_HZ	50000000	/* unit of the carrier period */
#define OHC_IRIN_MARK		0x8000
#define OHC_IRIN_DURATION	0x3fff		/* carrier periods */
#define OHC_IRIN_WORDS		100		/* bursts per frame; a longer code is chunked */

/* ---- config frame: [version, count, count x {kind, index, port, pin, flags}] ---- */
#define OHC_CFG_VERSION		1
#define OHC_CFG_HDR		2
#define OHC_CFG_ENTRY		5
#define OHC_CFG_MAX_ENTRIES	64

#define OHC_CFG_IR_OUT		1	/* index: IROUT_SEND bit; pin: its own carrier output */
#define OHC_CFG_IR_IN		2	/* index 0; pin: receiver */
#define OHC_CFG_RELAY		3	/* index: RELAY_GET selector */
#define OHC_CFG_CONTACT		4	/* index: CONTACT_GET bit */
#define OHC_CFG_UART_RX		5	/* index: UART_* port */
#define OHC_CFG_UART_TX		6
#define OHC_CFG_IR_CARRIER	7	/* index: carrier source 0..3; pin: shared carrier output */
#define OHC_CFG_IR_GATE		8	/* index: IROUT_SEND bit; pin: line routing a carrier to it */
#define OHC_CFG_HOLD		9	/* index: 0..; pin: held at a fixed level */
#define OHC_CFG_HEARTBEAT	10	/* index 0; pin: toggled while the app runs */
#define OHC_CFG_UART_MUX_A	11	/* index: UART_* port; high = the jack is serial */
#define OHC_CFG_UART_MUX_B	12	/* index: UART_* port; high = serial, RX/TX swapped */

#define OHC_CFG_F_ACTIVE_LOW	0x01	/* relay, contact, gate, demodulated receiver */
#define OHC_CFG_F_PULL_UP	0x02	/* contact */
#define OHC_CFG_F_PULL_DOWN	0x04	/* contact */
#define OHC_CFG_F_DEMODULATED	0x08	/* receiver outputs the envelope, not the carrier */
#define OHC_CFG_F_HIGH		0x10	/* hold: the level */
#define OHC_CFG_F_CARRIER(n)	((n) << 5)	/* gate: which carrier source it routes */
#define OHC_CFG_F_CARRIER_OF(f)	(((f) >> 5) & 3)
#define OHC_CFG_CARRIERS	4

/* reply: [version, result, count, count x status] */
#define OHC_CFG_R_OK		0
#define OHC_CFG_R_VERSION	1	/* frame version not spoken: nothing applied */
#define OHC_CFG_R_LENGTH	2	/* truncated or too many entries: nothing applied */

#define OHC_CFG_S_OK		0
#define OHC_CFG_S_KIND		1	/* unknown kind */
#define OHC_CFG_S_INDEX		2	/* index out of range, or used twice */
#define OHC_CFG_S_PIN		3	/* no such pin on this part */
#define OHC_CFG_S_RESERVED	4	/* host link or debug pin */
#define OHC_CFG_S_FUNC		5	/* the pin cannot do this */
#define OHC_CFG_S_BUSY		6	/* pin or peripheral already taken */
#define OHC_CFG_S_PAIR		7	/* partner missing: UART RX/TX on one UART, a gate's carrier */
#define OHC_CFG_S_FLAGS		8	/* flag not valid for this kind */

/* ---- MCU parts ---- */
#define OHC_PART_TM4C1231D5	1
#define OHC_PART_LM3S1162	2

/* Both parts: 64 KB flash, 1 KB pages, TI bootloader in the low 4 KB. */
#define OHC_APP_BASE		0x1000
#define OHC_FLASH_SIZE		0x10000
#define OHC_FLASH_PAGE		0x400

/*
 * ---- firmware image: a 64-byte header (little-endian), then the bytes
 * that go to flash at load_addr ----
 */
#define OHC_IMG_MAGIC		"OHCIOMCU"
#define OHC_IMG_HDR_LEN		64
#define OHC_IMG_OFF_MAGIC	0	/* 8 bytes */
#define OHC_IMG_OFF_HDR_VER	8	/* u16 = 1 */
#define OHC_IMG_OFF_HDR_LEN	10	/* u16 = 64 */
#define OHC_IMG_OFF_PART	12	/* u32 OHC_PART_* */
#define OHC_IMG_OFF_LOAD	16	/* u32, >= OHC_APP_BASE */
#define OHC_IMG_OFF_SIZE	20	/* u32, multiple of 4 */
#define OHC_IMG_OFF_CRC		24	/* u32 CRC-32 (IEEE) of the payload */
#define OHC_IMG_OFF_VERSION	28	/* 32 bytes, NUL padded: FIRMWARE_VERSION's reply */
#define OHC_IMG_OFF_HDR_CRC	60	/* u32 CRC-32 of bytes 0..59 */
#define OHC_IMG_VERSION_LEN	32

/* ---- TI serial bootloader: [size = n + 2][sum of data][data] ---- */
#define OHC_BL_ACK		0xcc
#define OHC_BL_NAK		0x33
#define OHC_BL_PING		0x20
#define OHC_BL_DOWNLOAD		0x21	/* addr BE32, size BE32: erases the range */
#define OHC_BL_RUN		0x22	/* addr BE32: never ACKed */
#define OHC_BL_GET_STATUS	0x23
#define OHC_BL_SEND_DATA	0x24
#define OHC_BL_SUCCESS		0x40
#define OHC_BL_CHUNK		32	/* SEND_DATA payload; TI allows more, every build takes this */

#endif /* OHC_IOMCU_PROTO_H */
