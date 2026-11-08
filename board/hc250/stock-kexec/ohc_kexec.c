// SPDX-License-Identifier: GPL-2.0
/*
 * ohc_kexec: boot a new ARM kernel from the stock Control4 HC-250 kernel
 * (2.6.32, CONFIG_KEXEC off). RAM only, writes no flash.
 *
 *   insmod ohc_kexec.ko kallsyms=0x<kallsyms_lookup_name>
 *   cat zImage-or-uImage > /dev/ohc-kexec
 *   echo "<cmdline>" > /sys/module/ohc_kexec/parameters/cmdline
 *   echo dry > /sys/module/ohc_kexec/parameters/go	(print the plan only)
 *   echo 1 > /sys/module/ohc_kexec/parameters/go
 *
 * The image sits in page chunks outside [RAM base, load_addr + max_mb). At go
 * it shuts devices down like a reboot, turns caches and the MMU off from an
 * identity-mapped stub, copies the chunks and ATAGs into place and jumps to
 * the zImage at load_addr with r0=0, r1=mach, r2=ATAGs.
 */
#include <linux/module.h>
#include <linux/kernel.h>
#include <linux/init.h>
#include <linux/fs.h>
#include <linux/miscdevice.h>
#include <linux/moduleparam.h>
#include <linux/string.h>
#include <linux/gfp.h>
#include <linux/uaccess.h>
#include <asm/io.h>
#include <asm/memory.h>
#include <asm/setup.h>

#define RAM_BASE	0x80000000UL
#define ATAGS_ADDR	(RAM_BASE + 0x100)
#define MAX_CHUNKS	2048
#define TOP_ORDER	8			/* 1 MiB chunks when free */
#define SEG_ORDER	3			/* 32 KiB segment list */
#define WDT2_BASE	0x48314000UL
#define UIMAGE_MAGIC	0x56190527		/* 27 05 19 56, read LE */
#define ZIMAGE_MAGIC	0x016f2818		/* at zImage + 0x24 */

static unsigned int kallsyms;
module_param(kallsyms, uint, 0400);
MODULE_PARM_DESC(kallsyms, "address of kallsyms_lookup_name");
static char *cmdline = "console=ttyS0,115200n8 ohc.ramboot=1";
module_param(cmdline, charp, 0644);
static unsigned int load_addr = 0x82000000;
module_param(load_addr, uint, 0644);
static unsigned int max_mb = 48;
module_param(max_mb, uint, 0644);
MODULE_PARM_DESC(max_mb, "largest image; also the no-alloc window above load_addr");
static unsigned int mem_mb = 504;
module_param(mem_mb, uint, 0644);
static unsigned int mach = 0xffffffff;
module_param(mach, uint, 0644);
static unsigned int shutdown = 1;
module_param(shutdown, uint, 0644);
MODULE_PARM_DESC(shutdown, "0 skips device_shutdown before the jump");

typedef unsigned long (*kln_t)(const char *);
static unsigned long (*k_get_free_pages)(gfp_t, unsigned int);
static void (*k_free_pages)(unsigned long, unsigned int);
static void (*k_restart_prepare)(char *);
static void (*k_setup_mm_for_reboot)(char);
static void (*k_flush_cache_all)(void);

struct chunk { unsigned long virt; unsigned int order; };
static struct chunk chunks[MAX_CHUNKS];
static unsigned int nchunks;
static size_t image_len, cur_used;	/* bytes in, bytes in chunks[nchunks-1] */
static unsigned long rejects;		/* chain of chunks in the window */
static unsigned int alloc_order = TOP_ORDER;
static unsigned long stub, atagv, segv;	/* go's pages, kept until rmmod */

