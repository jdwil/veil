# Build stage
FROM rust:1.80-bookworm AS builder

WORKDIR /build

# Copy workspace manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/veil-parser/Cargo.toml crates/veil-parser/
COPY crates/veil-ir/Cargo.toml crates/veil-ir/
COPY crates/veil-codegen/Cargo.toml crates/veil-codegen/
COPY crates/veil-cli/Cargo.toml crates/veil-cli/
COPY crates/veil-server/Cargo.toml crates/veil-server/

# Create dummy src files for dependency compilation
RUN mkdir -p crates/veil-parser/src && echo "fn main() {}" > crates/veil-parser/src/lib.rs && \
    mkdir -p crates/veil-ir/src && echo "fn main() {}" > crates/veil-ir/src/lib.rs && \
    mkdir -p crates/veil-codegen/src && echo "fn main() {}" > crates/veil-codegen/src/lib.rs && \
    mkdir -p crates/veil-cli/src && echo "fn main() {}" > crates/veil-cli/src/main.rs && \
    mkdir -p crates/veil-server/src && echo "fn main() {}" > crates/veil-server/src/lib.rs

# Build dependencies only (cached layer)
RUN cargo build --release -p veil-cli 2>/dev/null || true

# Copy actual source
COPY crates/ crates/
COPY layers/ layers/

# Build the real binary
RUN cargo build --release -p veil-cli

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/veil /usr/local/bin/veil
COPY --from=builder /build/layers /opt/veil/layers

ENV VEIL_LAYERS_DIR=/opt/veil/layers
ENV VEIL_PORT=8080
ENV VEIL_DATA_DIR=/var/veil
ENV RUST_LOG=info,veil=debug

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD curl -f http://localhost:${VEIL_PORT}/health || exit 1

ENTRYPOINT ["veil"]
CMD ["serve", "--multi", "-p", "8080"]
