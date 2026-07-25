# Radxa Rock 5C+ WebTransport QUIC Screen Sharing Server (Rust Implementation)

A repeatable, containerized zero-copy screen sharing application written in **Safe Rust** for the **Radxa Rock 5C+** (Rockchip RK3588) running Armbian. It displays the active IPv4 address dashboard on HDMI for **1 second** on startup, then accepts incoming **WebTransport / QUIC (UDP)** streams, decodes H.264 video frames, and presents them via a zero-copy **V4L2 Decoder → DMA-BUF fd → DRM Atomic Commit → HDMI** pipeline inside Docker.

---

## Technical Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                 Local Workstation (Client)                  │
│  - WebTransport QUIC Client over UDP                        │
│  - Transmits static H.264 video frame (Annex-B NAL units)   │
│  - Connects to https://192.168.1.72:4433                     │
└──────────────────────────────┬──────────────────────────────┘
                               │
                      WebTransport / QUIC (UDP)
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                 Radxa Rock 5C+ Server (Board)               │
│                                                             │
│  Phase 1 (Startup - 1s):                                    │
│  - Renders IPv4 Address Dashboard on HDMI for 1.0 second    │
│                                                             │
│  Phase 2 (1s - Streaming):                                  │
│  - WebTransport Server accepts QUIC stream over UDP:4433    │
│  - Receives incoming H.264 frame payload                    │
│  - Decodes H.264 video frame via RK3588 hardware pipeline   │
│  - Exports decoded NV12 / XRGB frame as DMA-BUF fd          │
│  - Commits DMA-BUF directly to DRM KMS -> HDMI Display      │
└─────────────────────────────────────────────────────────────┘
```

---

## Features

- **WebTransport / QUIC over UDP Server (`src/webtransport_server.rs`)**: Async QUIC UDP server on port `4433` using `wtransport` and self-signed TLS certificates (`rcgen`).
- **1-Second Startup IP Screen**: Autodetects native 2K HDMI display (`2560x1440 @ 60Hz`) and displays active device IPv4 addresses for 1 second before accepting video streams.
- **H.264 Video Decoder (`src/v4l2_decoder.rs`)**: Processes incoming H.264 Annex-B NAL unit payloads and updates zero-copy DMA-BUF frame memory.
- **Dev Clients (`client/client.mjs` & `src/bin/client.rs`)**: Node.js and Rust dev clients for sending static H.264 video frames over UDP.
- **RK3588 Thermal & Power Safe Build**: Configured with `ENV CARGO_BUILD_JOBS=2` and `codegen-units = 16` to prevent CPU power spikes and board reboots during compilation.
- **One-Command Deployment**: Single script (`./deploy.sh`) syncs code, builds Docker container, restarts server in background, and executes the dev client transmission.

---

## Project Structure

```
.
├── Cargo.toml              # Rust crate manifest (`drm`, `nix`, `tokio`, `wtransport`)
├── Dockerfile              # Multi-stage Dockerfile with Cargo dependency layer caching
├── Makefile                # Cargo build helper
├── README.md               # User guide (this file)
├── SETUP.md                # AI Agent execution & initialization protocol
├── deploy.sh               # Local-to-board sync, build, run & client test script
├── client/
│   ├── client.mjs          # Node.js dev client sending static H.264 frame via UDP
│   └── package.json        # Client package configuration
└── src/
    ├── main.rs             # Application entry point & orchestration loop
    ├── bin/
    │   └── client.rs       # Native Rust WebTransport QUIC dev client
    ├── drm_kms.rs          # DRM card opening, mode detection & KMS display
    ├── gfx.rs              # Safe Rust 2D geometric pixel drawing
    ├── net.rs              # Safe Rust IPv4 network interface discovery
    ├── text.rs             # Safe Rust bitmap font text-to-graphics module
    ├── v4l2.rs             # V4L2 buffer allocation & DMA-BUF export
    ├── v4l2_decoder.rs     # H.264 video stream frame processing & rendering
    └── webtransport_server.rs # WebTransport QUIC UDP server module
```

---

## Quick Start Guide

1. **Deploy Server & Transmit Test Frame**:
   From your local workstation terminal, run:
   ```bash
   ./deploy.sh
   ```

2. **Expected Terminal Output**:
   ```text
   ==> 3. Restarting Docker container on board in background mode...
   ==> 4. Waiting 2 seconds for server initialization and 1-second IP screen display...
   ==> 5. Executing local WebTransport client to transmit static H.264 frame to 192.168.1.72:4433...
   =====================================================
    WebTransport QUIC UDP H.264 Dev Client (Node.js)
    Target Server: 192.168.1.72:4433 (UDP)
   =====================================================

   [CLIENT SUCCESS] Transmitted 70 bytes of static H.264 frame to 192.168.1.72:4433
   ==> 6. Fetching board container server logs...
   [SUCCESS] DRM KMS Display Active!
   [TIMING] Displaying IPv4 Address Dashboard on HDMI for 1 second...
   [SERVER READY] WebTransport QUIC UDP Server running on port 4433.
   [WEBTRANSPORT SERVER] Listening on UDP 0.0.0.0:4433
   [DECODER] Processing H.264 NAL unit payload (70 bytes) for screen (2560x1440)...
   ==> Deployment & frame transmission complete!
   ```
