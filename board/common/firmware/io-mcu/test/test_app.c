/* SPDX-License-Identifier: MIT */
/* The application end to end against the fake HAL: frames in, frames and pins out. */
#include "app.h"
#include "hal.h"
#include "ir.h"
#include "test.h"
#include "vectors.h"

void cfg_ea3(void);
const uint8_t *cfg_frame(uint16_t *len);

static ohc_frame f[16];
static uint8_t fp[16][OHC_MAX_PAYLOAD];

static void req(uint8_t op, uint8_t seq, const uint8_t *p, uint16_t n)
{
    uint8_t b[2 * OHC_MAX_PAYLOAD + 16];

    fake_send(b, ohc_encode(op, seq, 0, p, n, b, sizeof(b)));
}

/* send, run the main loop, return how many frames came back */
static int ask(uint8_t op, uint8_t seq, const uint8_t *p, uint16_t n)
{
    req(op, seq, p, n);
    app_poll();
    return fake_frames(f, fp, 16);
}

static int poll(void)
{
    app_poll();
    return fake_frames(f, fp, 16);
}

static void boot(const part_desc *part)
{
    fake_reset(part);
    app_init();
}

static void configure_ea3(void)
{
    uint16_t n;
    const uint8_t *p;

    cfg_ea3();
    p = cfg_frame(&n);
    CHECK_EQ(ask(OHC_OP_CONFIG_SET, 0x40, p, n), 1);
    CHECK_EQ(f[0].opcode, OHC_OP_CONFIG_RESP);
    CHECK_EQ(f[0].seq, 0x40);
    CHECK_EQ(f[0].flags, OHC_FLAG_RESPONSE);
    CHECK_EQ(f[0].payload[1], OHC_CFG_R_OK);
    CHECK_EQ(f[0].payload[2], 23);
    for (int i = 0; i < 23; i++)
        CHECK_EQ(f[0].payload[3 + i], OHC_CFG_S_OK);
}

static void identity(void)
{
    uint8_t knock[] = { 0x55, 0x55 };
    static const uint8_t ab_ea[] = { 0x10, 0x02, 0xd7, 0x00, 0x02, 0x00, 0x04,
                                     0x00, 0x07, 0x08, 0x00, 0x14 };

    boot(&part_tm4c1231d5);

    /* the knock: AUTO_BAUD_RESP with the EA's 460800, as stock answers it */
    fake_send(knock, 2);
    app_poll();
    CHECK_EQ(fk.out_len, sizeof(ab_ea));
    CHECK_MEM(fk.out, ab_ea, sizeof(ab_ea));
    fk.out_len = 0;

    /* a captured stock request gets a well-formed reply with our version */
    fake_send(V(hc_fwv_req));
    CHECK_EQ(poll(), 1);
    CHECK_EQ(f[0].opcode, 0x35);
    CHECK_EQ(f[0].seq, 0x01);
    CHECK_EQ(f[0].flags, OHC_FLAG_RESPONSE);
    CHECK_EQ(f[0].length, strlen("openhc-test"));
    CHECK_MEM(f[0].payload, "openhc-test", f[0].length);

    fake_send(V(ea_pn_req));
    CHECK_EQ(poll(), 1);
    CHECK_EQ(f[0].opcode, 0x25);
    CHECK_MEM(f[0].payload, "openhc:io-mcu:tm4c1231d5 clk=50000000", f[0].length);

    /* a 0x55 inside a frame is data, not half a knock */
    {
        uint8_t i = 0x55;

        CHECK_EQ(ask(OHC_OP_RELAY_GET, 0x55, &i, 1), 1);
        CHECK_EQ(f[0].opcode, OHC_OP_RELAY_STATE);
        CHECK_EQ(f[0].seq, 0x55);
        CHECK_EQ(f[0].payload[0], 0x55);
    }

    /* unknown opcodes are dropped, as stock drops them */
    CHECK_EQ(ask(0x94, 1, 0, 0), 0);
    CHECK_EQ(ask(0x12, 1, 0, 0), 0);

    /* the HC-800's part reports 115200 */
    boot(&part_lm3s1162);
    fake_send(V(hc_ab_req));
    CHECK_EQ(poll(), 1);
    CHECK_EQ(f[0].opcode, 0xd7);
    CHECK_EQ(f[0].seq, 0x13);
    CHECK_EQ((f[0].payload[1] << 16) | (f[0].payload[2] << 8) | f[0].payload[3], 115200);
}