/* Relocation stub: copied to a page, entered at its physical address. */
extern char ohc_stub_start[], ohc_stub_end[];
extern unsigned long ohc_stub_segs, ohc_stub_mach, ohc_stub_atags, ohc_stub_entry;
asm(
"	.text\n"
"	.align	5\n"
"	.globl	ohc_stub_start\n"
"ohc_stub_start:\n"
"	mrc	p15, 0, r0, c1, c0, 0\n"
"	bic	r0, r0, #0x1000\n"		/* I */
"	bic	r0, r0, #0x0800\n"		/* Z */
"	bic	r0, r0, #0x0005\n"		/* C, M */
"	mcr	p15, 0, r0, c1, c0, 0\n"
"	isb\n"
"	mov	r0, #0\n"
"	mcr	p15, 0, r0, c8, c7, 0\n"	/* TLBIALL */
"	mcr	p15, 0, r0, c7, c5, 0\n"	/* ICIALLU */
"	mcr	p15, 0, r0, c7, c5, 6\n"	/* BPIALL */
"	dsb\n"
"	isb\n"
"	ldr	r4, ohc_stub_segs\n"
"1:	ldmia	r4!, {r5, r6, r7}\n"		/* dst, src, len (phys, len % 4 == 0) */
"	cmp	r7, #0\n"
"	beq	3f\n"
"2:	ldr	r8, [r6], #4\n"
"	str	r8, [r5], #4\n"
"	subs	r7, r7, #4\n"
"	bne	2b\n"
"	b	1b\n"
"3:	mov	r0, #0\n"
"	ldr	r1, ohc_stub_mach\n"
"	ldr	r2, ohc_stub_atags\n"
"	ldr	pc, ohc_stub_entry\n"
"	.globl	ohc_stub_segs, ohc_stub_mach, ohc_stub_atags, ohc_stub_entry, ohc_stub_end\n"
"ohc_stub_segs:	.word	0\n"
"ohc_stub_mach:	.word	0\n"
"ohc_stub_atags:	.word	0\n"
"ohc_stub_entry:	.word	0\n"
"ohc_stub_end:\n"
);

struct seg { u32 dst, src, len; };

static int in_window(unsigned long virt, unsigned int order)
{
	unsigned long p = __pa(virt), end = p + (PAGE_SIZE << order);

	return p < load_addr + ((unsigned long)max_mb << 20) && end > RAM_BASE;
}

/* A chunk outside the window; window hits are kept (chained) until free_all. */
static unsigned long get_chunk(unsigned int order)
{
	unsigned long v;
	int tries = 4096;

	while (tries--) {
		v = k_get_free_pages(GFP_KERNEL | __GFP_NOWARN |
				     (order ? __GFP_NORETRY : 0), order);
		if (!v)
			return 0;
		if (!in_window(v, order))
			return v;
		((unsigned long *)v)[0] = rejects;
		((unsigned long *)v)[1] = order;
		rejects = v;
	}
	return 0;
}

static void free_all(void)
{
	unsigned int i;

	for (i = 0; i < nchunks; i++)
		k_free_pages(chunks[i].virt, chunks[i].order);
	while (rejects) {
		unsigned long v = rejects;

		rejects = ((unsigned long *)v)[0];
		k_free_pages(v, ((unsigned long *)v)[1]);
	}
	nchunks = 0;
	image_len = cur_used = 0;
	alloc_order = TOP_ORDER;
}

static int ohc_open(struct inode *inode, struct file *f)
{
	if ((f->f_flags & O_ACCMODE) != O_RDONLY)
		free_all();
	return 0;
}

static ssize_t ohc_write(struct file *f, const char __user *buf, size_t len,
			 loff_t *ppos)
{
	size_t done = 0;

	if (image_len + len > ((size_t)max_mb << 20))
		return -EFBIG;
	while (done < len) {
		struct chunk *c;
		size_t room, n;

		if (!nchunks || cur_used == (PAGE_SIZE << chunks[nchunks - 1].order)) {
			unsigned long v;

			if (nchunks == MAX_CHUNKS)
				return -ENOMEM;
			while (!(v = get_chunk(alloc_order)) && alloc_order)
				alloc_order--;
			if (!v)
				return -ENOMEM;
			chunks[nchunks].virt = v;
			chunks[nchunks].order = alloc_order;
			nchunks++;
			cur_used = 0;
		}
		c = &chunks[nchunks - 1];
		room = (PAGE_SIZE << c->order) - cur_used;
		n = min(room, len - done);
		if (copy_from_user((void *)(c->virt + cur_used), buf + done, n))
			return -EFAULT;
		cur_used += n;
		done += n;
		image_len += n;
	}
	return done;
}

