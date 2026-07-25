FROM rust:1-slim-bookworm AS builder

# Limit parallel Cargo compilation jobs to 2 to prevent power surges & memory spikes on RK3588
ENV CARGO_BUILD_JOBS=2

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libdrm-dev \
    libv4l-dev \
    v4l-utils \
    libdrm-tests \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 1. Copy Cargo manifest first to cache dependency compilation layer
COPY Cargo.toml ./

# 2. Pre-build Cargo dependencies using a dummy main.rs
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

# 3. Copy actual Rust source code (subsequent edits recompile in ~1 second!)
COPY src/ ./src/
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    libdrm2 \
    libv4l-0 \
    v4l-utils \
    libdrm-tests \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/rock5c-v4l2-drm ./rock5c-v4l2-drm

EXPOSE 4433/udp

CMD ["./rock5c-v4l2-drm"]
