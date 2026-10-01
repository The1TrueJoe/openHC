// SPDX-License-Identifier: MIT
/*
 * iomcu-attach — bring up the Control4 IO microcontroller and hand its UART to
 * the openHC line discipline.
 *
 * Two jobs, in order, on one fd:
 *
 *   1. BRING-UP. The IO microcontroller (TI Tiva TM4C on EA, Stellaris LM3S on
 *      HC800) powers up in its TI *serial bootloader* and does NOT auto-run the
 *      application. Until the application is running it answers nothing on the
 *      DLE/STX protocol, so the kernel driver's identify times out and no
 *      gpiochip / rc device is ever created. This was mistaken for dead silicon
 *      for a long time; it is not — the part is sitting in the bootloader
 *      waiting to be told to run. We tell it, with the plain TI bootloader
 *      sequence (autobaud 0x55 0x55 -> 0xCC ACK, PING, COMMAND_RUN @ 0x1000),
 *      then one more 0x55 0x55 so the freshly-started application autobauds onto
 *      our line rate. See docs: shared/io-mcu.md ("Bring-up sequence").
 *
 *   2. ATTACH. The kernel driver gpio-ohc-iomcu presents the relays, contacts
 *      and IR as standard kernel devices, but it has to be given a port first.
 *      On x86 there is no device tree or ACPI node describing a Control4
 *      co-processor, so serdev has nothing to enumerate and the attachment has
 *      to come from userspace: set the line speed, then TIOCSETD. util-linux's
 *      ldattach does the ioctl but drags in fifty other binaries; this is that
 *      ioctl, plus the bring-up that has to happen first.
 *
 * The bring-up runs BEFORE TIOCSETD on purpose: tty_set_ldisc() holds
 * tty->ldisc_sem across the discipline's open(), and a reply cannot be read
 * back through that same lock — so the handshake has to happen while the port
 * is still plain N_TTY and this process owns the byte stream.
 *
 * Bring-up is idempotent and best-effort: it checks for a running application
 * first and skips the handshake if one answers (a warm reboot that did not
 * reset the part, or a board whose MCU auto-runs), and if the handshake fails
 * it still attaches — a driver that reports "no reply" is a better diagnosis
 * than a tool that refused to attach.
 *
 * The process must stay alive after TIOCSETD: a line discipline lasts only as
 * long as the fd that set it is open. Closing this would drop the gpiochip.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

static speed_t to_speed(long baud)
{
	switch (baud) {
	case 1200:   return B1200;
	case 2400:   return B2400;
	case 4800:   return B4800;
	case 9600:   return B9600;
	case 19200:  return B19200;
	case 38400:  return B38400;
	case 57600:  return B57600;
	case 115200: return B115200;
	case 230400: return B230400;
	/* The EA family's IO microcontroller runs here, not at 115200. */
	case 460800: return B460800;
	case 921600: return B921600;
	default:     return 0;
	}
}

/* Write every byte, retrying short writes. */
static int write_all(int fd, const unsigned char *b, size_t n)
{
	size_t off = 0;
	while (off < n) {
		ssize_t w = write(fd, b + off, n - off);
		if (w < 0) {
			if (errno == EINTR)
				continue;
			return -1;
		}
		off += (size_t)w;
	}
	return 0;
}

/*
 * Collect bytes for up to `ms` milliseconds (or until `cap` is reached), and
 * report whether `needle` appears anywhere in what arrived. The MCU's replies
 * are tiny and this runs a handful of times at boot, so a byte-at-a-time read
 * driven by poll() is more than fast enough and keeps the logic obvious.
 */
static int read_contains(int fd, const unsigned char *needle, size_t nlen,
			 int ms, unsigned char *buf, size_t cap, size_t *got)
{
	struct timespec t0, now;
	size_t n = 0;

	clock_gettime(CLOCK_MONOTONIC, &t0);
	for (;;) {
		int elapsed, remaining;
		struct pollfd pfd = { .fd = fd, .events = POLLIN };

		clock_gettime(CLOCK_MONOTONIC, &now);
		elapsed = (int)((now.tv_sec - t0.tv_sec) * 1000 +
				(now.tv_nsec - t0.tv_nsec) / 1000000);
		remaining = ms - elapsed;
		if (remaining <= 0)
			break;

		if (poll(&pfd, 1, remaining) <= 0)
			continue;
		if (n < cap) {
			ssize_t r = read(fd, buf + n, cap - n);
			if (r > 0)
				n += (size_t)r;
		} else {
			unsigned char drop[64];
			(void)!read(fd, drop, sizeof(drop));
		}
		if (nlen && n >= nlen) {
			for (size_t i = 0; i + nlen <= n; i++)
				if (!memcmp(buf + i, needle, nlen)) {
					if (got) *got = n;
					return 1;
				}
		}
	}
	if (got) *got = n;
	return 0;
}

/*
 * Does a Control4 IO *application* answer? FIRMWARE_VERSION_GET (0x34) is the
 * same request the kernel driver identifies with; a reply opcode of 0x35
 * (request+1) inside a DLE/STX frame means the app is up. Used both to skip an
 * unnecessary handshake and to confirm one worked.
 */
static int app_alive(int fd)
{
	/* DLE STX | 0x34 seq flags len16 | csum(=negated sum of body) */
	static const unsigned char fwget[] =
		{ 0x10, 0x02, 0x34, 0x00, 0x00, 0x00, 0x00, 0xcc };
	static const unsigned char resp[] = { 0x10, 0x02, 0x35 };
	unsigned char buf[128];

	tcflush(fd, TCIOFLUSH);
	if (write_all(fd, fwget, sizeof(fwget)) < 0)
		return 0;
	return read_contains(fd, resp, sizeof(resp), 300, buf, sizeof(buf), NULL);
}

