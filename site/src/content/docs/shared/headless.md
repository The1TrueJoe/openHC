---
title: Working without serial
description: What has to be true before you can close a controller up, unplug the console cable, and still develop on it — and the loop you use once it is.
sidebar:
  order: 7
---

Bring-up runs on a serial cable. Development shouldn't have to. This page is the
list of things that have to be true before you can put the lid back on, pull the
console lead, and keep working — and the loop you use afterwards.

The whole question reduces to one thing:

:::note[The rule]
A serial cable is only load-bearing while **a failure requires a human**. Make
every failure end in an automatic reset to something that answers SSH, and the
cable becomes a convenience.
:::

## The recovery loop

On the HC-800 the loop already exists, and it exists because of a decision made
much earlier: **openHC is never installed on this board.** It is `kexec`'d out of
whatever is already running, which writes to no partition at all. GRUB, the
vendor root and the factory-restore image are byte-identical to how they shipped.

```
   ┌──────────────────────────┐
   │  Control4 stock image    │◀────── power cycle
   │  GRUB default 1, sda4    │◀────── panic     (panic=10)
   │  answers SSH             │◀────── hang      (watchdog, 30 s)
   └───────────┬──────────────┘◀────── you       (sysrq-b)
               │
          kexec │  writes nothing
               ▼
   ┌──────────────────────────┐
   │  openHC                  │
   └──────────────────────────┘
```

The two middle arrows are the ones that matter: a panic and a hang get back to a
reachable system **with nobody in the room**. `sysrq-b` is the same thing on
demand, and it works when userspace is too wedged to run `reboot`. The power
cycle is the only one that still needs hands, and after this it is the fallback
rather than the procedure.

`menu.lst` is what makes the top box unconditional: `default 1` selects the
vendor root, `fallback 1` points at that same entry, and `timeout 0` with
`hiddenmenu` means nothing waits for a keypress. **Repeated resets never escalate
toward the factory-restore path** — worth confirming before you rely on a
mechanism whose whole job is to reset the box over and over.

## What had to be added

### The watchdog, so a hang is not permanent

`etc/init.d/S02watchdog` arms `/dev/watchdog` at boot: 30 second timeout, kicked
every 10. On the HC-800 that is the NM10 TCO, which the vendor image drives the
same way (`/sbin/watchdog -t 10`), so it is known good on the silicon rather than
assumed.

Two details that are easy to get wrong:

- **Stopping the kicker is not how you disarm it — it is how you fire it.**
  busybox writes the magic `V` on `SIGTERM`, which tells the driver the close was
  deliberate. Killing it any other way leaves a 30 second fuse burning.
- `kexec -e` jumps straight into the new kernel and runs **no** shutdown script,
  so the handover has to stop the watchdog explicitly. `ohc-flash` does.
  (The new kernel's `iTCO_wdt` probe also stops the timer, so this is belt and
  braces — but a 30 second fuse across a kexec is not a thing to leave to one
  mechanism.)

Boards with `CONFIG_WATCHDOG` off — the whole EA family, which is up against a
bzImage size ceiling — get the script anyway and it exits quietly. Inert beats
absent: the file is where you would look for it.

### Reachability, not just liveness, is what feeds it

A plain watchdog leaves one hole, and it is the one most likely to swallow a
box: **userspace that is perfectly healthy but has no network.** The kicker runs,
the timer never fires, and the unit sits there alive and unreachable — which from
the outside is indistinguishable from a hang and needs exactly the physical
access the watchdog exists to avoid. Breaking the NIC is a normal outcome of
working on a kernel, so this is not a corner case.

So `S02watchdog` also starts a monitor that `SIGKILL`s the kicker when the uplink
has been down for ten minutes. `SIGKILL` and not `SIGTERM`, deliberately: no
magic `V` is written, the chip keeps counting, and the board resets. Verified on
the unit — the kernel logs `watchdog: watchdog0: watchdog did not stop!` and the
board is gone within the timeout.

