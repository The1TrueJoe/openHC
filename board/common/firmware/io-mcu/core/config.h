/* SPDX-License-Identifier: MIT */
/* The config frame: parse it and check every entry against the part. */
#ifndef OHC_CONFIG_H
#define OHC_CONFIG_H

#include <stdbool.h>
#include <stdint.h>

#include "ohc-iomcu-proto.h"
#include "part.h"

typedef struct {
    uint8_t kind, index, port, pin, flags;
    uint8_t status;             /* OHC_CFG_S_* */
    const pin_func *func;       /* the row it uses, for carrier/capture/UART */
} cfg_entry;

typedef struct {
    uint8_t count;
    cfg_entry e[OHC_CFG_MAX_ENTRIES];
} cfg_table;

/* OHC_CFG_R_*; on OK every entry carries its own status. */
uint8_t cfg_parse(const part_desc *part, const uint8_t *payload, uint16_t len, cfg_table *out);

/* The reply payload: [version, result, count, statuses]. Returns its length. */
uint16_t cfg_reply(const cfg_table *t, uint8_t result, uint8_t *out);

/* Same entries, same pins, same flags: nothing to re-apply. */
bool cfg_same(const cfg_table *a, const cfg_table *b);

#endif
