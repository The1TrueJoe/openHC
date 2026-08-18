# packages/ — openHC on-device daemons

A Cargo workspace for the userspace that ships in the rootfs. Cross-compiled on
the host (rust-lld links ELF with no Docker or cross-binutils), then staged into
`board/common/rootfs-overlay/opt/ohc/bin/` so `make image` bundles it.

## ohc-webd

The controller dashboard + REST API. A single self-contained binary (~1.5 MB,
static musl) that:

- serves the **React UI** (in `ohc-webd/ui/`, compiled *into* the binary by
  `build.rs`),
- exposes a **board-agnostic REST API** — everything is driven by
  `/opt/ohc/board.env`, so one build runs on any board and differs only in that
  file (a CA-1 with zwave+zigbee+one combo serial vs a 3-serial/zigbee-only
  board),
- bridges each serial port to the browser over **WebSocket** (`/ws/serial/{dev}`)
  — the in-UI xterm terminal, replacing ttyd,
- moves **raw UART frames** to/from the radios (`/api/radios/{type}/tx|rx|reset`)
  — the transport a driver builds on, not a Zigbee/Z-Wave protocol stack.

Stack: `axum` (single-thread tokio), `libc` termios for serial (no serialport
crate), UI is Vite + React + TypeScript with CSS-variable design tokens. API docs
at `/api/openapi.json`. It can also **control Wi-Fi** — `GET /api/wifi/scan` and
`POST /api/wifi/connect` — via the shared `ohc-wifi` crate, so the dashboard drives
the same join flow the setup portal does.

## ohc-portal

The captive-portal Wi-Fi setup — a **separate** web app (~700 KB), deliberately
kept out of the dashboard. `S41wifi-ap` runs it only while the setup AP is up, on
its **own port `:8080`** (the dashboard keeps `:80`). The dashboard 302-redirects
the phone's OS connectivity check to it while the AP is up, which is what trips the
captive-portal popup. It serves a self-contained setup page for *every* path plus
the same scan/join API, and does no wireless I/O itself — it hands off to `ohc-wifi`.

## ohc-wifi

A tiny pure-std lib (no deps) shared by the two binaries above: reads the scanned
SSID cache, writes the `wpa_supplicant` station config (escaping SSID/PSK against
conf-injection), kicks `S41wifi-ap` to switch AP→station, and holds the portal
page. One copy of the credential-handling logic, used by both.

## Build

```sh
rustup target add armv7-unknown-linux-musleabihf   # ca1 (i.MX6SL, ARMv7)
rustup target add i686-unknown-linux-musl          # ea family (Atom, x86)

make webd BOARD=ca1     # UI build + cross-compile + stage into the overlay
make image BOARD=ca1    # bundle it into the rootfs
```

`packages/build.sh <board>` does the UI build (`npm run build` — must precede
cargo, since `build.rs` embeds `ui/dist`), picks a cargo whose toolchain has the
target's std (Homebrew's shadowing rustc does not), cross-compiles, and installs
the binary into the overlay. The init script `S90ohcweb` runs this one binary
(it just no-ops if the binary is not staged yet — run `make webd` first).

## Layout

```
packages/
  Cargo.toml            virtual workspace (ohc-wifi, ohc-webd, ohc-portal)
  .cargo/config.toml    cross targets (rust-lld linker)
  rust-toolchain.toml   pins the rustup toolchain
  build.sh              host build + stage both binaries into the overlay
  ohc-wifi/             shared lib: scan cache, wpa config, portal page
  ohc-webd/
    Cargo.toml
    build.rs            embeds ui/dist
    src/                main, api, serial (libc termios), board, system
    ui/                 Vite + React + TS dashboard
  ohc-portal/           standalone captive-portal app (:80, AP-only)
    src/main.rs
```
