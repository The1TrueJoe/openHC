// SPDX-License-Identifier: MIT
/*
 * splash — draw the openHC banner straight onto the framebuffer.
 *
 * No text console, no font, no image asset. `figlet -f slant openHC` emits an
 * ASCII drawing built from exactly five characters -- space . / \ _ -- so the
 * banner is rendered by drawing those as strokes at whatever size the panel
 * wants. That keeps it programmatic (change the word, change the picture) and
 * resolution-independent, and it costs no font data and no licence question.
 *
 * The banner text is read from a file, or from stdin when the path is "-", so
 * the usual invocation is simply:  figlet -f slant openHC | splash -
 */

#include <fcntl.h>
#include <linux/fb.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <ifaddrs.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>

#define MAXROWS 32
#define MAXCOLS 256

static unsigned char *fb;
static unsigned int fb_w, fb_h, fb_stride;
static unsigned int colour = 0xFF00FF00;   /* ARGB8888 green */

static void px(int x, int y)
{
	if (x < 0 || y < 0 || (unsigned)x >= fb_w || (unsigned)y >= fb_h)
		return;
	*(unsigned int *)(fb + (unsigned)y * fb_stride + (unsigned)x * 4) = colour;
}

/* filled square of side t centred on (x,y) -- the "pen" */
static void dot(int x, int y, int t)
{
	int i, j, h = t / 2;
	for (j = -h; j <= h; j++)
		for (i = -h; i <= h; i++)
			px(x + i, y + j);
}

static void line(int x0, int y0, int x1, int y1, int t)
{
	int dx = abs(x1 - x0), sx = x0 < x1 ? 1 : -1;
	int dy = -abs(y1 - y0), sy = y0 < y1 ? 1 : -1;
	int err = dx + dy, e2;

	for (;;) {
		dot(x0, y0, t);
		if (x0 == x1 && y0 == y1)
			break;
		e2 = 2 * err;
		if (e2 >= dy) { err += dy; x0 += sx; }
		if (e2 <= dx) { err += dx; y0 += sy; }
	}
}

/* One figlet cell at (x,y), size cw x ch. Only the five glyphs figlet slant
 * actually emits are handled; anything else is left blank on purpose rather
 * than drawn as a box, because a wrong box is more misleading than a gap. */
static void glyph(char c, int x, int y, int cw, int ch, int t)
{
	switch (c) {
	case '_':
		line(x, y + ch - 1 - t / 2, x + cw - 1, y + ch - 1 - t / 2, t);
		break;
	case '/':
		line(x, y + ch - 1, x + cw - 1, y, t);
		break;
	case '\\':
		line(x, y, x + cw - 1, y + ch - 1, t);
		break;
	case '.':
		dot(x + cw / 2, y + ch - 1 - t, t + 1);
		break;
	default:
		break;
	}
}

/* ---- small text: interface addresses in the corner ---------------------- *
 *
 * The banner above is drawn as strokes because figlet slant emits only five
 * characters. An IP address needs actual letterforms, so this is a plain 5x7
 * bitmap font -- written as rows of '#' rather than hex bytes because a
 * mistyped hex byte is invisible until it is on a screen, and a mistyped row
 * is obvious right here.
 *
 * Lowercase only: interface names are lowercase and addresses are digits, so
 * uppercase would be dead weight. Unknown characters draw nothing, matching
 * glyph() above -- a gap is honest, a wrong box is not.
 */
#define FW 5
#define FH 7

struct fglyph { char c; const char *r[FH]; };

