# Dev image: compile, test, headless rendering. Everything runs here, never on the host.
FROM rust:1.100.0-bookworm AS dev

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_HOME=/usr/local/cargo \
    CARGO_TARGET_DIR=/cache/target \
    SCCACHE_DIR=/cache/sccache \
    # cargo-xwin: accept the Microsoft SDK/CRT licence (required by the tool).
    XWIN_ACCEPT_LICENSE=1 \
    XWIN_CACHE_DIR=/cache/xwin \
    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json

RUN apt-get update && apt-get install -y --no-install-recommends \
        clang lld llvm cmake pkg-config ca-certificates curl git file zip \
        mesa-vulkan-drivers libvulkan1 vulkan-tools vulkan-validationlayers \
    && rm -rf /var/lib/apt/lists/*

# Pinned toolchain (same file the repo uses) plus Windows/Linux cross targets.
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN cd /tmp && rustup show active-toolchain || rustup toolchain install \
    && rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc aarch64-unknown-linux-gnu

RUN cargo install --locked cargo-xwin cargo-nextest cargo-deny sccache \
    && rm -rf /usr/local/cargo/registry

# aarch64 Linux cross linker and sysroot (after cargo install to keep that layer cached).
RUN apt-get update && apt-get install -y --no-install-recommends \
        gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTC_WRAPPER=sccache

RUN mkdir -p /cache && chmod -R a+rwX /cache
WORKDIR /workspace

# Builds the placeholder binary for the runtime image.
FROM dev AS server-build
COPY . /workspace
RUN cargo build --release -p probe && cp /cache/target/release/probe /probe

# Standalone server image. PLACEHOLDER: the entrypoint is the probe binary;
# lot 1.x replaces it with the `dedicated` server binary.
FROM debian:bookworm-slim AS server-runtime
COPY --from=server-build /probe /usr/local/bin/probe
ENTRYPOINT ["/usr/local/bin/probe"]