static bool any_pin_used(void)
{
    for (int p = 0; p < 8; p++)
        for (int n = 0; n < 8; n++)
            if (fk.pin[p][n].mode != PIN_UNUSED)
                return true;
    return false;
}

static void unconfigured(void)
{
    uint8_t i = 0;

    boot(&part_tm4c1231d5);
    /* nothing is driven until the host says what the pins are */
    fake_send(V(ea1_irsend));
    CHECK_EQ(poll(), 1);
    CHECK_EQ(f[0].opcode, OHC_OP_IROUT_STATUS);
    CHECK_EQ(f[0].payload[0], OHC_IR_ST_REJECTED);
    CHECK_EQ(ask(OHC_OP_RELAY_TOGGLE, 2, &i, 1), 1);
    CHECK_EQ(f[0].payload[1], 0);
    CHECK_EQ(ask(OHC_OP_CONTACT_GET, 3, 0, 0), 1);
    CHECK_EQ(f[0].length, 4);
    CHECK_EQ(f[0].payload[0], 0);
    CHECK(!any_pin_used());
    CHECK(!fk.burst_armed);
}

static void config_applies(void)
{
    boot(&part_tm4c1231d5);
    configure_ea3();
    CHECK_EQ(fk.pin[1][6].mode, PIN_OUT);       /* IR 0: PB6, idle low */
    CHECK_EQ(fk.pin[1][6].level, 0);
    CHECK(!fk.pin[1][6].gated);
    CHECK_EQ(fk.pin[0][2].mode, PIN_OUT);       /* relay: PA2, off */
    CHECK_EQ(fk.pin[0][2].level, 0);
    CHECK_EQ(fk.pin[5][0].mode, PIN_IN);        /* contact: PF0, pulled up */
    CHECK_EQ(fk.pin[5][0].pull, OHC_CFG_F_PULL_UP);
    CHECK_EQ(fk.pin[4][5].mode, PIN_IN);        /* UART pins wait as inputs */
    CHECK_EQ(fk.pin[4][2].mode, PIN_OUT);       /* combo line A: IR */
    CHECK_EQ(fk.pin[4][2].level, 0);
    CHECK_EQ(fk.pin[2][7].mode, PIN_OUT);       /* heartbeat */
    CHECK(!fk.uart_on[0]);

    /* a NAKed entry touches nothing */
    {
        static const uint8_t bad[] = {
            OHC_CFG_VERSION, 2,
            OHC_CFG_IR_OUT, 0, 0, 2, 0,     /* PA2: no timer */
            OHC_CFG_RELAY, 0, 5, 1, 0,      /* PF1 */
        };

        CHECK_EQ(ask(OHC_OP_CONFIG_SET, 1, bad, sizeof(bad)), 1);
        CHECK_EQ(f[0].payload[3], OHC_CFG_S_FUNC);
        CHECK_EQ(f[0].payload[4], OHC_CFG_S_OK);
        CHECK_EQ(fk.pin[0][2].mode, PIN_UNUSED);
        CHECK_EQ(fk.pin[1][6].mode, PIN_UNUSED);    /* the old table is gone */
        CHECK_EQ(fk.pin[5][1].mode, PIN_OUT);
    }
    /* a frame it cannot read changes nothing */
    {
        static const uint8_t v2[] = { 2, 0 };

        CHECK_EQ(ask(OHC_OP_CONFIG_SET, 2, v2, 2), 1);
        CHECK_EQ(f[0].payload[1], OHC_CFG_R_VERSION);
        CHECK_EQ(fk.pin[5][1].mode, PIN_OUT);
    }
}

