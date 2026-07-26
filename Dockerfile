# The ARM64 binary is compiled locally. The target only loads this runtime image.
FROM rust:1-slim-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libdrm-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

# GStreamer 1.26 contains v4l2slh265dec, the userspace implementation of the
# RK3399 rkvdec stateless request API.
FROM debian:trixie-slim
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad gstreamer1.0-libav \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/rock5c-v4l2-drm /usr/local/bin/rock5c-v4l2-drm
ENTRYPOINT ["/usr/local/bin/rock5c-v4l2-drm"]