It is **off everywhere, including the HC-800**, and that is the point: a reset
only helps if it lands somewhere *different*. It landed somewhere different while
openHC was a boot-once; now that openHC is the default entry, the identical reset
boots the identical image into the identical dead network ten minutes later,
forever — a reboot loop, not a safety net. A router reboot would be enough to
start one.

`OHC_NET_WATCHDOG=1` turns it on, and is worth it only on a board where a reset
genuinely lands elsewhere — i.e. alongside `--boot-once`.

It will not arm until the uplink has had carrier **and** an address at least
once, so a board still bringing its network up — or one that has no network at
all — is never caught by it.

### `panic=10`, so a panic is not permanent either

`kernel.panic` defaults to `0`, which means *stop forever*. On a sealed box with
no cable attached, stopping forever and being dead look exactly alike.

This one is built into the kernel (`CONFIG_CMDLINE="panic=10"`), not passed by
the launcher, because the launcher is GRUB's `menu.lst` or somebody's `kexec
--append` — neither of which lives in this repository. Baking it in makes
surviving a panic a property of the **image**, so it holds no matter how the
kernel was started or who typed the command. x86 prepends the built-in line and
appends the bootloader's, and a later value of a repeated key wins, so a launcher
can still override it deliberately.

### netconsole, for the part of boot SSH cannot reach

`dmesg` over SSH gives you everything once the box is up. The window it does not
cover is the one that matters when something goes wrong early — and that is what
the serial console was really for.

```
netconsole=6665@<box>/eth0,6666@<you>/<your-mac>
```

Built in, not a module: a module loads too late to log the thing that stopped the
module loading. Listen with `ohc-flash log`.

**It does not lose the early boot.** netconsole registers as a console with
`CON_PRINTBUFFER`, so when it comes up at around 6.9 s the kernel replays
everything already in the ring buffer — a captured log from this board starts at
`[    0.000000] Linux version` and carries **616 lines from before netconsole
itself existed**. So for a box that boots, the network gives you exactly what
the cable would have.

The limit is narrower than it looks, then, but it is real: a hang *before* that
replay happens means the buffer is never flushed and you get **nothing at all**.
That failure is a failure to boot, and the answer to it is the watchdog rather
than a log.

The boot line bakes in the listener's address, which is awkward when the laptop
moves. `ohc-netconsole on <host>` drives the same driver through configfs
(`CONFIG_NETCONSOLE_DYNAMIC`) so you can attach a log stream to a box that is
already up, from wherever you happen to be, without rebuilding or rebooting. It
resolves the remote MAC by pinging and reading the ARP table, falling back to
broadcast — netpoll writes the Ethernet header itself, with no ARP and no
routing, which is exactly what lets it keep logging from inside a panic.

### sysrq, for when userspace is too far gone to run `reboot`

`CONFIG_MAGIC_SYSRQ`, reachable at `/proc/sysrq-trigger`. `ohc-flash reset`
writes `b` to it: the reset happens inside the kernel, skipping every shutdown
path that might be the thing that is stuck.

## Finding it again

The unit takes a DHCP lease, so **do not write its address down.** It moves, and
chasing a stale one wastes more time than anything else here — a box that had
simply moved to `.112` once cost two unnecessary power cycles because only `.111`
was checked.

Two ways to find it, and you need both because they cover different halves of the
loop:

| | works when | speed |
|---|---|---|
| **mDNS** — `_openhc._tcp`, hostname `openhc-<board>-<MAC>.local` | openHC is running | instant |
| **ARP sweep for the MAC** | either OS is running | ~3 s |

The MAC is the one identifier that survives the reset, so it is what the tooling
keys on, and `ohc-flash discover` reports it. Note that it **probes before it
believes**: an ARP entry for an address the unit has since given up is listed by
neither tool, because a dead address that looks live is worse than no answer.

## The tool

