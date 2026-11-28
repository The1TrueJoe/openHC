# openHC

New, open software for old **Control4 controllers**. openHC replaces the
controller's Control4 OS with a current Linux system, so hardware Control4 no
longer supports keeps working: its relays, contact inputs, IR, serial ports,
Zigbee radio and audio outputs.

📖 **[Documentation and the full hardware research →](https://the1truejoe.github.io/openHC/)**

## What you can make it

The flasher asks what the controller should run and downloads it for you:

| Choice | What you get | Controllers |
|---|---|---|
| **openHC** | A web page for the box, its IO and radios on MQTT for your own home automation, and AirPlay 2 (plus Spotify Connect and more) on every audio output | all |
| **Control4 controller** | Back in your current Control4 project as a device: its IO, audio and an on-screen navigator, through a Composer driver ([openhc-control4](https://github.com/The1TrueJoe/openhc-control4)) | all |
| **Home Assistant** | Home Assistant and Music Assistant on the box, with its IO, Zigbee, Bluetooth and outputs already in them ([c4-ea-homeassistant](https://github.com/The1TrueJoe/c4-ea-homeassistant)) | EA series |
| **Music Assistant only** | Music Assistant on the box, playing to its outputs | EA series |

## Supported controllers

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

These are the controllers I have on hand. More will come. I do not plan to work
on models older than the HC-250 (HC-200, HC-300, HC-500, HC-1000).

## Install

You need the controller on your network and a Mac, Windows or Linux computer
on the same network. No serial cable, and the lid stays on.

1. Download **ohc-flasher** for your computer from the
   [latest release](https://github.com/The1TrueJoe/openHC/releases) (until the
   first release, from **dev (rolling)** on the same page). On macOS and Linux
   it also needs `sshpass` (`brew install sshpass`, or your distribution's
   package).
2. Open it. It scans your network and lists the Control4 controllers it finds.
   Pick yours.
3. It checks which model it is and what it runs now.
4. Choose what it should run (the table above). The flasher downloads it.
5. Press **Install**. The controller restarts on its own, once or twice, over a
   few minutes. Leave it powered until the flasher says it is done.

Then open the controller's address in a browser for openHC's own page. Its
address can change when it restarts; the flasher's **Find it again** shows the
new one. SSH works too: `ssh root@<address>`, password `openhc` (change it with
`passwd`).

Before you install, read
[Recovery](https://the1truejoe.github.io/openHC/shared/recovery/): it explains
how your controller goes back to Control4. The flasher does that too
(**restore**), and openHC never touches the controller's factory recovery.

### From a terminal

The flasher also comes as a command, `ohc-flash`, on the same release page:

```sh
ohc-flash discover                       # find controllers on the network
ohc-flash identify <ip>                  # what it is, and what it runs
ohc-flash install <ip> --with control4   # or openhc, home-assistant, music-assistant
ohc-flash restore <ip>                   # back to factory
```

`ohc-flash help` lists everything else.

## More projects on openHC

- **[openhc-control4](https://github.com/The1TrueJoe/openhc-control4)**: the
  Control4 controller choice.
- **[c4-ea-homeassistant](https://github.com/The1TrueJoe/c4-ea-homeassistant)**:
  the Home Assistant and Music Assistant choices.
- **[openhc-minecraft](https://github.com/The1TrueJoe/openhc-minecraft)**:
  Minecraft on an EA's HDMI output.

## Contributing

Building openHC, the repository's layout and how to work on a board:
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT. Not affiliated with or endorsed by Control4 / Snap One. Reverse-engineered
for interoperability from lawfully-owned units and published GPL sources; no
vendor code is redistributed here.
