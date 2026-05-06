# Multi-stage Dockerfile for `ligate-node`.
#
# Builder stage compiles the release binary with the same env vars CI
# uses (SKIP_GUEST_BUILD=1; the host binary doesn't need a real risc0
# guest at compile time of the host).
#
# Runtime stage is a minimal debian-slim base with just the system
# libraries the binary links against at runtime (libssl, libgcc,
# ca-certificates). No Rust toolchain, no dev headers, no build tools.
#
# Container conventions:
#   - WORKDIR /var/lib/ligate
#   - EXPOSE 12346 (chain REST) and 9100 (Prometheus /metrics)
#   - ENTRYPOINT ["/usr/local/bin/ligate-node"]
#   - Operator passes config + genesis via volume mounts and env vars
#     (SOV_CELESTIA_RPC_URL, SOV_CELESTIA_SIGNER_KEY, etc.).
#
# Tracking issue: #195. Sibling distribution channel for the GitHub
# Releases tarball workflow (#194). Operators choose whichever drop
# matches their ops culture; the binary inside is identical.

FROM rust:1.93-bookworm AS builder

# Build-time deps for the host binary. The chain depends on rocksdb
# (vendors librocksdb-sys), tonic, ed25519-dalek, and the sov-* graph;
# the union of their `build.rs` requirements is libssl-dev, pkg-config,
# cmake, clang, libclang-dev, plus the standard build-essential.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        libssl-dev \
        cmake \
        clang \
        libclang-dev \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/*

# Layer cache: copy Cargo manifests first so dep compilation caches
# survive source-only edits. Touch a stub `main.rs` so the dep build
# does not fail on a missing entrypoint, then replace with the real
# source for the binary build.
WORKDIR /work
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY constants.toml ./
COPY README.md ./
COPY LICENSE-APACHE LICENSE-MIT ./

# Skip the risc0 guest compilation (matches CI). The host binary
# builds against a placeholder guest ELF; real proving lives in a
# separate workflow once Phase A.4 lands.
ENV SKIP_GUEST_BUILD=1

# Release build, only the rollup binary. `--locked` forces using the
# checked-in Cargo.lock for reproducibility.
RUN cargo build --release --bin ligate-node --locked

# Strip symbols (best-effort; the cargo strip profile is on by default
# in newer toolchains, but the explicit call is cheap and idempotent).
RUN strip /work/target/release/ligate-node || true

FROM debian:bookworm-slim AS runtime

# Runtime deps: TLS (rustls uses ring, no openssl needed at link
# time, but Celestia gRPC paths through reqwest may; keep ca-certs
# regardless), libgcc for unwinding, libstdc++ for the C++ bits in
# librocksdb-sys, and adduser for the unprivileged service user.
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libgcc-s1 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -s /sbin/nologin -u 1000 -m -d /var/lib/ligate ligate

COPY --from=builder /work/target/release/ligate-node /usr/local/bin/ligate-node

USER ligate
WORKDIR /var/lib/ligate

# Chain REST + Prometheus metrics. Both bind 127.0.0.1 by default
# inside `ligate-node`'s config; operators expose externally via
# Caddy / Cloudflare or by binding the metrics port differently.
EXPOSE 12346 9100

ENTRYPOINT ["/usr/local/bin/ligate-node"]
CMD ["--help"]

# Image metadata. `LABEL org.opencontainers.image.*` fields populate
# GHCR's repo cards and let downstream tools (e.g., Trivy, Snyk)
# pick up the source repo.
LABEL org.opencontainers.image.title="ligate-node" \
      org.opencontainers.image.description="Ligate Chain rollup node binary" \
      org.opencontainers.image.url="https://github.com/ligate-io/ligate-chain" \
      org.opencontainers.image.source="https://github.com/ligate-io/ligate-chain" \
      org.opencontainers.image.licenses="Apache-2.0 OR MIT" \
      org.opencontainers.image.vendor="Ligate Labs"
