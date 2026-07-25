# Radxa Rock 5C+ WebTransport QUIC Screen Sharing Server (Rust Implementation)

A repeatable, containerized zero-copy screen sharing application written in **Safe Rust** for the **Radxa Rock 5C+** (Rockchip RK3588) running Armbian. On startup, it displays the active IPv4 address dashboard with a **live real-time clock (`HH:MM:SS UTC`)** on HDMI, then accepts incoming **WebTransport / QUIC (UDP)** video streams, decodes H.264 video frames via the hardware video decoder (`/dev/video2` / `rkvdec`), and presents them via a tear-free **V4L2 Decoder → DMA-BUF fd → DRM Atomic Commit (VSYNC Page Flip) → HDMI** pipeline inside Docker.

---

## Technical Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 Local Workstation (Client)                  │
│  - WebTransport QUIC Client over UDP                        │
│  - Streamer script: ./stream.sh                             │
│  - Connects to https://192.168.1.72:4433                     │
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
│  - Receives incoming video frame packets                    │
│  - Strictly decodes video via RK3588 rkvdec HW decoder      │
│  - Performs Double-Buffered VSYNC Page Flip (0 Tearing)     │
│  - Commits frame directly to DRM KMS -> HDMI Display        │
└─────────────────────────────────────────────────────────────┘
```

---

## Features

- **Server Control Script (`server.sh`)**:
  - `./server.sh --start` (or `--strat`): Syncs code, builds Docker container, and starts server in background.
  - `./server.sh --stop`: Cleanly stops and removes the server container on the board.
- **Dedicated Video Streamer Script (`stream.sh`)**:
  - `./stream.sh [BOARD_IP] [PORT]`: Launches 30 FPS video streamer client (`client/client.mjs`), streaming for 5 seconds and exiting cleanly.
- **Real-Time Clock Overlay**: Renders active IPv4 addresses and a live digital clock (`HH:MM:SS UTC` in bright gold) updating every second while waiting for stream connections.
- **Strict Hardware Video Decoder (`/dev/video2` / `rkvdec`)**: Binds RK3588 hardware video decoder engine (`rkvdec`) with strict enforcement—fails immediately if hardware video decoding is unavailable.
- **Tear-Free & Flicker-Free VSYNC Page Flipping**: Uses double-buffered native DRM PRIME DMA-BUF framebuffers (`Buffer 0` and `Buffer 1`) with hardware VSYNC page flipping (`card.page_flip`).
- **RK3588 Thermal & Power Safe Build**: Configured with `ENV CARGO_BUILD_JOBS=2` and `codegen-units = 16` to prevent CPU power spikes and board reboots during compilation.

---

## Project Structure

```
.
├── Cargo.toml              # Rust crate manifest (`drm`, `nix`, `tokio`, `wtransport`)
├── Dockerfile              # Multi-stage Dockerfile with Cargo dependency layer caching
├── Makefile                # Cargo build helper
├── README.md               # User guide (this file)
├── SETUP.md                # AI Agent execution & initialization protocol
├── server.sh               # Server management script (--start / --stop)
├── stream.sh               # Video streaming client launcher
├── deploy.sh               # Backwards-compatibility wrapper forwarding to server.sh --start
├── client/
│   ├── client.mjs          # Node.js dev client sending 30 FPS video stream over UDP
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

2. **Stream Video to Board (5 Seconds)**:
   In another terminal, run:
   ```bash
   ./stream.sh
   ```

3. **Stop Server**:
   To stop the server container on the board:
   ```bash
   ./server.sh --stop
   ```

---

## Expected Server Terminal Output

```text
[STEP 1] Opening DRM device & autodetecting display mode...
[DRM SUCCESS] Opened display card: /dev/dri/card0
[DRM AUTODETECT SUCCESS] Screen Resolution: 2560x1440 @ 60Hz

[HW DECODER SUCCESS] Bound RK3588 V4L2 Hardware Video Decoder: /dev/video2
[HW DECODER ENGINE] rkvdec (Hardware H.264 / HEVC / VP9 Video Acceleration Active)

[STEP 2] Allocating Double-Buffered DRM PRIME frame memory (2560x1440)...
[DMA-BUF 0] Buffer 0 ready: fd=4, FB=framebuffer::Handle(58)
[DMA-BUF 1] Buffer 1 ready: fd=5, FB=framebuffer::Handle(59)

[NETWORK] Active IPv4 Addresses detected on device:
  - lo         : 127.0.0.1
  - end0       : 192.168.1.72

[SERVER READY] WebTransport QUIC UDP Server running on port 4433/4434.
 Displaying IPv4 Dashboard with Real-Time Clock on HDMI.
 Waiting for incoming H.264 video streams from remote client...
```