static void relays_contacts(void)
{
    uint8_t i = 0;

    boot(&part_tm4c1231d5);
    configure_ea3();
    CHECK_EQ(ask(OHC_OP_RELAY_TOGGLE, 5, &i, 1), 1);
    CHECK_EQ(f[0].opcode, OHC_OP_RELAY_STATE);
    CHECK_EQ(f[0].payload[0], 0);
    CHECK_EQ(f[0].payload[1], 1);
    CHECK_EQ(fk.pin[0][2].level, 1);
    CHECK_EQ(ask(OHC_OP_RELAY_GET, 6, &i, 1), 1);
    CHECK_EQ(f[0].payload[1], 1);

    /* the same table again (a driver rebind) leaves the relay closed */
    configure_ea3();
    CHECK_EQ(fk.pin[0][2].level, 1);
    CHECK_EQ(ask(OHC_OP_RELAY_GET, 7, &i, 1), 1);
    CHECK_EQ(f[0].payload[1], 1);
    CHECK_EQ(ask(OHC_OP_RELAY_TOGGLE, 8, &i, 1), 1);
    CHECK_EQ(f[0].payload[1], 0);
    CHECK_EQ(fk.pin[0][2].level, 0);

    /* an index with no relay echoes back with 0 */
    i = 3;
    CHECK_EQ(ask(OHC_OP_RELAY_TOGGLE, 9, &i, 1), 1);
    CHECK_EQ(f[0].payload[0], 3);
    CHECK_EQ(f[0].payload[1], 0);

    /* contact 0 is active-low: the pin pulled to ground reads closed, LE bit 0 */
    fk.pin[5][0].level = 1;
    CHECK_EQ(ask(OHC_OP_CONTACT_GET, 10, 0, 0), 1);
    CHECK_EQ(f[0].opcode, OHC_OP_CONTACT_STATE);
    CHECK_EQ(f[0].payload[0], 0);
    fk.pin[5][0].level = 0;
    CHECK_EQ(ask(OHC_OP_CONTACT_GET, 11, 0, 0), 1);
    CHECK_EQ(f[0].payload[0], 1);
    CHECK_EQ(f[0].payload[3], 0);
}

static void transmit(void)
{
    static const uint16_t want[] = { 0x157, 0xac, 0x16, 0x16 };
    uint8_t busy[64];

    boot(&part_tm4c1231d5);
    configure_ea3();
    req(OHC_OP_IRIN_SET_CAPTURE, 1, 0, 0);
    poll();
    CHECK(fk.capture_on);

    /* the captured frame that lit an EA1's jack 1, answered byte for byte as stock did */
    fake_send(V(ea1_irsend));
    app_poll();
    CHECK_EQ(fk.out_len, sizeof(ea1_irstatus));
    CHECK_MEM(fk.out, ea1_irstatus, sizeof(ea1_irstatus));
    fk.out_len = 0;
    CHECK_EQ(fk.carrier_period[0], 1315);      /* T0A at 38 kHz */
    CHECK(!fk.capture_on);                     /* not hearing ourselves */

    for (int i = 0; i < 4; i++) {
        CHECK(fk.burst_armed);
        CHECK_EQ(fk.burst_ticks, want[i] * 1315u);
        CHECK_EQ(fk.pin[1][6].gated, i % 2 == 0);
        if (i == 1) {
            /* a second code while the first is going out */
            memcpy(busy, ea1_irsend, sizeof(ea1_irsend));
            busy[3] = 0x12;
            busy[sizeof(ea1_irsend) - 1] = (uint8_t)(busy[sizeof(ea1_irsend) - 1] - 1);
            fake_send(busy, sizeof(ea1_irsend));
            CHECK_EQ(poll(), 1);
            CHECK_EQ(f[0].seq, 0x12);
            CHECK_EQ(f[0].payload[0], OHC_IR_ST_BUSY);
        }
        app_burst_isr();
    }
    CHECK(!fk.burst_armed);
    CHECK(!fk.pin[1][6].gated);
    CHECK_EQ(fk.carrier_period[0], 0);
    CHECK_EQ(poll(), 1);
    CHECK_EQ(f[0].opcode, OHC_OP_IROUT_STATUS);
    CHECK_EQ(f[0].seq, 0x11);
    CHECK_EQ(f[0].flags, 0);
    CHECK_EQ(f[0].payload[0], OHC_IR_ST_DONE);
    CHECK(fk.capture_on);

    /* forever, until IROUT_STOP */
    {
        uint8_t p[OHC_IR_FIXED + 4] = { 0xff, 0, 0, 0x41, 0, 0, 0, 0, 0, 0x6d, 0, 1, 0, 0, 0, 10, 0, 10 };

        CHECK_EQ(ask(OHC_OP_IROUT_SEND, 0x30, p, sizeof(p)), 1);
        CHECK_EQ(f[0].payload[0], OHC_IR_ST_OK);
        for (int i = 0; i < 100; i++)
            app_burst_isr();
        CHECK(fk.burst_armed);
        CHECK(fk.pin[1][6].gated);
        CHECK(fk.pin[3][4].gated);              /* channels 0 and 6 together */
        CHECK_EQ(ask(OHC_OP_IROUT_STOP, 0x31, 0, 0), 0);
        app_burst_isr();
        CHECK(!fk.burst_armed);
        CHECK_EQ(poll(), 1);
        CHECK_EQ(f[0].seq, 0x30);
        CHECK_EQ(f[0].payload[0], OHC_IR_ST_STOPPED);
    }

    /* the carrier word that hard-faults stock is refused */
    {
        uint8_t p[OHC_IR_FIXED + 4] = { 1, 0, 0, 1 };

        CHECK_EQ(ask(OHC_OP_IROUT_SEND, 0x32, p, sizeof(p)), 1);
        CHECK_EQ(f[0].payload[0], OHC_IR_ST_REJECTED);
        CHECK(!fk.burst_armed);
    }

    /* reconfiguring mid-code stops it and turns everything off */
    fake_send(V(ea1_irsend));
    poll();
    CHECK(fk.burst_armed);
    {
        static const uint8_t none[] = { OHC_CFG_VERSION, 0 };

        CHECK_EQ(ask(OHC_OP_CONFIG_SET, 3, none, 2), 1);
        CHECK(!fk.burst_armed);
        CHECK(!any_pin_used());
    }
}