`ohc-flash` is the whole loop — finding a unit, driving it, installing on it and
taking it back off:

```bash
ohc-flash discover                                  # every Control4 unit that answers
ohc-flash identify 10.0.0.111                       # what is it, and how sure are we
ohc-flash plan hc800                                # both methods, and what each writes

ohc-flash log                                       # netconsole listener — start this FIRST
ohc-flash install HOST --images DIR --method kexec --netconsole 10.0.0.105
ohc-flash install HOST --images DIR --method grub   # persistent: openHC every boot
ohc-flash install HOST --images DIR --method grub --boot-once   # ... one boot only
ohc-flash boot HOST                                 # re-enter an installed openHC
ohc-flash uninstall HOST                            # put the boot chain back

ohc-flash sh HOST 'dmesg | tail -30'                # run something
ohc-flash push HOST file...                         # copy into /tmp
ohc-flash reset HOST                                # sysrq-reboot, back to stock
```

There used to be a `tools/ohc-hc800` shell script beside this. It grew during
bring-up, when the flasher knew nothing about the board, and it was folded in
once the flasher did — because the two had become two implementations of the
same discovery, and only one of them had learned not to trust a stale ARP entry.
Two tools disagreeing about which address a box is at is the exact failure this
page exists to prevent.

:::caution[About one auth in ten is refused]
Measured on a healthy box: 47 identical `sshpass` runs produced 5
`Permission denied (publickey,password)` and 42 successes. It is **sshpass
racing dropbear's password prompt**, not a wrong password — the rate is the same
with and without the legacy algorithm options, and it happens against both
images.

A tool that treats a refusal as authoritative will call the box unreachable
roughly every tenth command, which is exactly what sends you looking for the
serial cable again. `ohc-flash` retries; anything you write yourself should too.
The real cure is **public-key auth**, which never touches that prompt — worth
adding a key to the image if you find yourself scripting against it a lot.
:::

## Deploying a new build

Nothing in this changes: CI builds the image, you fetch the bundle, the kexec
method pushes it and starts it. No partition is written at any point.

```bash
gh run download <run-id> -n openhc-hc800 -D /tmp/img && unzip -o /tmp/img/*.zip -d /tmp/img
ohc-flash log &                         # in another shell, BEFORE the install
ohc-flash install <host> --images /tmp/img --method kexec --netconsole <your-ip>
```

The one case that still needs a file from elsewhere: kexec'ing **from the vendor
image**. Its kernel is `CONFIG_KEXEC=y` but Control4 never shipped the userspace
tool, so `boot` pushes a static i686 one. `.github/workflows/tools.yml` builds
it; drop `kexec-i686-static` next to the images or in `~/.cache/openhc/`.

## The whole loop, measured

Run on the unit on 2026-09-10, with the serial cable idle throughout:

| Step | What happened |
|---|---|
| `ohc-flash install --method kexec --netconsole` | staged 25 MB, resolved the listener's MAC from the box, kexec'd — **679 log lines captured, from `[0.000000]`** |
| sysrq-reboot on openHC at `.111` | netconsole caught `sysrq: Resetting`, connection dropped |
| board resets, GRUB `default 1` | vendor image answering SSH ~2 min later — **at `.112`, not `.111`** |
| find it again | located by MAC, reported `running=vendor (Control4 stock)`, kernel `3.16.38-8.260.24` |
| kexec from the vendor image | pushed the static kexec + 24 MB of image, `kexec -l`, `kexec -e` |
| openHC back | at `.111` again, **685 lines of boot log captured over the network** |

Both DHCP addresses came up inside one hour, which is the whole argument for
never writing one down. The first attempt at this test reported the box dead
because a **stale ARP entry** for `.111` outranked the live one for `.112` —
`sweep()` now probes a candidate before believing it, and says which rows it
skipped.

## Installing it, without giving up any of that

