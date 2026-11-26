# openHC

Open, kernel-up firmware for **Control4 controllers**. A modern Linux kernel and
root filesystem, built from source, running on hardware the vendor shipped
locked.

📖 **[Documentation and the full hardware research →](https://the1truejoe.github.io/openHC/)**

## Extensions

Separate projects that build on openHC:

- **[openhc-control4](https://github.com/The1TrueJoe/openhc-control4)**: back
  into a Control4 project as a controller, with its IO and the on-screen
  navigator on the TV.
- **[openhc-minecraft](https://github.com/The1TrueJoe/openhc-minecraft)**:
  Minecraft on an EA's HDMI output.
- **[c4-ea-homeassistant](https://github.com/The1TrueJoe/c4-ea-homeassistant)**:
  Home Assistant on the EA series.

## Currently Supported controllers

| Controller | Supported |
|---|:---:|
| HC-250 | ✅ |
| HC-800 | ✅ |
| EA-1 | ✅ |
| EA-3 | ✅ |
| EA-5 | ❌ |
| CA-1 | ✅ |
| CA-10 | ❌ |
| Core-1 | ❌ |
| Core-3 | ❌ |
| Core-5 | ❌ |
| IO Extender (v1) | ✅ |
| IO Extender (v2) | ❌ |

These are just the controllers I have on hand. More will be coming in the future. I do not have plans to work on models older than the HC-250 (HC-200, HC-300, HC-500, HC-1000).

## The flasher

Everything goes through the flasher: installing openHC, going back into it, and
restoring a controller to factory. It works over the network, with no serial
cable and no lid off, and it identifies the controller before touching it.

**GUI:** download `ohc-flasher` for macOS, Windows or Linux from the
[latest release](https://github.com/The1TrueJoe/openHC/releases), along with the
image bundle for your controller (`openhc-<board>-<version>.zip`), and run it.
On an HC-800 or EA, **Make it a Control4 controller again** on the Install
screen also sets it up with [openhc-control4](https://github.com/The1TrueJoe/openhc-control4)
and saves the Composer driver to your Downloads.

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
ohc-flash control4 <ip>                           # make it a Control4 controller again
ohc-flash boot <ip>                               # go back into an installed openHC
ohc-flash uninstall <ip>                          # boot stock Control4 again
ohc-flash restore <ip>                            # back to factory
```

`ohc-flash help` lists every option. Before installing, read
[Recovery](https://the1truejoe.github.io/openHC/shared/recovery/) for how your
controller gets back to stock.

## License

MIT. Not affiliated with or endorsed by Control4 / Snap One. Reverse-engineered
for interoperability from lawfully-owned units and published GPL sources; no
vendor code is redistributed here.