static void receive(void)
{
    static uint16_t code[67];
    uint32_t t = 1000, p = nec_carrier;
    int n;

    memcpy(code, nec_head, sizeof(nec_head));
    for (int i = 8; i < 66; i += 2) {
        code[i] = 0x8016;
        code[i + 1] = (i / 2) % 2 ? 0x0016 : 0x003f;
    }
    code[66] = 0x8016;

    boot(&part_tm4c1231d5);
    configure_ea3();
    /* configured but not asked for: edges go nowhere */
    app_capture_edge(5, true);
    fk.now = 1000000;
    CHECK_EQ(poll(), 0);

    req(OHC_OP_IRIN_SET_CAPTURE, 1, 0, 0);
    poll();
    for (int i = 0; i < 67; i++) {
        uint32_t len = code[i] & OHC_IRIN_DURATION;

        if (code[i] & OHC_IRIN_MARK)
            for (uint32_t k = 0; k < len; k++)
                app_capture_edge(t + k * p, false);
        t += len * p;
    }
    fk.now = t + 1000;
    CHECK_EQ(poll(), 0);    /* not over yet */
    fk.now = t + 50000000u / 1000000u * IR_RX_IDLE_US + 1;
    n = poll();
    CHECK_EQ(n, 1);
    CHECK_EQ(f[0].opcode, OHC_OP_IRIN_CAPTURED);
    CHECK_EQ(f[0].length, 2 + 67 * 2);
    CHECK_EQ((f[0].payload[0] << 8) | f[0].payload[1], nec_carrier);
    for (int i = 0; i < 67; i++)
        CHECK_EQ((f[0].payload[2 + 2 * i] << 8) | f[0].payload[3 + 2 * i], code[i]);

    /* on a part clocked at 80 MHz the period still goes out in 50 MHz ticks */
    boot(&part_tm4c1231d5);
    fk.clock = 80000000u;
    configure_ea3();
    req(OHC_OP_IRIN_SET_CAPTURE, 1, 0, 0);
    poll();
    t = 0;
    p = 80000000u / 38000u;
    for (int i = 0; i < 67; i++) {
        uint32_t len = code[i] & OHC_IRIN_DURATION;

        if (code[i] & OHC_IRIN_MARK)
            for (uint32_t k = 0; k < len; k++)
                app_capture_edge(t + k * p, false);
        t += len * p;
    }
    fk.now = t + 80000000u / 1000000u * IR_RX_IDLE_US + 1;
    CHECK_EQ(poll(), 1);
    CHECK_EQ((f[0].payload[0] << 8) | f[0].payload[1], (p * 50000u + 40000u) / 80000u);
}

