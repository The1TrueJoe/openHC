/* openHomeController IO-processor firmware — wire protocol.
 *
 * Clean-room implementation of the DLE/STX protocol the Control4 IO MCUs speak,
 * written from observed behaviour (docs/io-mcu-firmware.md). No vendor code.
 *
 * Deliberately free of hardware dependencies so it builds and is tested on the
 * host: the MCU side supplies bytes in and takes bytes out.
 *
 *   DLE STX | opcode | seq | flags | len16 BE | payload... | checksum
 *   0x10 02                                                  negated 8-bit sum
 *                                                            of everything after STX
 * Any 0x10 between opcode and checksum is escaped by doubling it.
 */
#ifndef OHC_PROTO_H
#define OHC_PROTO_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#define OHC_DLE 0x10u
#define OHC_STX 0x02u

/* Largest payload we will accept or emit. The stock firmware chunks IR captures
 * at 202 bytes; 256 burst pairs plus a header is the worst legal IROUT_SEND. */
#define OHC_MAX_PAYLOAD 552u

/* Flags byte. Bit 1 marks a reply; the stock MCU sets it on every response. */
#define OHC_FLAG_RESPONSE 0x02u

/* Opcodes. Replies are request+1 throughout — that is the protocol's rule, not
 * a coincidence, and the vendor's own *_GET / *_STATE naming follows it. */
enum {
    OHC_OP_IR_PIN_STATE_SET      = 0x11,
    OHC_OP_IR_PIN_STATE_CLEAR    = 0x14,
    OHC_OP_PRODUCT_NAME          = 0x24,
    OHC_OP_PRODUCT_NAME_RESP     = 0x25,
    OHC_OP_FIRMWARE_VERSION_GET  = 0x34,
    OHC_OP_FIRMWARE_VERSION_RESP = 0x35,
    OHC_OP_IR_MODE_SET           = 0x41,
    OHC_OP_RELAY_GET             = 0x54,
    OHC_OP_RELAY_STATE           = 0x55,
    OHC_OP_RELAY_TOGGLE          = 0x56,
    OHC_OP_IROUT_SEND            = 0x66,
    OHC_OP_IROUT_STATUS          = 0x68,
    OHC_OP_IROUT_STOP_RAMP       = 0x69,
    OHC_OP_CONTACT_GET           = 0x74,
    OHC_OP_CONTACT_STATE         = 0x75,
    OHC_OP_IRIN_SET_CAPTURE      = 0x77,
    OHC_OP_IRIN_CAPTURED         = 0x97,
    OHC_OP_UART_SET_CONTROL      = 0xA0,
    OHC_OP_UART_SEND             = 0xA1,
    OHC_OP_UART_RECEIVE          = 0xA2,
    OHC_OP_UART_READY_FOR_DATA   = 0xA4,
    OHC_OP_AUTO_BAUD_GET         = 0xD2,
    OHC_OP_AUTO_BAUD_RESP        = 0xD7,
};

typedef struct {
    uint8_t  opcode;
    uint8_t  seq;
    uint8_t  flags;
    uint16_t length;
    uint8_t  payload[OHC_MAX_PAYLOAD];
} ohc_frame;

/* Incremental receiver. Feed it bytes as they arrive; it calls back once per
 * complete, checksum-valid frame. Malformed input resynchronises rather than
 * wedging — a UART sees line noise and half-frames on every reset. */
typedef void (*ohc_frame_cb)(const ohc_frame *frame, void *user);

typedef enum {
    OHC_RX_IDLE = 0,   /* hunting for DLE */
    OHC_RX_STX,        /* saw DLE, expecting STX */
    OHC_RX_BODY,       /* accumulating body, un-stuffing as we go */
    OHC_RX_BODY_DLE,   /* saw DLE inside body, expecting the doubled DLE */
} ohc_rx_state;

typedef struct {
    ohc_rx_state state;
    uint8_t      body[OHC_MAX_PAYLOAD + 8];
    uint16_t     body_len;
    uint16_t     need;          /* total body bytes for the current frame */
    ohc_frame_cb cb;
    void        *user;
    uint32_t     bad_checksums; /* diagnostics, cheap and worth having */
    uint32_t     resyncs;
} ohc_rx;

void    ohc_rx_init(ohc_rx *rx, ohc_frame_cb cb, void *user);
void    ohc_rx_feed(ohc_rx *rx, const uint8_t *data, size_t len);

/* Checksum over the un-stuffed body (opcode..last payload byte). */
uint8_t ohc_checksum(const uint8_t *body, size_t len);

/* Encode a frame, applying DLE stuffing. Returns bytes written, or 0 if `out`
 * is too small. Worst case is 2 + 2*(5 + length + 1). */
size_t  ohc_encode(uint8_t opcode, uint8_t seq, uint8_t flags,
                   const uint8_t *payload, uint16_t length,
                   uint8_t *out, size_t out_cap);

/* Convenience: encode a reply to `req` (opcode+1, response flag set). */
size_t  ohc_encode_reply(const ohc_frame *req, const uint8_t *payload,
                         uint16_t length, uint8_t *out, size_t out_cap);

#endif /* OHC_PROTO_H */
