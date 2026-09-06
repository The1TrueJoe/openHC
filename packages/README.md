# packages/ — openHC on-device daemons

Two things share this directory.

**A Cargo workspace** for the userspace daemons that ship in the rootfs.
Cross-compiled on the host (rust-lld links ELF with no Docker or
cross-binutils), then staged into `board/common/rootfs-overlay/opt/ohc/bin/` so
`make image` bundles it.

**Buildroot packages**, one per subdirectory containing a `<name>.mk` —
`board/external.mk` globs them in and `board/Config.in` sources each
`Config.in`. These are built *by* Buildroot rather than staged into the
overlay, because they need the target toolchain or a kernel tree: `figlet`,
`ohc-motd`, `ohc-splash`, `sgx545-ce`, `sgx545-um`, `wpebackend-pvr`,
`ohc-webview`. `build/build.sh` force-rebuilds every one of them on each run,
since we maintain them and their stamps mean nothing.

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

## The display stack

Three packages that only make sense together, on boards with the `sgx` feature:

```
ohc-webview            creates one WebKit view, loads a URL, runs a main loop
   |  links
WPE WebKit             browser engine, renders through EGL/GLES, no X, no
   |  dlopens          window system of its own
wpebackend-pvr         libwpe backend: hands WPE the DDK's framebuffer EGL
   |  draws through
sgx545-um  +  sgx545-ce    PowerVR DDK userspace + our GPL kernel driver
```

`ohc-webview` defaults to `http://localhost/`, which is **ohc-webd** above — the
dashboard is what a controller should show when nobody has said otherwise.
`OHC_WEBVIEW_URL` overrides it, and that is the seam: a layer built on top of
openHC points the surface somewhere else without openHC needing to know or care
what serves it.

Nothing starts the webview at boot. `S01splash` owns the framebuffer until
someone runs it by hand, which is deliberate while the display path is still
being brought up. See `https://the1truejoe.github.io/openHC/ea/graphics/` for what is verified and what is not.

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