A `kexec` install lives entirely in RAM, so a power cut leaves you on stock with
25 MB to push again. The persistent install fixes that **without** inverting the
recovery story, and the trick is one GRUB Legacy keyword.

The naive persistent install is `default 2`, and it quietly undoes everything
above: openHC becomes what the box boots, so a panic reboots into the same
panic and the network watchdog resets into the same dead network. A safety net
turns into a loop.

There are therefore two modes, and the choice is the whole decision.

### The default: openHC is what the box runs

`default 2`, and openHC boots every time — including after a power cut, with
nothing to re-run. `fallback 1` still catches a kernel that will not load.

What it does not catch is a kernel that loads and then panics: `panic=10`
reboots into the same panic. So **the network watchdog is off on this board**
(`OHC_NET_WATCHDOG=0`) and `panic_on_oops` is not set — both of those turn a
survivable fault into a loop once openHC is the thing that boots. The hardware
watchdog stays armed for a genuinely hung kernel, which is what it is for.

Holding the ID button at power-on boots entry 0 regardless of `default`, which
is the way out of a loop.

### `--boot-once`: openHC is what the box can be *asked* to run

For a unit you cannot walk to, or a kernel you do not yet trust. The entry ends
with `savedefault 1` and `default` becomes `saved`. GRUB executes that **before**
handing over to the kernel, so every openHC boot immediately re-points the
default back at Control4:

```
default         saved            <- `default 2` in the persistent mode
...
title           openHC
root            (hd0,2)
kernel          /boot/openhc-bzImage console=ttyS0,115200
initrd          /boot/openhc-initrd.gz
savedefault     1                <- boot-once only; absent by default
boot
```

openHC is then always exactly **one** boot. Anything at all — panic, watchdog,
power cut, `reset` — comes back on stock, which answers SSH. Going back in is
`ohc-flash boot`, which sets one byte and reboots; the images are already on
disk.

The cost is that every reboot needs that command, which is friction you do not
want once the kernel is trusted — hence the default above.

Measured on the unit, in this order:

| | |
|---|---|
| probe `default saved` with saved = the stock entry | booted stock — GRUB reads `/boot/grub/default` correctly on this stage2 |
| `ohc-flash install --method grub --boot-once` | 169536 KB free on `sda3`, wrote 25628 KB, `menu.lst verified, 596 bytes` |
| reboot | **openHC from disk**, `panic=10 console=ttyS0,115200`, no kexec |
| read `/boot/grub/default` | already `1` — `savedefault` fired during that boot |
| reboot again | stock Control4 |
| `ohc-flash boot` | openHC again, and the default back to `1` |
| `ohc-flash uninstall` | `menu.lst` restored from the backup the install kept, images removed from `sda3`, vendor entries byte-identical |
| reinstall from pristine stock | ran the `default 1` → `default saved` conversion for real, `menu.lst verified, 595 bytes` |

:::caution[sda1 is the one unrecoverable partition]
Every recovery layer on this board — including the hardware factory-default
button — needs GRUB to read `menu.lst` from `sda1`. Take the byte-exact backup
first (`backups/hc800/`, md5 recorded and verified), and read back what you
wrote. The installer refuses to write a `menu.lst` whose
`support_factorydefault` / `factorydefault` lines it cannot find afterwards, and
restores its own backup if the read-back differs. `sda2` is never written by any
method. See [Recovery](/shared/recovery/).

## What is still only on the cable

Being honest about the gaps:

- **A hang before netconsole's replay**, i.e. in roughly the first 7 seconds.
  Not the *messages* from that window — those are replayed — but a box that
  stops inside it sends nothing at all. The recovery is the watchdog.
- **GRUB itself.** No interactive menu exists (`timeout 0`, `hiddenmenu`), so
  there is nothing to catch even with a cable attached. Choosing a different
  entry means editing `default`, which is a write to `sda1` — see
  [Recovery](/shared/recovery/) first.
- **A dead NIC or a pulled Ethernet cable.** Nothing helps; that is the floor.
