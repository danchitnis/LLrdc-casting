FROM debian:bookworm-slim

# Install compilation dependencies and libraries
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libdrm-dev \
    libv4l-dev \
    v4l-utils \
    libdrm-tests \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Makefile ./
COPY src/ ./src/

RUN make clean && make

CMD ["./v4l2_dmabuf_drm"]
