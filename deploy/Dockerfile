# ===========================================================================
# VEIL Runtime — Production Docker Image
# Includes: veil-runtime server, veil CLI, Rust toolchain, Node.js, Git
# ===========================================================================

# Build stage — compile veil-runtime and veil-cli from source
FROM rust:1.96-bookworm AS builder

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
# Runtime stage — slim image with toolchains for compiling generated code
# ===========================================================================
FROM debian:bookworm-slim

# System deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git build-essential pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Rust toolchain (minimal — just rustc + cargo for cargo check/build)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --profile minimal --default-toolchain stable && \
    /root/.cargo/bin/rustup component add clippy

ENV PATH="/root/.cargo/bin:${PATH}"

# Node.js 20 (for TypeScript compilation)
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && \
    apt-get install -y --no-install-recommends nodejs && \
    rm -rf /var/lib/apt/lists/* && \
    npm install -g typescript svelte-check

# VEIL binaries
COPY --from=builder /build/target/release/veil-runtime /usr/local/bin/veil-runtime
COPY --from=builder /build/target/release/veil /usr/local/bin/veil

# Platform layers
COPY --from=builder /build/layers /opt/veil/layers

# UI (pre-built SvelteKit app — .svelte-kit/output from npm run build)
COPY ui/.svelte-kit/output /opt/veil/ui

# Environment
ENV VEIL_LAYERS_DIR=/opt/veil/layers
ENV VEIL_UI_DIR=/opt/veil/ui
ENV VEIL_PORT=8080
ENV RUST_LOG=info,veil=debug

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:${VEIL_PORT}/health || exit 1

ENTRYPOINT ["veil-runtime"]
