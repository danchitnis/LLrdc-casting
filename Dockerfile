# 1. HTML Client Builder Stage
ARG BUILD_DATE=unknown
FROM node:latest AS html-builder
ARG BUILD_DATE
WORKDIR /app/client
COPY client/package*.json ./
RUN npm ci || npm install
COPY client ./
RUN npm run build

# 2. Rust Binary Builder Stage
FROM rust:1-slim-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libdrm-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml ./
COPY client ./client
COPY --from=html-builder /app/client/dist/index.html ./client/index.html
COPY src ./src
ARG BUILD_DATE=unknown
RUN cargo build --release

# 3. Runtime Stage
# GStreamer 1.26 contains v4l2slh265dec, the userspace implementation of the
# RK3399 rkvdec stateless request API.
FROM debian:trixie-slim
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad gstreamer1.0-libav ffmpeg \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/llrdc-casting /usr/local/bin/llrdc-casting
ENTRYPOINT ["/usr/local/bin/llrdc-casting"]
