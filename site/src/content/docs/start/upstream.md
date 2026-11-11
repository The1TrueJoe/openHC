---
title: Upstream contributions
description: Changes from openHC work that other projects have accepted.
sidebar:
  order: 3
---

When a fix belongs in another project, it goes upstream. openHC carries its
own copy only until the upstream release reaches the build.

This page lists only changes a maintainer has accepted. Patches still under
review are not listed until they are merged or applied.

## Accepted

| Project | Change | Accepted | Upstream | Why openHC needed it |
|---|---|---|---|---|
| Linux (ALSA) | ALSA: hda/generic: Add indep_dig_outs hint | 2026-11-04 | [`dddc85c16`](https://git.kernel.org/pub/scm/linux/kernel/git/tiwai/sound.git/commit/?id=dddc85c161758963d7d9aced4882e3cd6f2e8c61) in sound.git for-next | The HC-800's HDMI output and coax jack play separate streams. |
| Buildroot | package/nqptp: bump to version 1.2.9 | 2026-10-26 | [`04b8e19efd`](https://gitlab.com/buildroot.org/buildroot/-/commit/04b8e19efd4b7085583cdc3fe099890a04abbc9e) | shairport-sync 5.6 needs nqptp's newer clock interface; without it AirPlay 2 does not play. |
| shairport-sync | Give each later instance its own D-Bus and MPRIS name, made from its name | 2026-11-01 | [#2324](https://github.com/mikebrady/shairport-sync/pull/2324) | Each AirPlay endpoint on one controller gets its own D-Bus name. |
| shairport-sync | Add dbus_nametag to name each instance's D-Bus and MPRIS interfaces | 2026-11-03 | [#2330](https://github.com/mikebrady/shairport-sync/pull/2330) | Same, with a name the config chooses. |
| gl4es | src/gl: Track GL_FOG_HINT and reject invalid modes | 2026-10-28 | [#526](https://github.com/ptitSeb/gl4es/pull/526) | Minecraft on the EA's PowerVR SGX 545. |
| gl4es | src/gl: Add LIBGL_VERTEXFOG for per-vertex fixed-pipeline fog | 2026-10-28 | [#527](https://github.com/ptitSeb/gl4es/pull/527) | Minecraft on the EA's PowerVR SGX 545. |
| gl4es | src/gl: Draw GL_LINE_STRIP arrays as elements on PowerVR SGX 545 | 2026-10-28 | [#528](https://github.com/ptitSeb/gl4es/pull/528) | Minecraft on the EA's PowerVR SGX 545. |
| gl4es | src/gl: Report GL_ACTIVE_TEXTURE and GL_CLIENT_ACTIVE_TEXTURE from gl4es state | 2026-10-28 | [#529](https://github.com/ptitSeb/gl4es/pull/529) | Minecraft on the EA's PowerVR SGX 545. |
| gl4es | src/gl: Accept GL_TEXTURE_MAX_ANISOTROPY as a no-op without anisotropic filtering | 2026-10-28 | [#530](https://github.com/ptitSeb/gl4es/pull/530) | Minecraft on the EA's PowerVR SGX 545. |

## Still carried in openHC

Accepted is not the same as released. Until a release with the change reaches
the build, openHC keeps its own copy:

- **ALSA `indep_dig_outs`**:
  `board/hc800/patches/linux/0001-ALSA-hda-generic-Add-indep_dig_outs-hint.patch`,
  until the kernel openHC builds contains it.
The shairport-sync D-Bus names are no longer carried: both pull requests are in
shairport-sync 5.6.3, which openHC builds.

The gl4es fixes are no longer carried:
[openhc-minecraft](https://github.com/The1TrueJoe/openhc-minecraft) builds
gl4es at upstream `c1d7d014`, which includes all five.
