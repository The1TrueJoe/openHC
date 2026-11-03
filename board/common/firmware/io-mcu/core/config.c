/* SPDX-License-Identifier: MIT */
#include "config.h"

#define MAX_INDEX_GPIO 32u   /* CONTACT_GET is a u32 mask */

/* A row of this pin offering `want`; on `unit` unless that is 0xff. */
static const pin_func *find(const part_desc *part, uint8_t port, uint8_t pin,
                            uint8_t want, uint8_t unit)
{
    for (uint8_t i = 0; i < part->n_funcs; i++) {
        const pin_func *f = &part->funcs[i];

        if (f->port == port && f->pin == pin && (f->funcs & want) &&
            (unit == 0xff || f->unit == unit))
            return f;
    }
    return 0;
}

static uint8_t allowed_flags(uint8_t kind)
{
    switch (kind) {
    case OHC_CFG_RELAY:   return OHC_CFG_F_ACTIVE_LOW;
    case OHC_CFG_CONTACT: return OHC_CFG_F_ACTIVE_LOW | OHC_CFG_F_PULL_UP | OHC_CFG_F_PULL_DOWN;
    case OHC_CFG_IR_IN:   return OHC_CFG_F_ACTIVE_LOW | OHC_CFG_F_DEMODULATED;
    case OHC_CFG_IR_GATE: return OHC_CFG_F_ACTIVE_LOW | OHC_CFG_F_CARRIER(3);
    case OHC_CFG_HOLD:    return OHC_CFG_F_HIGH;
    default:              return 0;
    }
}

static uint8_t index_limit(const part_desc *part, uint8_t kind)
{
    switch (kind) {
    case OHC_CFG_IR_OUT:  return OHC_IR_MAX_OUTPUTS;
    case OHC_CFG_IR_IN:   return 1;
    case OHC_CFG_RELAY:
    case OHC_CFG_CONTACT: return MAX_INDEX_GPIO;
    case OHC_CFG_UART_RX:
    case OHC_CFG_UART_TX:
    case OHC_CFG_UART_MUX_A:
    case OHC_CFG_UART_MUX_B: return part->user_uarts;
    case OHC_CFG_IR_CARRIER: return OHC_CFG_CARRIERS;
    case OHC_CFG_IR_GATE:    return OHC_IR_MAX_OUTPUTS;
    case OHC_CFG_HOLD:       return 32;
    case OHC_CFG_HEARTBEAT:  return 1;
    default:              return 0;
    }
}

/* Claims, so a later entry cannot reuse a pin, a timer half or a UART. */
struct claims {
    uint8_t pins[8];
    uint32_t units;             /* carrier/capture units */
    uint32_t uarts;             /* UARTs, by number */
    uint32_t idx[OHC_CFG_UART_MUX_B + 1];   /* index used, per kind */
    uint32_t gates[OHC_CFG_CARRIERS];       /* gate bits used, per carrier */
};

static uint8_t check(const part_desc *part, cfg_entry *e, struct claims *c)
{
    uint8_t bit = (uint8_t)(1u << (e->pin & 7u));
    uint8_t want = 0;

    if (e->kind < OHC_CFG_IR_OUT || e->kind > OHC_CFG_UART_MUX_B)
        return OHC_CFG_S_KIND;
    if (e->index >= index_limit(part, e->kind))
        return OHC_CFG_S_INDEX;
    if (e->port > 7 || e->pin > 7 || !(part->bonded[e->port] & bit))
        return OHC_CFG_S_PIN;
    if (part->reserved[e->port] & bit)
        return OHC_CFG_S_RESERVED;
    if (e->flags & ~allowed_flags(e->kind))
        return OHC_CFG_S_FLAGS;
    if ((e->flags & OHC_CFG_F_PULL_UP) && (e->flags & OHC_CFG_F_PULL_DOWN))
        return OHC_CFG_S_FLAGS;

    switch (e->kind) {
    case OHC_CFG_IR_OUT:
    case OHC_CFG_IR_CARRIER: want = PF_CARRIER; break;
    case OHC_CFG_IR_IN:   want = part->capture_any_gpio ? 0 : PF_CAPTURE; break;
    case OHC_CFG_UART_RX: want = PF_UART_RX; break;
    case OHC_CFG_UART_TX: want = PF_UART_TX; break;
    }
    e->func = 0;
    if (want) {
        e->func = find(part, e->port, e->pin, want, 0xff);
        if (!e->func)
            return OHC_CFG_S_FUNC;
    }

    if (e->kind == OHC_CFG_IR_GATE) {
        /* a jack may have a gate per carrier, not two for the same one */
        uint32_t *g = &c->gates[OHC_CFG_F_CARRIER_OF(e->flags)];

        if (*g & (1u << e->index))
            return OHC_CFG_S_INDEX;
        if (c->pins[e->port] & bit)
            return OHC_CFG_S_BUSY;
        *g |= 1u << e->index;
        c->pins[e->port] |= bit;
        return OHC_CFG_S_OK;
    }
    if (c->idx[e->kind] & (1u << e->index))
        return OHC_CFG_S_INDEX;
    if (c->pins[e->port] & bit)
        return OHC_CFG_S_BUSY;
    if (want & (PF_CARRIER | PF_CAPTURE)) {
        if (c->units & (1u << e->func->unit))
            return OHC_CFG_S_BUSY;
        c->units |= 1u << e->func->unit;
    }
    c->idx[e->kind] |= 1u << e->index;
    c->pins[e->port] |= bit;
    return OHC_CFG_S_OK;
}

