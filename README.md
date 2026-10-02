# openHC

Open, kernel-up firmware for **Control4 controllers**. A modern Linux kernel and
root filesystem, built from source, running on hardware the vendor shipped
locked.

📖 **[Documentation and the full hardware research →](https://the1truejoe.github.io/openHC/)**

## Supported controllers

| Controller | Supported |
|---|:---:|
| EA-1 (v1) | ✅ |
| EA-1 (v2) | ❌ |
| EA-1 (v2, PoE) | ❌ |
| EA-3 (v1) | ❌ |
| EA-3 (v2) | ✅ |
| CA-1 | ✅ |
| HC-800 | ✅ |
| IO Extender (v1) | ✅ |

## The flasher

Everything goes through the flasher: installing openHC, going back into it, and
restoring a controller to factory. It works over the network, with no serial
cable and no lid off, and it identifies the controller before touching it.

**GUI:** download `ohc-flasher` for macOS, Windows or Linux from the
[latest release](https://github.com/The1TrueJoe/openHC/releases), along with the
image bundle for your controller (`openhc-<board>-<version>.zip`), and run it.

**Command line:** build `ohc-flash` from this repository:

```sh
cd flasher
cargo build --release -p ohc-flash-cli
```

Both need `sshpass` to log into a controller (`brew install sshpass`, or your
distribution's package).

```sh
ohc-flash discover                                # find controllers on the network
ohc-flash identify <ip>                           # what it is, and what is running
ohc-flash plan <board>                            # what an install would do
ohc-flash install <ip> --images openhc-<board>-<version>.zip
ohc-flash boot <ip>                               # go back into an installed openHC
ohc-flash uninstall <ip>                          # boot stock Control4 again
ohc-flash restore <ip>                            # back to factory
```

`ohc-flash help` lists every option. Before installing, read
[Recovery](https://the1truejoe.github.io/openHC/shared/recovery/) for how your
controller gets back to stock.

## Build it yourself

Needs Docker and Python 3; nothing else is installed on the host.

```sh
make image BOARD=ea3-v2
```

`make help` lists the boards. Full detail:
[Building an image](https://the1truejoe.github.io/openHC/build/).

## Layout

```
board/      Buildroot BR2_EXTERNAL root — one directory per board, plus the
            shared common/ and ea-common/ trees they compose from
packages/   Buildroot packages and the Rust workspace (iod, webd, sysmond)
flasher/    the flasher — Rust workspace, GUI + CLI over one engine
build/      Dockerised Buildroot
site/       this project's documentation site (Astro + Starlight)
```

## License

MIT. Not affiliated with or endorsed by Control4 / Snap One. Reverse-engineered
for interoperability from lawfully-owned units and published GPL sources; no
vendor code is redistributed here.