/*
 * Run the TI serial-bootloader bring-up so the application starts. Returns 0
 * once an application answers, -1 if the part could not be brought up.
 *
 * Bytes are from the vendor's own reflash library and the captured handshake in
 * docs/shared/io-mcu.md:
 *   autobaud      55 55               -> 00 CC   (0xCC = bootloader ACK)
 *   PING          03 20 20            -> 00 CC
 *   COMMAND_RUN   07 32 22 00 00 10 00-> (none, by design: it has already
 *                                         jumped to the app at 0x1000)
 *   app autobaud  55 55               -> 10 02 d7 ... (AUTO_BAUD_RESP frame)
 * COMMAND_RUN's address is big-endian (0x00001000) and is deliberately not
 * ACKed. We do not send GET_STATUS: its reply is a *packet* that must itself be
 * ACKed or the bootloader desyncs, and we do not need it.
 */
static int iomcu_bringup(int fd)
{
	static const unsigned char knock[]  = { 0x55, 0x55 };
	static const unsigned char ack[]    = { 0x00, 0xcc };
	static const unsigned char ping[]   = { 0x03, 0x20, 0x20 };
	static const unsigned char run[]    = { 0x07, 0x32, 0x22, 0x00, 0x00, 0x10, 0x00 };
	static const unsigned char dstx[]   = { 0x10, 0x02 };
	unsigned char buf[128];

	if (app_alive(fd)) {
		fprintf(stderr, "iomcu-attach: application already running\n");
		return 0;
	}

	fprintf(stderr, "iomcu-attach: no application — running TI bootloader bring-up\n");

	/* autobaud the bootloader: it measures our rate from 0x55 0x55. */
	tcflush(fd, TCIOFLUSH);
	if (write_all(fd, knock, sizeof(knock)) < 0)
		return -1;
	if (!read_contains(fd, ack, sizeof(ack), 400, buf, sizeof(buf), NULL)) {
		fprintf(stderr, "iomcu-attach: bootloader did not ACK autobaud (55 55)\n");
		return -1;
	}

	/* PING, mostly to confirm the link before the un-ACKed RUN. */
	if (write_all(fd, ping, sizeof(ping)) < 0)
		return -1;
	(void)read_contains(fd, ack, sizeof(ack), 400, buf, sizeof(buf), NULL);

	/* RUN the application at 0x1000. No reply by design. */
	if (write_all(fd, run, sizeof(run)) < 0)
		return -1;
	usleep(300000);

	/* The freshly-started application autobauds on its own first 0x55 0x55. */
	tcflush(fd, TCIOFLUSH);
	if (write_all(fd, knock, sizeof(knock)) < 0)
		return -1;
	(void)read_contains(fd, dstx, sizeof(dstx), 700, buf, sizeof(buf), NULL);

	if (app_alive(fd)) {
		fprintf(stderr, "iomcu-attach: application is up\n");
		return 0;
	}
	fprintf(stderr, "iomcu-attach: application did not come up after RUN\n");
	return -1;
}

int main(int argc, char **argv)
{
	struct termios t;
	speed_t speed;
	long baud, ldisc;
	int fd, n;

	if (argc != 4) {
		fprintf(stderr, "usage: %s <tty> <baud> <ldisc>\n", argv[0]);
		return 2;
	}
	baud = strtol(argv[2], NULL, 10);
	ldisc = strtol(argv[3], NULL, 10);
	speed = to_speed(baud);
	if (!speed) {
		fprintf(stderr, "iomcu-attach: unsupported baud %ld\n", baud);
		return 2;
	}

	fd = open(argv[1], O_RDWR | O_NOCTTY);
	if (fd < 0) {
		fprintf(stderr, "iomcu-attach: %s: %s\n", argv[1], strerror(errno));
		return 1;
	}

	/*
	 * Raw 8N1 at the board's rate. The discipline is handed a byte stream,
	 * so anything the tty layer would otherwise do to it — echo, newline
	 * translation, flow control — is a corruption of a binary protocol. The
	 * bring-up handshake below needs exactly the same raw framing.
	 */
	if (tcgetattr(fd, &t) < 0) {
		fprintf(stderr, "iomcu-attach: tcgetattr: %s\n", strerror(errno));
		return 1;
	}
	cfmakeraw(&t);
	t.c_cflag |= CLOCAL | CREAD;
	t.c_cflag &= ~(unsigned)(CSTOPB | PARENB | PARODD | CRTSCTS);
	t.c_cflag = (t.c_cflag & ~(unsigned)CSIZE) | CS8;
	cfsetispeed(&t, speed);
	cfsetospeed(&t, speed);
	if (tcsetattr(fd, TCSANOW, &t) < 0) {
		fprintf(stderr, "iomcu-attach: tcsetattr: %s\n", strerror(errno));
		return 1;
	}
	tcflush(fd, TCIOFLUSH);

	/*
	 * Start the application BEFORE handing the port to the line discipline.
	 * Best-effort: on failure we still attach, so the driver gets to report
	 * the "no reply" diagnosis with its tx/rx counters.
	 */
	if (iomcu_bringup(fd) < 0)
		fprintf(stderr, "iomcu-attach: bring-up failed; attaching anyway\n");
	tcflush(fd, TCIOFLUSH);

	n = (int)ldisc;
	if (ioctl(fd, TIOCSETD, &n) < 0) {
		fprintf(stderr, "iomcu-attach: TIOCSETD %ld: %s\n", ldisc, strerror(errno));
		return 1;
	}

	/*
	 * Hold the fd. The discipline — and therefore the gpiochip — lives
	 * exactly as long as this process does.
	 */
	for (;;)
		pause();
}
