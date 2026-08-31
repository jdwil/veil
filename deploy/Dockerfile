# ===========================================================================
# VEIL Runtime — Production Docker Image
# Includes: veil-runtime server, veil CLI, Rust toolchain, Node.js, Git,
#           Terraform, AWS CLI, and all tools needed for the deploy pipeline.
# ===========================================================================

# Build stage — compile veil-runtime and veil-cli from source
#
# TOOLCHAIN PIN (load-bearing for the Execution Host FFI path): the builder and
# the runtime rustup toolchain below MUST be the SAME rustc, and MUST match the
# rustc the compile pipeline uses at runtime. Rust has no stable ABI — a cdylib
# artifact built with a different rustc than the host is UB. The execution host
# records its fingerprint and refuses to dlopen a mismatched artifact; pinning
# here keeps legitimate artifacts from being needlessly refused.
ARG RUST_VERSION=1.96.0
FROM rust:${RUST_VERSION}-bookworm AS builder

WORKDIR /build

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/veil-parser/Cargo.toml crates/veil-parser/
COPY crates/veil-ir/Cargo.toml crates/veil-ir/
COPY crates/veil-codegen/Cargo.toml crates/veil-codegen/
COPY crates/veil-cli/Cargo.toml crates/veil-cli/
COPY crates/veil-server/Cargo.toml crates/veil-server/
COPY crates/veil-runtime/Cargo.toml crates/veil-runtime/

# Create dummy src files for dependency compilation
RUN mkdir -p crates/veil-parser/src && echo "pub fn x(){}" > crates/veil-parser/src/lib.rs && \
    mkdir -p crates/veil-ir/src && echo "pub fn x(){}" > crates/veil-ir/src/lib.rs && \
    mkdir -p crates/veil-codegen/src && echo "pub fn x(){}" > crates/veil-codegen/src/lib.rs && \
    mkdir -p crates/veil-cli/src && echo "fn main(){}" > crates/veil-cli/src/main.rs && \
    mkdir -p crates/veil-server/src && echo "pub fn x(){}" > crates/veil-server/src/lib.rs && \
    mkdir -p crates/veil-runtime/src && echo "fn main(){}" > crates/veil-runtime/src/main.rs

# Build dependencies only (cached layer)
RUN cargo build --release -p veil-runtime -p veil-cli 2>/dev/null || true

# Copy actual source
COPY crates/ crates/
COPY layers/ layers/

# Build the real binaries
RUN touch crates/*/src/*.rs && \
    cargo build --release -p veil-runtime -p veil-cli

# ===========================================================================
# Runtime stage — full toolchain for build + deploy pipeline
# ===========================================================================
FROM debian:bookworm-slim

# System deps (including zip for Lambda packaging, unzip for terraform)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential pkg-config libssl-dev \
    zip unzip jq python3 python3-pip && \
    rm -rf /var/lib/apt/lists/*

# ─── Rust toolchain ──────────────────────────────────────────────────────────
# Re-declare the ARG in this stage (ARGs do not cross FROM boundaries) and pin
# rustup to the SAME version as the builder. The compile pipeline
# (compile_and_register → cargo build) runs in THIS image, so its rustc MUST
# equal the host's or the toolchain-fingerprint gate will (correctly) refuse
# every artifact it produces. Do NOT use `--default-toolchain stable` here: a
# floating stable drifts from the builder on every rebuild.
ARG RUST_VERSION=1.96.0
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" && \
    /root/.cargo/bin/rustup component add clippy && \
    /root/.cargo/bin/rustup target add x86_64-unknown-linux-gnu

ENV PATH="/root/.cargo/bin:${PATH}"

# Pin the execution-host's enforced toolchain fingerprint to the pinned rustc +
# target. The host also derives this from `rustc -vV` at startup (present in
# this image), but setting it explicitly makes the contract auditable and keeps
# it correct even if rustc is later removed from the runtime image.
ENV VEIL_TOOLCHAIN_FINGERPRINT="${RUST_VERSION}/x86_64-unknown-linux-gnu"

# ─── Node.js via nvm (version switchable per project) ────────────────────────
ENV NVM_DIR="/root/.nvm"
RUN curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash && \
    . "$NVM_DIR/nvm.sh" && \
    nvm install 20 && \
    nvm alias default 20 && \
    npm install -g typescript svelte-check

# Make node/npm available without sourcing nvm
ENV PATH="/root/.nvm/versions/node/v20.19.0/bin:${PATH}"

# ─── Terraform via tfenv (version switchable per project) ────────────────────
RUN git clone --depth 1 https://github.com/tfutils/tfenv.git /opt/tfenv && \
    ln -s /opt/tfenv/bin/tfenv /usr/local/bin/tfenv && \
    ln -s /opt/tfenv/bin/terraform /usr/local/bin/terraform && \
    tfenv install 1.9.8 && \
    tfenv use 1.9.8

# ─── AWS CLI v2 ──────────────────────────────────────────────────────────────
RUN curl "https://awscli.amazonaws.com/awscli-exe-linux-x86_64.zip" -o "/tmp/awscliv2.zip" && \
    unzip -q /tmp/awscliv2.zip -d /tmp && \
    /tmp/aws/install && \
    rm -rf /tmp/aws /tmp/awscliv2.zip

# ─── VEIL binaries ──────────────────────────────────────────────────────────
COPY --from=builder /build/target/release/veil-runtime /usr/local/bin/veil-runtime
COPY --from=builder /build/target/release/veil /usr/local/bin/veil

# Platform layers
COPY --from=builder /build/layers /opt/veil/layers

# UI (pre-built SvelteKit app — static SPA from npm run build)
COPY ui/build /opt/veil/ui

# Environment
ENV VEIL_LAYERS_DIR=/opt/veil/layers
ENV VEIL_UI_DIR=/opt/veil/ui
ENV VEIL_PORT=8080
ENV RUST_LOG=info,veil=debug

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:${VEIL_PORT}/health || exit 1

ENTRYPOINT ["veil-runtime"]
