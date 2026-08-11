# Buildroot build, done INSIDE `docker build` with BuildKit cache mounts.
#
# The earlier model bind-mounted the whole build tree from macOS (-v $PWD:/work)
# and Docker Desktop's virtiofs kept dying ("unexpected EOF") under Buildroot's
# heavy package I/O. Building on the VM's native filesystem via cache mounts
# (dl + output) avoids that entirely, and still persists downloads and build
# state across runs. This mirrors openSpeakerPoint's proven build.
#
# Build + extract the image (from repo root):
#   DOCKER_BUILDKIT=1 docker build -f build/Dockerfile \
#       --target artifacts --output type=local,dest=output/images .

FROM debian:bookworm-slim AS build-env

ARG BUILDROOT_REF=2024.02.9
ARG BR2_JLEVEL=

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential bc bison flex git rsync unzip wget cpio file \
        python3 libncurses-dev libssl-dev ca-certificates perl patch xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN git clone --depth 1 --branch "$BUILDROOT_REF" \
        https://github.com/buildroot/buildroot /opt/buildroot

# Buildroot refuses to run as root.
RUN useradd -m builder && chown -R builder:builder /opt/buildroot
ENV BUILDROOT_DIR=/opt/buildroot
USER builder
WORKDIR /src
COPY --chown=builder:builder . /src

# Cache mounts live on the VM's native fs, not the macOS bind mount. dl caches
# downloads; output caches the whole build (toolchain, packages) across runs.
RUN --mount=type=cache,target=/src/dl,uid=1000,gid=1000 \
    --mount=type=cache,target=/src/output,uid=1000,gid=1000 \
    BR2_JLEVEL="$BR2_JLEVEL" bash ./build/build.sh \
    && rm -rf /tmp/artifacts && mkdir -p /tmp/artifacts \
    && cp -a /src/output/build/images/. /tmp/artifacts/

FROM scratch AS artifacts
COPY --from=build-env /tmp/artifacts/ /
