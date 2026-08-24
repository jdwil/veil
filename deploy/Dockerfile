# Build stage
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
RUN mkdir -p crates/veil-parser/src && echo "fn main() {}" > crates/veil-parser/src/lib.rs && \
    mkdir -p crates/veil-ir/src && echo "fn main() {}" > crates/veil-ir/src/lib.rs && \
    mkdir -p crates/veil-codegen/src && echo "fn main() {}" > crates/veil-codegen/src/lib.rs && \
    mkdir -p crates/veil-cli/src && echo "fn main() {}" > crates/veil-cli/src/main.rs && \
    mkdir -p crates/veil-server/src && echo "fn main() {}" > crates/veil-server/src/lib.rs && \
    mkdir -p crates/veil-runtime/src && echo "fn main() {}" > crates/veil-runtime/src/main.rs

# Build dependencies only (cached layer)
RUN cargo build --release -p veil-runtime 2>/dev/null || true

# Copy actual source
COPY crates/ crates/
COPY layers/ layers/
COPY stubs/ stubs/
COPY ui/ ui/

# Build the runtime and CLI binaries
RUN cargo build --release -p veil-runtime -p veil-cli

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/veil-runtime /usr/local/bin/veil-runtime
COPY --from=builder /build/target/release/veil /usr/local/bin/veil
COPY --from=builder /build/layers /opt/veil/layers
COPY --from=builder /build/stubs /opt/veil/stubs
COPY --from=builder /build/ui/build /opt/veil/ui

ENV VEIL_LAYERS_DIR=/opt/veil/layers
ENV VEIL_STUBS_DIR=/opt/veil/stubs
ENV VEIL_UI_DIR=/opt/veil/ui
ENV VEIL_PORT=8080
ENV RUST_LOG=info,veil=debug

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:${VEIL_PORT}/health || exit 1

ENTRYPOINT ["veil-runtime"]
