# Radxa Rock 5C+ WebTransport QUIC Screen Sharing Server (Rust Implementation)

A repeatable, containerized zero-copy screen sharing application written in **Safe Rust** for the **Radxa Rock 5C+** (Rockchip RK3588) running Armbian. On startup, it displays the active IPv4 address dashboard with a **live real-time clock (`HH:MM:SS UTC`)** on HDMI, then accepts incoming **WebTransport / QUIC (UDP)** H.264 and H.265 / HEVC video streams, decodes video frames via the hardware video decoder (`/dev/video2` / `rkvdec`), and presents them via a tear-free **V4L2 Decoder → DMA-BUF fd → DRM Atomic Commit (VSYNC Page Flip) → HDMI** pipeline inside Docker.

---

## Technical Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 Local Workstation (Client)                  │
│  - WebTransport QUIC Client over UDP                        │
│  - Downloader: ./download_bunny.sh [RES]                    │
│  - Bitstream Prep: ./prepare_stream.sh [RES] [FPS] [CODEC]   │
│  - Streamer script: ./stream.sh --h264 | --h265             │
│  - Connects to https://192.168.1.72:4433 / UDP:4434          │
└──────────────────────────────┬──────────────────────────────┘
                               │
                      WebTransport / QUIC (UDP)
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 Radxa Rock 5C+ Server (Board)               │
│                                                             │
│  Idle / Waiting Phase:                                      │
│  - Displays IPv4 Dashboard with Live Real-Time Clock on HDMI│
│                                                             │
│  Streaming Phase:                                           │
│  - WebTransport Server accepts QUIC stream over UDP:4433    │
│  - Receives incoming H.264 / H.265 video frame packets      │
│  - Strictly decodes video via RK3588 rkvdec HW decoder      │
│  - Performs Double-Buffered VSYNC Page Flip (0 Tearing)     │
│  - Commits frame directly to DRM KMS -> HDMI Display        │
└─────────────────────────────────────────────────────────────┘
```

---

## Features

- **Server Control Script (`server.sh`)**:
  - `./server.sh --start` (or `--strat`): Builds `linux/arm64` Docker image locally on the workstation and streams a fast compressed image over SSH (`docker save | gzip -1 | ssh ... docker load`) to avoid Rock Pi RAM exhaustion.
  - `./server.sh --stop`: Cleanly stops and removes the running server container on the board.
- **Video Downloader Script (`download_bunny.sh`)**:
  - `./download_bunny.sh [RESOLUTION]`: Downloads and scales Big Buck Bunny video for any resolution (`1080p`, `720p`, `480p`, `360p`, `4k`).
- **Stream Preparation Script (`prepare_stream.sh`)**:
  - `./prepare_stream.sh [RES] [FPS] [CODEC]`: Transcodes MP4 to Annex-B elementary bitstream file with Access Unit Delimiters (`-aud 1`) and self-contained parameter set headers (`repeat-headers=1`).
- **Dedicated Video Streamer Script (`stream.sh`)**:
  - Supports CLI option flags (`--h264`, `--h265`, `--1080p`, `--720p`, `--fps`, `--ip`, `--port`, `--file`) and positional arguments.
  - Automatically invokes `./prepare_stream.sh` if bitstream file is not present.
- **Codec Support (H.264 & H.265 / HEVC)**: Supports streaming and decoding both H.264 and H.265 / HEVC bitstreams.
- **Real-Time Clock Overlay**: Renders active IPv4 addresses and a live digital clock (`HH:MM:SS UTC` in bright gold) updating every second while waiting for stream connections.
- **Strict Hardware Video Decoder (`/dev/video2` / `rkvdec`)**: Binds RK3588 hardware video decoder engine (`rkvdec`) with strict enforcement.
- **Tear-Free & Flicker-Free VSYNC Page Flipping**: Uses double-buffered native DRM PRIME DMA-BUF framebuffers (`Buffer 0` and `Buffer 1`) with hardware VSYNC page flipping (`card.page_flip`).

---

## Project Structure

```
.
├── Cargo.toml              # Rust crate manifest (`drm`, `nix`, `tokio`, `wtransport`)
├── Dockerfile              # Multi-stage Dockerfile with Cargo dependency layer caching
├── Makefile                # Cargo build helper
├── README.md               # User guide (this file)
├── SETUP.md                # AI Agent execution & initialization protocol
├── download_bunny.sh       # Video downloader & scaler script
├── prepare_stream.sh       # Bitstream pre-encoding script for H264 & H265
├── server.sh               # Server management & local cross-deployment script
├── stream.sh               # Video streaming client launcher with CLI flag parsing
├── deploy.sh               # Backwards-compatibility wrapper forwarding to server.sh --start
├── client/
│   ├── client.mjs          # Node.js streamer client sending H264 / H265 frames over UDP
│   └── package.json        # Client package configuration
└── src/
    ├── main.rs             # Application entry point & orchestration loop
    ├── bin/
    │   └── client.rs       # Native Rust WebTransport QUIC dev client
    ├── drm_kms.rs          # DRM card opening, mode detection & KMS display
    ├── gfx.rs              # Safe Rust 2D geometric pixel drawing
    ├── net.rs              # Safe Rust IPv4 network interface discovery
    ├── text.rs             # Safe Rust bitmap font text-to-graphics module
    ├── v4l2_decoder.rs     # RK3588 hardware video decoder & frame processing
    └── webtransport_server.rs # WebTransport QUIC UDP server module
```

---

## Quick Start Guide

1. **Start Server on Board**:
   From your local workstation terminal, run:
   ```bash
   ./server.sh --start
   ```

2. **Stream H.264 Video to Board**:
   ```bash
   ./stream.sh --h264 --1080p
   ```

3. **Stream H.265 / HEVC Video to Board**:
   ```bash
   ./stream.sh --h265 --1080p
   ```

4. **Stop Server**:
   To stop the server container on the board:
   ```bash
   ./server.sh --stop
   ```

---

## Expected Server Terminal Output

```text
[STEP 1] Opening DRM device & autodetecting display mode...
[DRM SUCCESS] Opened display card: /dev/dri/card0
[DRM] Selected 1080p HDMI display mode: 1920x1080 @ 60Hz
[DRM AUTODETECT SUCCESS] Screen Resolution: 1920x1080 @ 60Hz

[HW DECODER SUCCESS] Bound RK3588 V4L2 Hardware Video Decoder: /dev/video2
[HW DECODER ENGINE] rkvdec (Hardware H.264 / HEVC / VP9 Video Acceleration Active)

[STEP 2] Allocating Double-Buffered DRM PRIME frame memory (1920x1080)...
[DMA-BUF 0] Buffer 0 ready: fd=12, FB=framebuffer::Handle(60)
[DMA-BUF 1] Buffer 1 ready: fd=13, FB=framebuffer::Handle(61)

[NETWORK] Active IPv4 Addresses detected on device:
  - lo         : 127.0.0.1
  - end0       : 192.168.1.72

[SERVER READY] WebTransport QUIC UDP Server running on port 4433/4434.
 Displaying IPv4 Dashboard with Real-Time Clock on HDMI.
 Waiting for incoming video streams from remote client...
```
