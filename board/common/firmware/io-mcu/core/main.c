/* SPDX-License-Identifier: MIT */
#include "app.h"
#include "hal.h"

int main(void)
{
    hal_init();
    app_init();
    for (;;)
        app_poll();
}
