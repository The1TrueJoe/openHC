# The CEFDK container wrapper lives in the Rust flasher, so the build and the
# installer cannot disagree about the container layout. Built in its own stage:
# it is cached independently of Buildroot, and -p ohc-flash-cli keeps the GUI's
# dependency tree out of it entirely.
# rust:*-slim-bookworm, not Debian's cargo: bookworm ships cargo 0.66 (Rust
# 1.63), which cannot parse a v4 Cargo.lock — and the workspace's lock is v4,
# written by a current cargo. Bookworm base so the binary this produces runs in
# the Buildroot stage below, which is also bookworm.
FROM rust:1-slim-bookworm AS flasher
WORKDIR /f
COPY flasher/ /f/
RUN cargo build --release -p ohc-flash-cli

# Buildroot build, done INSIDE `docker build` with BuildKit cache mounts.
#
# The earlier model bind-mounted the whole build tree from macOS (-v $PWD:/work)
# and Docker Desktop's virtiofs kept dying ("unexpected EOF") under Buildroot's
# heavy package I/O. Building on the VM's native filesystem via cache mounts
# (dl + output) avoids that entirely, and still persists downloads and build
# state across runs. This mirrors openSpeakerPoint's proven build.
#
# Build + extract the image (from repo root):
#   DOCKER_BUILDKIT=1 docker build -f build/Dockerfile --build-arg BOARD=ea1 \
#       --target artifacts --output type=local,dest=output/images .

FROM debian:bookworm-slim AS build-env

# Only BUILDROOT_REF belongs up here: it is consumed by the git clone below.
# BR2_JLEVEL and BOARD are declared immediately before the one RUN that uses
# them, because an ARG invalidates the layer cache from its DECLARATION
# onward, not from first use. Declared here, changing the job count re-ran
# apt-get and re-cloned Buildroot before reaching any of the real work.
ARG BUILDROOT_REF=2024.02.9

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential bc bison flex git rsync unzip wget cpio file figlet \
        python3 libncurses-dev libssl-dev ca-certificates perl patch xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN git clone --depth 1 --branch "$BUILDROOT_REF" \
        https://github.com/buildroot/buildroot /opt/buildroot

# Patches to Buildroot ITSELF, as opposed to BR2_GLOBAL_PATCH_DIR which patches
# package *sources*. Needed when the thing to change is a .mk or a Config.in --
# e.g. a dependency that does not apply to this hardware. Applied with --forward
# so a patch that upstream has since adopted is skipped rather than fatal.
COPY build/buildroot-patches /tmp/br-patches
RUN set -e; for p in /tmp/br-patches/*.patch; do \
        [ -e "$p" ] || continue; \
        echo ">> buildroot patch: ${p##*/}"; \
        patch -d /opt/buildroot -p1 --forward --batch < "$p" || \
            { echo "!! failed: ${p##*/}" >&2; exit 1; }; \
    done

COPY --from=flasher /f/target/release/ohc-flash /usr/local/bin/ohc-flash

# Buildroot refuses to run as root.
RUN useradd -m builder && chown -R builder:builder /opt/buildroot
ENV BUILDROOT_DIR=/opt/buildroot
USER builder
WORKDIR /src
COPY --chown=builder:builder . /src

# Cache mounts live on the VM's native fs, not the macOS bind mount. dl caches
# downloads; output caches the whole build (toolchain, packages) across runs.
#
# sharing=locked, not the default sharing=shared. Two builds writing one output
# cache corrupt each other's kernel tree mid-compile, and the symptoms point
# nowhere near the cause: "./scripts/basic/fixdep: not found" (Error 127),
# "asm-generic/rwonce.h: No such file", "tar: Cannot stat" during extract. It
# does not take two `docker build` commands to hit -- killing the CLI leaves
# the build running inside buildkitd, so a cancelled build keeps writing while
# the next one starts. Locking serialises them; it costs nothing when builds
# do not overlap.
ARG BR2_JLEVEL=
ARG BOARD=ea1

RUN --mount=type=cache,target=/src/dl,uid=1000,gid=1000,sharing=locked \
    --mount=type=cache,target=/src/output,uid=1000,gid=1000,sharing=locked \
    BR2_JLEVEL="$BR2_JLEVEL" BOARD="$BOARD" bash ./build/build.sh \
    && rm -rf /tmp/artifacts && mkdir -p /tmp/artifacts \
    && cp -a "/src/output/build/$BOARD/images/." /tmp/artifacts/

FROM scratch AS artifacts
COPY --from=build-env /tmp/artifacts/ /