static int ohc_release(struct inode *inode, struct file *f)
{
	if ((f->f_flags & O_ACCMODE) != O_RDONLY)
		printk(KERN_INFO "ohc-kexec: %zu bytes in %u chunks (%lu KiB chunks last)\n",
		       image_len, nchunks, (PAGE_SIZE << alloc_order) >> 10);
	return 0;
}

static const struct file_operations ohc_fops = {
	.owner = THIS_MODULE,
	.open = ohc_open,
	.write = ohc_write,
	.release = ohc_release,
};

static struct miscdevice ohc_misc = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "ohc-kexec",
	.fops = &ohc_fops,
};

static size_t build_atags(struct tag *t)
{
	struct tag *t0 = t;
	size_t n = strlen(cmdline);

	while (n && (cmdline[n - 1] == '\n' || cmdline[n - 1] == ' '))
		n--;
	if (n > COMMAND_LINE_SIZE - 1)
		n = COMMAND_LINE_SIZE - 1;

	t->hdr.tag = ATAG_CORE;
	t->hdr.size = tag_size(tag_core);
	t->u.core.flags = 1;
	t->u.core.pagesize = PAGE_SIZE;
	t->u.core.rootdev = 0;
	t = tag_next(t);

	t->hdr.tag = ATAG_MEM;
	t->hdr.size = tag_size(tag_mem32);
	t->u.mem.start = RAM_BASE;
	t->u.mem.size = (u32)mem_mb << 20;
	t = tag_next(t);

	t->hdr.tag = ATAG_CMDLINE;
	t->hdr.size = (sizeof(struct tag_header) + n + 1 + 3) >> 2;
	memcpy(t->u.cmdline.cmdline, cmdline, n);
	t->u.cmdline.cmdline[n] = 0;
	t = tag_next(t);

	t->hdr.tag = ATAG_NONE;
	t->hdr.size = 0;
	t = (struct tag *)((char *)t + sizeof(struct tag_header));
	return (char *)t - (char *)t0;
}

