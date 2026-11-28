# Contributing

How openHC is built and laid out. For installing it, see the
[README](README.md). The [documentation site](https://the1truejoe.github.io/openHC/)
has the hardware research behind each board.

## Building an image

The image builds with Buildroot inside Docker, so nothing is installed on the
host:

```sh
make image BOARD=ea3-v1          # writes output/images/
make help                        # every target and board
```

`make native BOARD=...` builds without Docker on Linux, the way CI does
(`build/image.sh`; host packages from `build/deps.sh`). `JOBS=1` helps if the
toolchain build runs out of memory.

CI builds every board on each push. Main's builds go on the rolling `dev`
pre-release (`openhc-<board>-<sha>-dev.zip`), and a published release gets its
own set. The flasher downloads the newest published release, or `dev` until
there is one.

## The flasher

```sh
cd flasher
cargo test --workspace
cargo build --release -p ohc-flash-gui    # ohc-flasher, the GUI
cargo build --release -p ohc-flash-cli    # ohc-flash, the command line
```

To install your own build, use the GUI's **Use my own image** (or drag the
folder or zip onto its window), or `ohc-flash install <ip> --images output/images`.

`make gui`, `make discover` and `make flash HOST=<ip> IMAGES=<dir|zip>` run the
same from the repository root.

## Layout

| Path | What |
|---|---|
| `board/common/` | what every board gets: the shared defconfig, kernel config, rootfs overlay (init scripts in `etc/init.d`), and feature sets under `features/` |
| `board/<board>/` | one board family (`ea/`, `ca1/`, `hc250/`, `hc800/`, `ioxv1/`): its defconfig, kernel fragment, `ohc.features` and overlay |
| `packages/` | openHC's own programs and their Buildroot packages: `iod` (IO), `audiod` (audio outputs and endpoints), `webd` (the web page), `sysmond`, `zigbee`, `webview`, `update`, `restore`, ... |
| `flasher/` | the flasher: `core` (boards, methods), `transport` (SSH, discovery, TFTP), `engine` (the install flows), `gui`, `cli` |
| `build/` | the Docker and CI build scripts |
| `site/` | the documentation site |

A board's defconfig is built from layers: `board/common/common_defconfig`, then
the family's, then each feature set its `ohc.features` lists, then the board's
own. Later layers win.

## Pull requests

Work on a branch and open a pull request against `main`; CI's `ci-ok` check
has to pass. Keep a change to one subject, and say in the description what it
was tested on (which board, or "builds only").