/*
 * A UART port needs one good RX and one good TX on the same UART. A pin can
 * offer more than one UART (PC4 is U4RX or U1RX on the TM4C), so pick the
 * first UART both pins offer that nothing else has.
 */
static void pair_uarts(const part_desc *part, cfg_table *t, struct claims *c)
{
    for (uint8_t i = 0; i < t->count; i++) {
        cfg_entry *rx = &t->e[i], *tx = 0;
        const pin_func *a = 0, *b = 0;

        if (rx->kind != OHC_CFG_UART_RX || rx->status != OHC_CFG_S_OK)
            continue;
        for (uint8_t j = 0; j < t->count; j++)
            if (t->e[j].kind == OHC_CFG_UART_TX && t->e[j].index == rx->index &&
                t->e[j].status == OHC_CFG_S_OK)
                tx = &t->e[j];
        /* in table order, so a pin's primary function wins */
        for (uint8_t k = 0; tx && k < part->n_funcs && !b; k++) {
            a = &part->funcs[k];
            if (a->port != rx->port || a->pin != rx->pin || !(a->funcs & PF_UART_RX) ||
                (c->uarts & (1u << a->unit)))
                continue;
            b = find(part, tx->port, tx->pin, PF_UART_TX, a->unit);
        }
        if (!b) {
            rx->status = OHC_CFG_S_PAIR;
            if (tx)
                tx->status = OHC_CFG_S_PAIR;
            continue;
        }
        rx->func = a;
        tx->func = b;
        c->uarts |= 1u << a->unit;
    }
    for (uint8_t i = 0; i < t->count; i++) {
        cfg_entry *tx = &t->e[i];
        bool paired = false;

        if (tx->kind != OHC_CFG_UART_TX || tx->status != OHC_CFG_S_OK)
            continue;
        for (uint8_t j = 0; j < t->count; j++)
            if (t->e[j].kind == OHC_CFG_UART_RX && t->e[j].index == tx->index &&
                t->e[j].status == OHC_CFG_S_OK)
                paired = true;
        if (!paired)
            tx->status = OHC_CFG_S_PAIR;
    }
}

uint8_t cfg_parse(const part_desc *part, const uint8_t *p, uint16_t len, cfg_table *t)
{
    struct claims c = { { 0 }, 0, 0, { 0 }, { 0 } };

    t->count = 0;
    if (len < OHC_CFG_HDR)
        return OHC_CFG_R_LENGTH;
    if (p[0] != OHC_CFG_VERSION)
        return OHC_CFG_R_VERSION;
    if (p[1] > OHC_CFG_MAX_ENTRIES || len != OHC_CFG_HDR + p[1] * OHC_CFG_ENTRY)
        return OHC_CFG_R_LENGTH;

    t->count = p[1];
    for (uint8_t i = 0; i < t->count; i++) {
        const uint8_t *r = &p[OHC_CFG_HDR + i * OHC_CFG_ENTRY];
        cfg_entry *e = &t->e[i];

        e->kind = r[0];
        e->index = r[1];
        e->port = r[2];
        e->pin = r[3];
        e->flags = r[4];
        e->func = 0;
        e->status = check(part, e, &c);
    }
    pair_uarts(part, t, &c);
    /* a gate routes a carrier: without that carrier it routes nothing */
    for (uint8_t i = 0; i < t->count; i++) {
        cfg_entry *g = &t->e[i];
        bool found = false;

        if (g->kind != OHC_CFG_IR_GATE || g->status != OHC_CFG_S_OK)
            continue;
        for (uint8_t j = 0; j < t->count; j++)
            if (t->e[j].kind == OHC_CFG_IR_CARRIER && t->e[j].status == OHC_CFG_S_OK &&
                t->e[j].index == OHC_CFG_F_CARRIER_OF(g->flags))
                found = true;
        if (!found)
            g->status = OHC_CFG_S_PAIR;
    }
    return OHC_CFG_R_OK;
}

uint16_t cfg_reply(const cfg_table *t, uint8_t result, uint8_t *out)
{
    out[0] = OHC_CFG_VERSION;
    out[1] = result;
    out[2] = t->count;
    for (uint8_t i = 0; i < t->count; i++)
        out[3 + i] = t->e[i].status;
    return (uint16_t)(3u + t->count);
}

bool cfg_same(const cfg_table *a, const cfg_table *b)
{
    if (a->count != b->count)
        return false;
    for (uint8_t i = 0; i < a->count; i++) {
        const cfg_entry *x = &a->e[i], *y = &b->e[i];

        if (x->kind != y->kind || x->index != y->index || x->port != y->port ||
            x->pin != y->pin || x->flags != y->flags || x->status != y->status)
            return false;
    }
    return true;
}
