/* SPDX-License-Identifier: MIT */
#include "test.h"

int t_failed, t_checks;

void t_hexdump(const char *tag, const uint8_t *p, size_t n)
{
    fprintf(stderr, "  %s:", tag);
    for (size_t i = 0; i < n; i++)
        fprintf(stderr, " %02x", p[i]);
    fprintf(stderr, "\n");
}

int main(void)
{
    test_proto();
    test_ir();
    test_config();
    test_app();
    printf("io-mcu host tests: %d checks, %d failed\n", t_checks, t_failed);
    return t_failed ? 1 : 0;
}
