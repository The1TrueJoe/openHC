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
  so the handover has to stop the watchdog explicitly. `ohc-hc800 boot` does.
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

It is **off by default** and the HC-800 opts in with `OHC_NET_WATCHDOG=1`, for
the same reason `panic=` is scoped to this board: a reset only helps if it lands
somewhere *different*. Here it lands on the untouched vendor image, which
answers SSH. On a board where openHC is what is installed, the identical reset
boots the identical image into the identical dead network ten minutes later,
forever — a reboot loop, not a safety net.

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
module loading. Listen with `ohc-hc800 log`.

**It starts at about 6.3 seconds**, when the NIC comes up — that is a real limit,
not a bug. Anything before that is only visible on the wire if you have the
cable. In practice a failure that early is a failure to boot at all, and the
answer to that is the watchdog, not a log.

The boot line bakes in the listener's address, which is awkward when the laptop
moves. `ohc-netconsole on <host>` drives the same driver through configfs
(`CONFIG_NETCONSOLE_DYNAMIC`) so you can attach a log stream to a box that is
already up, from wherever you happen to be, without rebuilding or rebooting. It
resolves the remote MAC by pinging and reading the ARP table, falling back to
broadcast — netpoll writes the Ethernet header itself, with no ARP and no
routing, which is exactly what lets it keep logging from inside a panic.

### sysrq, for when userspace is too far gone to run `reboot`

`CONFIG_MAGIC_SYSRQ`, reachable at `/proc/sysrq-trigger`. `ohc-hc800 reset`
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
keys on. `ohc-hc800` learns it the first time it finds the box and caches it, so
it configures itself after one successful `find`.

## The tool

`tools/ohc-hc800` is the loop, with the discovery and the two root passwords
already handled:

```bash
tools/ohc-hc800 find                    # where is it, and which OS booted
tools/ohc-hc800 sh 'dmesg | tail -30'   # run something
tools/ohc-hc800 log                     # netconsole listener
tools/ohc-hc800 boot output/build/hc800/images
tools/ohc-hc800 reset                   # back to stock
```

Seed it once, if mDNS is not available to you:

```bash
OHC_HOST=10.0.0.111 tools/ohc-hc800 find
```

:::caution[About one auth in ten is refused]
Measured on a healthy box: 47 identical `sshpass` runs produced 5
`Permission denied (publickey,password)` and 42 successes. It is **sshpass
racing dropbear's password prompt**, not a wrong password — the rate is the same
with and without the legacy algorithm options, and it happens against both
images.

A tool that treats a refusal as authoritative will call the box unreachable
roughly every tenth command, which is exactly what sends you looking for the
serial cable again. `ohc-hc800` retries; anything you write yourself should too.
The real cure is **public-key auth**, which never touches that prompt — worth
adding a key to the image if you find yourself scripting against it a lot.
:::

## Deploying a new build

Nothing in this changes: CI builds the image, you fetch the bundle, `boot` pushes
it and kexecs. No partition is written at any point.

```bash
gh run download <run-id> -n openhc-hc800 -D /tmp/img && unzip -o /tmp/img/*.zip -d /tmp/img
tools/ohc-hc800 log &                   # in another shell
tools/ohc-hc800 boot /tmp/img
```

The one case that still needs a file from elsewhere: kexec'ing **from the vendor
image**. Its kernel is `CONFIG_KEXEC=y` but Control4 never shipped the userspace
tool, so `boot` pushes a static i686 one. `.github/workflows/tools.yml` builds
it; drop `kexec-i686-static` next to the images or in `~/.cache/openhc/`.

## The whole loop, measured

Run on the unit on 2026-09-10, with the serial cable idle throughout:

| Step | What happened |
|---|---|
| `ohc-hc800 reset` on openHC at `.111` | netconsole caught `sysrq: Resetting`, connection dropped |
| board resets, GRUB `default 1` | vendor image answering SSH ~2 min later — **at `.112`, not `.111`** |
| `ohc-hc800 find` | located it by MAC and reported `running=vendor (Control4 stock)`, kernel `3.16.38-8.260.24` |
| `ohc-hc800 boot <dir>` | pushed the static kexec + 24 MB of image, `kexec -l`, `kexec -e` |
| openHC back | at `.111` again, **685 lines of boot log captured over the network** |

Both DHCP addresses came up inside one hour, which is the whole argument for
never writing one down. The first attempt at this test reported the box dead
because a **stale ARP entry** for `.111` outranked the live one for `.112` —
`sweep()` now probes a candidate before believing it, and says which rows it
skipped.

## What this deliberately does *not* give you

**A power cycle still returns the box to Control4.** openHC is kexec'd, so it
lives entirely in RAM and nothing on disk knows it exists. That is the property
the whole recovery story is built on — but it also means a power cut, or the
watchdog doing its job, leaves you on stock until you run `boot` again. Closing
the lid does not make openHC the thing the box runs; it makes openHC the thing
you can *put* on the box from anywhere.

Making it survive a power cycle means writing `sda1`, and GRUB Legacy has the
right primitive for doing that safely: `savedefault` is in the installed
`stage2`, so `default saved` plus `savedefault 1` on an openHC entry gives
**boot-once** — GRUB reverts the default to the vendor entry before it hands
over to the kernel, so a bad image costs one power cycle and nothing else.
`fallback 1` covers an unreadable kernel on top of that.

That is a real change to the boot chain rather than a runtime one, so read
[Recovery](/shared/recovery/) first — and take the `sda1` backup before, not
after.

## What is still only on the cable

Being honest about the gaps:

- **Anything before the NIC comes up** — roughly the first 6 seconds. A failure
  there is a failure to boot, and the recovery is the watchdog.
- **GRUB itself.** No interactive menu exists (`timeout 0`, `hiddenmenu`), so
  there is nothing to catch even with a cable attached. Choosing a different
  entry means editing `default`, which is a write to `sda1` — see
  [Recovery](/shared/recovery/) first.
- **A dead NIC or a pulled Ethernet cable.** Nothing helps; that is the floor.