static int ohc_go(const char *val, struct kernel_param *kp)
{
	struct seg *s;
	void __iomem *wdt;
	size_t skip = 0, off = 0, alen;
	unsigned int i;
	void (*jump)(void);

	if (!image_len || !nchunks)
		return -ENODATA;
	if (*(u32 *)chunks[0].virt == UIMAGE_MAGIC)
		skip = 64;
	if (image_len <= skip + 0x30 ||
	    *(u32 *)(chunks[0].virt + skip + 0x24) != ZIMAGE_MAGIC) {
		printk(KERN_ERR "ohc-kexec: not a zImage (or uImage of one)\n");
		return -ENOEXEC;
	}

	if (!stub)
		stub = get_chunk(0);
	if (!atagv)
		atagv = get_chunk(0);
	if (!segv)
		segv = get_chunk(SEG_ORDER);
	if (!stub || !atagv || !segv)
		return -ENOMEM;
	if ((nchunks + 2) * sizeof(*s) > (PAGE_SIZE << SEG_ORDER))
		return -E2BIG;

	alen = build_atags((struct tag *)atagv);
	s = (struct seg *)segv;
	for (i = 0; i < nchunks; i++) {
		size_t len = (i == nchunks - 1) ? cur_used : (PAGE_SIZE << chunks[i].order);
		size_t from = i ? 0 : skip;	/* a uImage header sits in chunk 0 */

		if (len > from) {
			s->dst = load_addr + off + from - skip;
			s->src = __pa(chunks[i].virt) + from;
			s->len = (len - from + 3) & ~3;
			s++;
		}
		off += len;
	}
	s->dst = ATAGS_ADDR;
	s->src = __pa(atagv);
	s->len = (alen + 3) & ~3;
	s++;
	s->dst = s->src = s->len = 0;

	memcpy((void *)stub, ohc_stub_start, ohc_stub_end - ohc_stub_start);
	*(u32 *)(stub + ((char *)&ohc_stub_segs - ohc_stub_start)) = __pa(segv);
	*(u32 *)(stub + ((char *)&ohc_stub_mach - ohc_stub_start)) = mach;
	*(u32 *)(stub + ((char *)&ohc_stub_atags - ohc_stub_start)) = ATAGS_ADDR;
	*(u32 *)(stub + ((char *)&ohc_stub_entry - ohc_stub_start)) = load_addr;
	jump = (void (*)(void))__pa(stub);

	printk(KERN_EMERG "ohc-kexec: %zu bytes -> 0x%08x, %u segments, stub 0x%08lx, ATAGs 0x%08lx: %s\n",
	       image_len - skip, load_addr, (unsigned int)(s - (struct seg *)segv),
	       __pa(stub), ATAGS_ADDR, cmdline);
	if (val && val[0] == 'd') {		/* dry run: show the plan, stay in stock */
		for (s = (struct seg *)segv; s->len; s++)
			printk(KERN_INFO "ohc-kexec:   0x%08x <- 0x%08x +0x%x\n",
			       s->dst, s->src, s->len);
		return 0;
	}

	/* Stock watchdogd holds WDT2 open (NOWAYOUT): stop it while its clocks are on. */
	wdt = ioremap(WDT2_BASE, SZ_4K);
	if (wdt) {
		int t;

		__raw_writel(0xAAAA, wdt + 0x48);
		for (t = 1000000; t && (__raw_readl(wdt + 0x34) & 0x10); t--)
			;
		__raw_writel(0x5555, wdt + 0x48);
		for (t = 1000000; t && (__raw_readl(wdt + 0x34) & 0x10); t--)
			;
	}

	if (shutdown)
		k_restart_prepare(NULL);	/* reboot notifiers + device_shutdown */
	printk(KERN_EMERG "ohc-kexec: bye\n");

	local_irq_disable();
	local_fiq_disable();
	k_setup_mm_for_reboot(0);	/* 1:1 sections below TASK_SIZE */
	k_flush_cache_all();		/* clean+invalidate D (all levels), invalidate I */
	asm volatile("mcr p15, 0, %0, c8, c7, 0\n"
		     "dsb\n"
		     "isb\n" : : "r" (0) : "memory");
	jump();
	return 0;
}
static unsigned int go;
module_param_call(go, ohc_go, param_get_uint, &go, 0200);

#define RESOLVE(var, name) do { \
	*(unsigned long *)&(var) = kln(name); \
	if (!(var)) { printk(KERN_ERR "ohc-kexec: no %s\n", name); return -ENOENT; } \
} while (0)

static int __init ohc_init(void)
{
	kln_t kln = (kln_t)kallsyms;

	if (!kallsyms) {
		printk(KERN_ERR "ohc-kexec: kallsyms= (kallsyms_lookup_name) is required\n");
		return -EINVAL;
	}
	if (kln("kallsyms_lookup_name") != kallsyms) {
		printk(KERN_ERR "ohc-kexec: kallsyms= is not kallsyms_lookup_name\n");
		return -EINVAL;
	}
	RESOLVE(k_get_free_pages, "__get_free_pages");
	RESOLVE(k_free_pages, "free_pages");
	RESOLVE(k_restart_prepare, "kernel_restart_prepare");
	RESOLVE(k_setup_mm_for_reboot, "setup_mm_for_reboot");
	RESOLVE(k_flush_cache_all, "v7_flush_kern_cache_all");
	return misc_register(&ohc_misc);
}

static void __exit ohc_exit(void)
{
	misc_deregister(&ohc_misc);
	free_all();
	if (stub)
		k_free_pages(stub, 0);
	if (atagv)
		k_free_pages(atagv, 0);
	if (segv)
		k_free_pages(segv, SEG_ORDER);
}

module_init(ohc_init);
module_exit(ohc_exit);
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("openHC: boot a new kernel from the stock HC-250 kernel");
