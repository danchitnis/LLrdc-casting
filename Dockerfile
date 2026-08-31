# 1. Embedded Client and Management UI Builder Stage
FROM node:latest AS html-builder
WORKDIR /app/client
COPY client/package*.json ./
# Hardware browser tests use an installed branded Chrome on the sender; never
# download Playwright's bundled browsers into the production build image.
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
RUN npm ci || npm install
COPY client ./
RUN npm run build

# 2. Rust Binary Builder Stage
FROM rust:1-slim-bookworm AS rust-base
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libdrm-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY client ./client
COPY --from=html-builder /app/client/dist/index.html ./client/index.html
COPY --from=html-builder /app/client/dist-admin/index.html ./client/admin.html
COPY src ./src

FROM rust-base AS tests
RUN cargo test --locked

FROM tests AS builder
ARG BUILD_DATE=unknown
ARG BUILD_REVISION=development
ENV LLRDC_BUILD_DATE=$BUILD_DATE LLRDC_BUILD_REVISION=$BUILD_REVISION
RUN cargo build --release --locked

# 3. Runtime Stage
# GStreamer 1.26 contains v4l2slh265dec, the userspace implementation of the
# RK3399 rkvdec stateless request API.
FROM debian:trixie-slim
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    gstreamer1.0-tools gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad gstreamer1.0-libav ffmpeg \
    && rm -rf /var/lib/apt/lists/*
ARG BUILD_DATE=unknown
ARG BUILD_REVISION=development
LABEL org.opencontainers.image.source="https://github.com/danchitnis/LLrdc-casting" \
      org.opencontainers.image.revision="$BUILD_REVISION" \
      org.opencontainers.image.created="$BUILD_DATE"
COPY --from=builder /app/target/release/llrdc-casting /usr/local/bin/llrdc-casting
COPY --from=builder /app/target/release/llrdc-management /usr/local/bin/llrdc-management
ENTRYPOINT ["/usr/local/bin/llrdc-management"]
