/* ohc-ths8200 — bring up the TI THS8200 component video DAC over i2c-dev.
 *
 * The HC-800 has two video chips on SMBus i2c-6: a THS8200 component DAC at
 * 0x21 and an ADV7511 HDMI transmitter at 0x72. Only the THS8200 is ever
 * configured — Control4's own OS 3.x never touches the ADV7511 (see
 * board/hc800/video/ths8200-720p60.regs and the HC-800 page in the docs) — so
 * this is the board's proven route to a picture.
 *
 * The vendor does it in-kernel, in a GPL module (ths8200.ko exporting
 * ths8200_set_720P). We do it from userspace instead, for two reasons: the
 * register values are DATA we captured off working hardware rather than code we
 * would be copying, and a userspace tool can be re-pointed at a different mode
 * without rebuilding a kernel.
 *
 *   ohc-ths8200 [-b BUS] [-a ADDR] [-n] [FILE]
 *
 *     -b   i2c bus number      (default 6, the ICH7 SMBus on this board)
 *     -a   7-bit slave address (default 0x21)
 *     -n   dry run: parse and validate, touch no hardware
 *     FILE register file       (default /opt/ohc/video/ths8200-720p60.regs)
 *
 * Exit status is 0 only if every register was written.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include <linux/i2c-dev.h>

#define THS8200_VERSION_REG  0x02u
#define THS8200_VERSION_VAL  0x04u
#define MAX_REGS             256

static int i2c_write_reg(int fd, unsigned char reg, unsigned char val)
{
    unsigned char buf[2] = { reg, val };
    ssize_t n = write(fd, buf, 2);
    return (n == 2) ? 0 : -1;
}

static int i2c_read_reg(int fd, unsigned char reg, unsigned char *out)
{
    if (write(fd, &reg, 1) != 1) {
        return -1;
    }
    return (read(fd, out, 1) == 1) ? 0 : -1;
}

int main(int argc, char **argv)
{
    const char *path = "/opt/ohc/video/ths8200-720p60.regs";
    int bus = 6, addr = 0x21, dry = 0, opt;
    unsigned char regs[MAX_REGS], have[MAX_REGS];
    char dev[32];
    FILE *f;
    char line[256];
    int fd, count = 0, written = 0;
    unsigned char ver;

    while ((opt = getopt(argc, argv, "b:a:nh")) != -1) {
        switch (opt) {
        case 'b': bus = (int)strtol(optarg, NULL, 0); break;
        case 'a': addr = (int)strtol(optarg, NULL, 0); break;
        case 'n': dry = 1; break;
        default:
            fprintf(stderr, "usage: %s [-b BUS] [-a ADDR] [-n] [FILE]\n", argv[0]);
            return 2;
        }
    }
    if (optind < argc) {
        path = argv[optind];
    }

    memset(have, 0, sizeof have);

    f = fopen(path, "r");
    if (!f) {
        fprintf(stderr, "ohc-ths8200: %s: %s\n", path, strerror(errno));
        return 1;
    }
    while (fgets(line, sizeof line, f)) {
        unsigned int r, v;
        char *h = strchr(line, '#');
        if (h) {
            *h = '\0';                     /* strip the trailing comment */
        }
        if (sscanf(line, "%x %x", &r, &v) != 2) {
            continue;                      /* blank or comment-only line */
        }
        if (r >= MAX_REGS || v > 0xff) {
            fprintf(stderr, "ohc-ths8200: %s: bad entry %x %x\n", path, r, v);
            fclose(f);
            return 1;
        }
        regs[r] = (unsigned char)v;
        have[r] = 1;
        count++;
    }
    fclose(f);

    if (count == 0) {
        fprintf(stderr, "ohc-ths8200: %s contains no registers\n", path);
        return 1;
    }
    printf("ohc-ths8200: %d registers from %s\n", count, path);

    if (dry) {
        printf("ohc-ths8200: dry run, hardware untouched\n");
        return 0;
    }

    snprintf(dev, sizeof dev, "/dev/i2c-%d", bus);
    fd = open(dev, O_RDWR);
    if (fd < 0) {
        fprintf(stderr, "ohc-ths8200: %s: %s (CONFIG_I2C_CHARDEV?)\n",
                dev, strerror(errno));
        return 1;
    }
    if (ioctl(fd, I2C_SLAVE, addr) < 0) {
        fprintf(stderr, "ohc-ths8200: address 0x%02x: %s\n", addr, strerror(errno));
        close(fd);
        return 1;
    }

    /* Identify the part before writing to it. Register 0x02 is VERSION and
     * reads 0x04 on this board's THS8200 (confirmed on live hardware). Writing
     * a 138-register timing set into whatever else might answer at 0x21 is not
     * something to do on a hunch. */
    if (i2c_read_reg(fd, THS8200_VERSION_REG, &ver) < 0) {
        fprintf(stderr, "ohc-ths8200: no answer at 0x%02x on %s\n", addr, dev);
        close(fd);
        return 1;
    }
    if (ver != THS8200_VERSION_VAL) {
        fprintf(stderr, "ohc-ths8200: VERSION reads 0x%02x, expected 0x%02x — "
                        "not a THS8200, refusing to write\n", ver, THS8200_VERSION_VAL);
        close(fd);
        return 1;
    }
    printf("ohc-ths8200: THS8200 at %s:0x%02x, VERSION 0x%02x\n", dev, addr, ver);

    for (int r = 0; r < MAX_REGS; r++) {
        if (!have[r] || r == THS8200_VERSION_REG) {
            continue;                      /* VERSION is read-only */
        }
        if (i2c_write_reg(fd, (unsigned char)r, regs[r]) < 0) {
            fprintf(stderr, "ohc-ths8200: write 0x%02x=0x%02x failed: %s\n",
                    r, regs[r], strerror(errno));
            close(fd);
            return 1;
        }
        written++;
    }
    close(fd);
    printf("ohc-ths8200: wrote %d registers, output configured\n", written);
    return 0;
}
