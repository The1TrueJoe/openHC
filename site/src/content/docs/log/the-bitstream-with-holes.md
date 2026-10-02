---
title: The bitstream with holes in it
topic: IO
summary: The IO Extender's FPGA refused to start, and the theory got as far as JTAG. The bitstream was being read with the wrong ECC and had zeros in the middle. Then IR stayed dark for a simpler reason, a register nobody wrote.
description: Bringing up the IO Extender's serial ports and IR outputs, and the two bugs that sat underneath everything.
sidebar:
  order: 11
  label: The bitstream with holes in it
---

The IO Extender's four RS-232 ports and eight IR outputs live inside a Xilinx
FPGA that powers up blank. Until a bitstream is clocked into it, none of that IO
exists. Control4's kernel loads `fpga_fw.bin` at boot, so openHC had to as well.

The first fix was real. The FPGA's configuration pins share balls with EMIF
address lines, and the vendor flips a pin-mux register for the length of the
load. Without that, the data never reached the part. With it, the FPGA configured
and the serial ports came up.

## Then it stopped working

On a different bench it stopped configuring. The load ran clean, with no CRC
error raised, but DONE stayed low and the version register read `0x8000`. The
driver didn't help: it checked the version register for a few known garbage
patterns, `0x8000` wasn't one, so it declared success, created four UARTs on a
dead part and hung the box a few seconds later.

That driver check was one bug, and an easy one. The real question was why the
same driver and the same file now failed. I ruled things out the slow way: the
network chip on the same bus (eth0 down for the load), reading NAND just before
the load (a run with the bitstream copied over SSH instead), the pin mux (the
registers matched the stock OS). Every clean comparison failed identically.

The thing I hadn't compared was the file. Booted on the stock OS, `fpga_fw.bin`
had a different md5 from my copy. Same size. 1,082 bytes of mine were zeros.

## The wrong ECC, and a filesystem being polite

openHC had read that file off the unit's own NAND, through a JFFS2 mount. The
device tree declared 4-bit hardware ECC. Recomputing the spare-area bytes from a
raw dump showed the flash was written with **1-bit** ECC, stored inverted, at a
different offset. Every sector matched that once the bit order was right, and
nine of them carried a genuine single-bit flip that the correct ECC would have
fixed.

With the wrong mode, openHC fixed nothing. One flip landed in a JFFS2 node of the
bitstream. JFFS2 spotted the bad data CRC and did the reasonable thing for a
filesystem: dropped the node and read that range as a hole. Holes read as zeros.
`cat` succeeded, `md5sum` succeeded, and the FPGA was handed a bitstream with a
gap in it.

It explained something else too. The stock U-Boot keeps a bad-block table in the
last two blocks, written with 1-bit ECC. openHC couldn't read it, so it wrote its
own, which U-Boot then couldn't read, so it wrote its own. Every boot.

One line in the device tree fixed both. Now `S12fpga` copies the bitstream
read-only from the unit's recovery partition at boot, nothing proprietary ships
in the image, and the FPGA comes up `0x0400`, DONE high, every time.

## IR: the register nobody wrote

With the FPGA loaded, the IR driver emitted, GO cleared, and a learner pointed at
the emitter saw nothing. An earlier session had blamed the emitter's alignment,
because the stock OS's own sends weren't reaching the learner either.

The stock driver has no source, but it has debug strings, and they name every
register it writes: *output enabled*, *carrier period*, *watermark*, *repeat
count*. Laid over our register map, three of our names were wrong. What we
called a jack "select" was the FIFO watermark. What we called a carrier prescaler
was the repeat count. And the register at offset zero, which we wrote as 0 every
time, was **output enable**, a bitmask of which jacks to drive. Nothing had ever
been switched on.

Set bit 0 and jack 1 lit. The learner then gave the rest: the carrier is
50 MHz divided by the period register, exact from 50 to 100 kHz, and the FIFO
holds a whole code. An NEC frame came back from the learner as 38000 Hz to the
hertz.

Neither problem was in the code I was looking at when I started. The FPGA bug was
a one-line device tree mistake that only showed up as corruption in another file,
and IR was a register we wrote as zero.