static const struct fglyph font[] = {
{'0',{" ### ","#   #","#  ##","# # #","##  #","#   #"," ### "}},
{'1',{"  #  "," ##  ","  #  ","  #  ","  #  ","  #  "," ### "}},
{'2',{" ### ","#   #","    #","   # ","  #  "," #   ","#####"}},
{'3',{"#####","   # ","  #  ","   # ","    #","#   #"," ### "}},
{'4',{"   # ","  ## "," # # ","#  # ","#####","   # ","   # "}},
{'5',{"#####","#    ","#### ","    #","    #","#   #"," ### "}},
{'6',{"  ## "," #   ","#    ","#### ","#   #","#   #"," ### "}},
{'7',{"#####","    #","   # ","  #  "," #   "," #   "," #   "}},
{'8',{" ### ","#   #","#   #"," ### ","#   #","#   #"," ### "}},
{'9',{" ### ","#   #","#   #"," ####","    #","   # "," ##  "}},
{'.',{"     ","     ","     ","     ","     "," ##  "," ##  "}},
{':',{"     "," ##  "," ##  ","     "," ##  "," ##  ","     "}},
{'-',{"     ","     ","     ","#####","     ","     ","     "}},
{'/',{"    #","    #","   # ","  #  "," #   ","#    ","#    "}},
{'a',{"     ","     "," ### ","    #"," ####","#   #"," ####"}},
{'b',{"#    ","#    ","#### ","#   #","#   #","#   #","#### "}},
{'c',{"     ","     "," ####","#    ","#    ","#    "," ####"}},
{'d',{"    #","    #"," ####","#   #","#   #","#   #"," ####"}},
{'e',{"     ","     "," ### ","#   #","#####","#    "," ### "}},
{'f',{"  ## "," #  #"," #   ","###  "," #   "," #   "," #   "}},
{'g',{"     "," ####","#   #"," ####","    #","#   #"," ### "}},
{'h',{"#    ","#    ","#### ","#   #","#   #","#   #","#   #"}},
{'i',{"  #  ","     "," ##  ","  #  ","  #  ","  #  "," ### "}},
{'k',{"#    ","#    ","#   #","#  # ","###  ","#  # ","#   #"}},
{'l',{" ##  ","  #  ","  #  ","  #  ","  #  ","  #  "," ### "}},
{'m',{"     ","     ","## # ","# # #","# # #","# # #","#   #"}},
{'n',{"     ","     ","#### ","#   #","#   #","#   #","#   #"}},
{'o',{"     ","     "," ### ","#   #","#   #","#   #"," ### "}},
{'p',{"     ","     ","#### ","#   #","#### ","#    ","#    "}},
{'r',{"     ","     ","# ## ","##   ","#    ","#    ","#    "}},
{'s',{"     ","     "," ####","#    "," ### ","    #","#### "}},
{'t',{" #   "," #   ","###  "," #   "," #   "," #  #","  ## "}},
{'u',{"     ","     ","#   #","#   #","#   #","#   #"," ####"}},
{'v',{"     ","     ","#   #","#   #","#   #"," # # ","  #  "}},
{'w',{"     ","     ","#   #","# # #","# # #","# # #"," ### "}},
{'x',{"     ","     ","#   #"," # # ","  #  "," # # ","#   #"}},
{'y',{"     ","     ","#   #","#   #"," ####","    #"," ### "}},
{'z',{"     ","     ","#####","   # ","  #  "," #   ","#####"}},
};

/* One character at (x,y), each font pixel drawn as an s x s block. */
static void text_char(char c, int x, int y, int s)
{
	unsigned i;
	int r, col, dx, dy;

	for (i = 0; i < sizeof(font) / sizeof(font[0]); i++) {
		if (font[i].c != c)
			continue;
		for (r = 0; r < FH; r++)
			for (col = 0; col < FW; col++)
				if (font[i].r[r][col] == '#')
					for (dy = 0; dy < s; dy++)
						for (dx = 0; dx < s; dx++)
							px(x + col * s + dx, y + r * s + dy);
		return;
	}
}

static void text(const char *str, int x, int y, int s)
{
	for (; *str; str++, x += (FW + 1) * s)
		text_char(*str, x, y, s);
}

/*
 * Draw every IPv4 address the box currently holds, bottom-right, one per line.
 *
 * Read live from the kernel rather than passed in, so the caller does not have
 * to know which interfaces exist -- wired, Wi-Fi and the switch ports all just
 * appear. Loopback is skipped; it tells you nothing and is always there.
 *
 * This runs at whatever moment the splash is drawn, so at S01 time it usually
 * finds nothing (the network is not up yet) and draws nothing, which is
 * correct. The network scripts re-run the splash once they have an address.
 */
static void draw_addresses(void)
{
	struct ifaddrs *ifa, *p;
	char lines[8][40];
	int n = 0, i, s, longest = 0, lh, x, y;

	if (getifaddrs(&ifa))
		return;
	for (p = ifa; p && n < 8; p = p->ifa_next) {
		char ip[INET_ADDRSTRLEN];
		const struct sockaddr_in *in;

		if (!p->ifa_addr || p->ifa_addr->sa_family != AF_INET)
			continue;
		if (!strcmp(p->ifa_name, "lo"))
			continue;
		in = (const struct sockaddr_in *)p->ifa_addr;
		if (!inet_ntop(AF_INET, &in->sin_addr, ip, sizeof(ip)))
			continue;
		snprintf(lines[n], sizeof(lines[n]), "%s %s", p->ifa_name, ip);
		n++;
	}
	freeifaddrs(ifa);
	if (!n)
		return;

	for (i = 0; i < n; i++) {
		int len = (int)strlen(lines[i]);

		if (len > longest)
			longest = len;
	}

	/* Scale to the panel, then keep it inside a sane range: too small to read
	 * is as useless as too big to fit.
	 *
	 * The divisor was 400, which on this hardware is wrong: 720/400 truncates
	 * to s=1, i.e. glyphs 5x7 PIXELS on a 720x480 panel across a room. Verified
	 * on a live EA3 by reading /dev/fb0 back -- the address line occupied
	 * x 608-714, y 469-475, seven pixels tall. It was legible only because the
	 * panel was close.
	 *
	 * 240 gives s=3 (15x21 px) at 720 wide, which is comfortably readable and
	 * still fits: 18 chars * (FW+1) * 3 = 324 px, inside the fb_w/2 = 360 px
	 * budget the loop below enforces. Wider panels clamp at 4. */
	s = (int)fb_w / 240;
	if (s < 1) s = 1;
	if (s > 4) s = 4;
	while (s > 1 && longest * (FW + 1) * s > (int)fb_w / 2)
		s--;

	lh = (FH + 2) * s;
	x = (int)fb_w - longest * (FW + 1) * s - 4 * s;
	y = (int)fb_h - n * lh - 2 * s;
	for (i = 0; i < n; i++)
		text(lines[i], x, y + i * lh, s);
}