static void serial(void)
{
    static const uint8_t ctl[] = { 0, 1, 3, 8, 0, 1 };     /* port 0, 9600 8N1 */
    static const uint8_t at[] = { 0, 'A', 'T', '\r' };

    boot(&part_tm4c1231d5);
    configure_ea3();
    /* sending before it is switched on goes nowhere */
    CHECK_EQ(ask(OHC_OP_UART_SEND, 1, at, sizeof(at)), 0);
    CHECK_EQ(fk.uart_tx_len[0], 0);

    CHECK_EQ(ask(OHC_OP_UART_SET_CONTROL, 2, ctl, sizeof(ctl)), 0);    /* no reply */
    CHECK(fk.uart_on[0]);
    CHECK_EQ(fk.uart_baud[0], 9600);
    CHECK_EQ(ask(OHC_OP_UART_SEND, 3, at, sizeof(at)), 0);
    CHECK_EQ(fk.uart_tx_len[0], 3);
    CHECK_MEM(fk.uart_tx[0], "AT\r", 3);

    /* a full FIFO holds the rest for later */
    fk.uart_fifo_room = 1;
    ask(OHC_OP_UART_SEND, 4, at, sizeof(at));
    CHECK_EQ(fk.uart_tx_len[0], 4);
    fk.uart_fifo_room = -1;
    poll();
    CHECK_EQ(fk.uart_tx_len[0], 6);

    /* the loopback: bytes in come back as UART_RECEIVE once the line pauses */
    fk.millis = 100;
    app_uart_rx(0, 'A');
    app_uart_rx(0, 'T');
    app_uart_rx(0, '\r');
    CHECK_EQ(poll(), 0);
    fk.millis = 103;
    CHECK_EQ(poll(), 1);
    CHECK_EQ(f[0].opcode, OHC_OP_UART_RECEIVE);
    CHECK_EQ(f[0].length, 4);
    CHECK_MEM(f[0].payload, at, 4);

    /* a busy line goes out in batches without waiting */
    for (int i = 0; i < 100; i++)
        app_uart_rx(0, (uint8_t)i);
    CHECK_EQ(poll(), 2);
    CHECK_EQ(f[0].length, 49);
    CHECK_EQ(f[1].length, 49);

    /* baud index 7 and up is 115200; port 1 is UART7 */
    {
        static const uint8_t c1[] = { 1, 1, 9, 8, 0, 1 };

        ask(OHC_OP_UART_SET_CONTROL, 5, c1, sizeof(c1));
        CHECK_EQ(fk.uart_baud[1], 115200);
    }
    /* enable 0 switches it off */
    {
        static const uint8_t off[] = { 0, 0, 3, 8, 0, 1 };

        ask(OHC_OP_UART_SET_CONTROL, 6, off, sizeof(off));
        CHECK(!fk.uart_on[0]);
    }
}

void cfg_hc800(void);

