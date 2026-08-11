# CE5300 fan PWM (PCI 8086:089f, BAR0 256B) — register map

Reverse-engineered on the stock 3.12 box by driving /sys/class/pwm/pwm0:2 to
0/50/100% via the vendor pwm.ko and dumping BAR0 (physical 0xdffe0f00) with an
mmap /dev/mem reader at each level.

Layout: 4 channels, **stride 0x20**, channel N registers at N*0x20. Fan = ch2 (base 0x40).

Per-channel (offset from channel base):
  +0x00  CTRL    write 0x6000 to run, 0x0 to stop        (ch2 abs 0x40)
  +0x04  DUTY    compare = round(duty_ns * 27 / 1000);    (ch2 abs 0x44)
                 100%/full-on = bit17 (0x00020000)
  +0x08  PERIOD  count   = round(period_ns * 27 / 1000)   (ch2 abs 0x48)
  +0x18  status/counter (read-only, ignore)

Clock = **27 MHz** (1 tick ~= 37.04 ns). period_ns 40000 -> 0x437 (1079).

Evidence:
  - 50%: DUTY 0x044 = 0x21c (540) = PERIOD 0x437 (1079) / 2.  100%: DUTY = 0x20000 (full-on bit).
  - CTRL 0x040 = 0x6000 when run=1, 0 when run=0.
  - stride confirmed: unconfigured ch0/1/3 PERIOD regs at 0x08/0x28/0x68 all read default 0x1ffff;
    only ch2 PERIOD (0x48) = 0x437 after we set 40000ns.
  - No hardware "claim" — the sysfs request/ownership was a driver lock; raw MMIO writes just work.

Driver: bind PCI 8086:089f, pcim_iomap BAR0, implement pwm_ops .apply()/.get_state() using the above.