int main(int argc, char **argv)
{
	const char *path = argc > 1 ? argv[1] : "/etc/splash.txt";
	const char *dev  = argc > 2 ? argv[2] : "/dev/fb0";
	static char rows[MAXROWS][MAXCOLS];
	int nrows = 0, cols = 0, fd, r, c;
	struct fb_var_screeninfo var;
	struct fb_fix_screeninfo fix;
	int cw, ch, t, x0, y0, fill;
	FILE *f = strcmp(path, "-") ? fopen(path, "r") : stdin;

	if (!f) { perror(path); return 1; }
	while (nrows < MAXROWS && fgets(rows[nrows], MAXCOLS, f)) {
		int len = strlen(rows[nrows]);
		while (len && (rows[nrows][len - 1] == '\n' || rows[nrows][len - 1] == '\r'))
			rows[nrows][--len] = 0;
		if (len > cols) cols = len;
		nrows++;
	}
	if (f != stdin) fclose(f);
	/* trailing blank lines carry no ink and would only shrink the banner */
	while (nrows && strspn(rows[nrows - 1], " ") == strlen(rows[nrows - 1]))
		nrows--;
	if (!nrows || !cols) { fprintf(stderr, "splash: empty banner\n"); return 1; }

	fd = open(dev, O_RDWR);
	if (fd < 0) { perror(dev); return 1; }
	if (ioctl(fd, FBIOGET_VSCREENINFO, &var) || ioctl(fd, FBIOGET_FSCREENINFO, &fix)) {
		perror("ioctl"); close(fd); return 1;
	}
	if (var.bits_per_pixel != 32) {
		fprintf(stderr, "splash: need 32bpp, got %u\n", var.bits_per_pixel);
		close(fd); return 1;
	}
	fb_w = var.xres; fb_h = var.yres; fb_stride = fix.line_length;

	fb = mmap(NULL, (size_t)fb_stride * fb_h, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	if (fb == MAP_FAILED) { perror("mmap"); close(fd); return 1; }

	/* How much of the panel the banner fills, as a percentage of each axis.
	 * figlet cells are about twice as tall as wide, and that 2:1 is the only
	 * aspect correction -- it is a banner, and the panel this runs on is fixed
	 * at 720x480.
	 *
	 * 42, not the original 85: at 85% the wordmark spanned nearly the whole
	 * screen. Sizing a logo is something you judge by looking at it, so it is
	 * a knob rather than a constant -- SPLASH_FILL overrides it, and
	 * S01splash passes it through. */
	fill = 42;
	{
		const char *e = getenv("SPLASH_FILL");
		if (e) {
			int v = atoi(e);
			if (v >= 5 && v <= 100) fill = v;
			else fprintf(stderr, "splash: ignoring SPLASH_FILL=%s "
			                     "(want 5..100)\n", e);
		}
	}

	cw = (int)(fb_w * fill / 100) / cols;
	if (cw < 2) cw = 2;
	ch = cw * 2;
	while (nrows * ch > (int)(fb_h * fill / 100) && cw > 2) { cw--; ch = cw * 2; }
	t  = ch / 8; if (t < 2) t = 2;
	x0 = ((int)fb_w - cols * cw) / 2;
	y0 = ((int)fb_h - nrows * ch) / 2;

	memset(fb, 0, (size_t)fb_stride * fb_h);
	for (r = 0; r < nrows; r++)
		for (c = 0; rows[r][c]; c++)
			glyph(rows[r][c], x0 + c * cw, y0 + r * ch, cw, ch, t);

	draw_addresses();

	munmap(fb, (size_t)fb_stride * fb_h);
	close(fd);
	return 0;
}