/* The HC-800: one shared carrier, gated onto the jack by an active-low line. */
static void transmit_gated(void)
{
    uint16_t n;
    const uint8_t *p;
    uint8_t ir[OHC_IR_FIXED + 4] = { 1, 0, 0, 0x01, 0, 0, 0, 0, 0, 0x6d, 0, 0, 0, 0, 0, 20, 0, 10 };

    boot(&part_lm3s1162);
    cfg_hc800();
    p = cfg_frame(&n);
    CHECK_EQ(ask(OHC_OP_CONFIG_SET, 1, p, n), 1);
    CHECK_EQ(f[0].payload[1], OHC_CFG_R_OK);
    CHECK_EQ(fk.pin[2][7].level, 1);    /* jack 1's gates closed (high) */
    CHECK_EQ(fk.pin[2][6].level, 1);
    CHECK_EQ(fk.pin[5][4].level, 0);    /* its C line held low */
    CHECK_EQ(fk.pin[3][1].mode, PIN_OUT);   /* carrier 0, PD1, idle low */

    CHECK_EQ(ask(OHC_OP_IROUT_SEND, 2, ir, sizeof(ir)), 1);
    CHECK_EQ(f[0].payload[0], OHC_IR_ST_OK);
    CHECK_EQ(fk.pin[2][7].level, 0);    /* carrier 0's gate open for jack 1 */
    CHECK_EQ(fk.pin[2][6].level, 1);    /* carrier 1's stays shut */
    CHECK_EQ(fk.pin[2][5].level, 1);    /* jack 2 untouched */
    CHECK_EQ(fk.carrier_period[1], 1315);   /* PWM1 */
    CHECK(fk.pin[3][1].gated);
    app_burst_isr();
    CHECK(!fk.pin[3][1].gated);
    app_burst_isr();
    CHECK(!fk.burst_armed);
    CHECK_EQ(fk.pin[2][7].level, 1);    /* closed again */
    CHECK_EQ(fk.carrier_period[1], 0);
    CHECK_EQ(poll(), 1);
    CHECK_EQ(f[0].payload[0], OHC_IR_ST_DONE);

    /* relays on PA5..PA2; contacts read closed low */
    {
        uint8_t i = 3;

        CHECK_EQ(ask(OHC_OP_RELAY_TOGGLE, 3, &i, 1), 1);
        CHECK_EQ(fk.pin[0][2].level, 1);
        fk.pin[1][0].level = 1;
        fk.pin[1][1].level = 1;
        fk.pin[1][2].level = 1;
        fk.pin[1][3].level = 0;
        CHECK_EQ(ask(OHC_OP_CONTACT_GET, 4, 0, 0), 1);
        CHECK_EQ(f[0].payload[0], 0x08);    /* contact 4, as a live HC-800 answers */
    }
}

static void combo_and_heartbeat(void)
{
    static const uint8_t on[] = { 0, 1, 3, 8, 0, 1 }, off[] = { 0, 0, 3, 8, 0, 1 };
    static const uint8_t swap[] = { 0, 1 };
    bool was;

    boot(&part_tm4c1231d5);
    configure_ea3();
    ask(OHC_OP_UART_SET_CONTROL, 1, on, sizeof(on));
    CHECK_EQ(fk.pin[4][2].level, 1);    /* serial: A high, B low */
    CHECK_EQ(fk.pin[1][1].level, 0);
    ask(OHC_OP_UART_MODE, 2, swap, sizeof(swap));
    CHECK_EQ(fk.pin[4][2].level, 0);    /* swapped: B high */
    CHECK_EQ(fk.pin[1][1].level, 1);
    ask(OHC_OP_UART_SET_CONTROL, 3, off, sizeof(off));
    CHECK_EQ(fk.pin[4][2].level, 0);    /* back to IR */
    CHECK_EQ(fk.pin[1][1].level, 0);

    was = fk.pin[2][7].level;
    fk.millis += 150;
    poll();
    CHECK(fk.pin[2][7].level != was);
    fk.millis += 50;
    poll();
    CHECK(fk.pin[2][7].level != was);   /* not yet again */
    fk.millis += 100;
    poll();
    CHECK(fk.pin[2][7].level == was);
}

static void reset(void)
{
    boot(&part_tm4c1231d5);
    configure_ea3();
    CHECK_EQ(ask(OHC_OP_RESET, 9, 0, 0), 1);
    CHECK_EQ(f[0].opcode, OHC_OP_RESET_ACK);
    CHECK_EQ(f[0].seq, 9);
    CHECK_EQ(fk.resets, 1);
    CHECK(!any_pin_used());
}

void test_app(void)
{
    identity();
    unconfigured();
    config_applies();
    relays_contacts();
    transmit();
    receive();
    serial();
    transmit_gated();
    combo_and_heartbeat();
    reset();
}
