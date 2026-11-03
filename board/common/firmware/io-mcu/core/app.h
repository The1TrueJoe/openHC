/* SPDX-License-Identifier: MIT */
/* The application: opcode dispatch and the IO state behind it. */
#ifndef OHC_APP_H
#define OHC_APP_H

void app_init(void);
void app_poll(void);    /* one turn of the main loop */

#endif
