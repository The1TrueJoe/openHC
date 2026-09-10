// SPDX-License-Identifier: MIT
/*
 * iomcu-attach — hand a UART to the openHC IO microcontroller line discipline.
 *
 * The kernel driver gpio-ohc-iomcu presents the relays and contacts as a
 * standard gpiochip, but it has to be given a port first. On x86 there is no
 * device tree or ACPI node describing a Control4 co-processor, so serdev has
 * nothing to enumerate and the attachment has to come from userspace: set the
 * line speed, then TIOCSETD.
 *
 * util-linux's ldattach does exactly this, and pulling it in means pulling in
 * BR2_PACKAGE_UTIL_LINUX_BINARIES — some fifty binaries — for one ioctl on an
 * image the project works hard to keep small. This is that ioctl.
 *
 * The process must stay alive: a line discipline lasts only as long as the fd
 * that set it is open. Closing this would drop the gpiochip.
 */
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <sys/ioctl.h>
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
	 * translation, flow control — is a corruption of a binary protocol.
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
