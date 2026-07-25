# Radxa Rock 5C+ V4L2 DMA-BUF DRM Atomic Display (Rust Implementation)

A repeatable, containerized zero-copy rendering application written in **Safe Rust** for the **Radxa Rock 5C+** (Rockchip RK3588) running Armbian. It draws geometric shapes (rectangles) on an HDMI screen using a hardware-accelerated **V4L2 Decoder / M2M → DMA-BUF fd → DRM Atomic Commit → HDMI** pipeline inside Docker.

---

## Technical Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    V4L2 Decoder / M2M                       │
│  - Opens /dev/video0 (rockchip-rga / iep / rkvdec)          │
│  - Configures NV12 or ARGB frame format                      │
│  - Allocates MMAP buffer & renders central rectangle        │
│  - Exports DMA-BUF file descriptor via ioctl(VIDIOC_EXPBUF) │
└──────────────────────────────┬──────────────────────────────┘
                               │
                          DMA-BUF fd (Zero-Copy)
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│                     DRM Atomic Commit                       │
│  - Autodetects native HDMI resolution (1080p / 2K / 4K)     │
│  - Converts DMA-BUF fd to GEM handle (drmPrimeFDToHandle)   │
│  - Creates DRM Framebuffer (drmModeAddFB2)                  │
│  - Sets Atomic properties (PLANE, CRTC, MODE_ID, FB_ID)     │
│  - Commits frame directly to HDMI via drmModeAtomicCommit   │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
                         HDMI Display
```

---

## Features

- **Safe Rust Implementation**: Uses the official `drm` crate (`drm::control::Device`, `connector`, `framebuffer`, `Mode`) for safe DRM card operations and safe slice abstractions for pixel drawing.
- **Dockerized & Isolated**: Multi-stage Docker build (`rust:1.80-slim-bookworm` -> `debian:bookworm-slim`), producing a minimal runtime image without polluting host Armbian OS.
- **Dynamic Resolution Autodetection**: Queries connected HDMI display modes via DRM/KMS and sets frame buffer and CRTC dimensions automatically (supporting 1080p, 2K/2560x1440, 4K, etc.).
- **Zero-Copy Memory Pipeline**: Uses DMA-BUF file descriptors exported from V4L2 memory and imported into DRM KMS objects without CPU memory copies.
- **One-Command Deployment**: Single script (`./deploy.sh`) syncs code from host machine, builds Docker image, and displays output on screen.

---

## Project Structure

```
.
├── Cargo.toml              # Rust crate manifest (`drm`, `nix`, `libc`)
├── Dockerfile              # Multi-stage Dockerfile (Rust 1.80 builder -> Debian slim with libdrm-tests)
├── Makefile                # Cargo build helper
├── README.md               # User guide (this file)
├── SETUP.md                # AI Agent execution & initialization protocol
├── deploy.sh               # Local-to-board sync, build, and run script
└── src/
    └── main.rs             # Safe Rust application implementing V4L2 -> DMA-BUF -> DRM pipeline
```

---

## Hardware Diagnostics & DRM Inspection Commands (`modetest`)

To inspect display hardware, connectors, modes, and planes directly on the Rock 5C+:

```bash
# 1. Query connected display connectors and EDID modes (e.g. 2560x1440 @ 60Hz)
docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm modetest -M rockchip -c

# 2. Query CRTCs and Primary/Overlay Plane IDs
docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm modetest -M rockchip -p

# 3. Test hardware pattern output directly on HDMI connector (e.g. Connector ID 54)
docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm modetest -M rockchip -s 54:2560x1440
```

---

## Prerequisites

1. **Radxa Rock 5C+ Board**:
   - Running Armbian (Linux kernel 6.x, aarch64).
   - Connected to network (e.g., IP `192.168.1.72`).
   - Connected to HDMI display.
   - Docker installed (`docker.io`).
2. **Host Machine (Workstation)**:
   - Git repository checked out locally.
   - SSH key configured for passwordless access to board (`ssh-copy-id 192.168.1.72`).

---

## Quick Start Guide

1. **Edit Target IP**:
   In `deploy.sh`, adjust target IP if needed:
   ```bash
   BOARD_IP="192.168.1.72"
   ```

2. **Deploy and Run**:
   From your local workstation terminal, run:
   ```bash
   ./deploy.sh
   ```

3. **Expected Terminal Output**:
   ```text
   =====================================================
    Safe Rust Pipeline: V4L2 -> DMA-BUF fd -> DRM Atomic Commit -> HDMI
    Radxa Rock 5C+ / Rockchip RK3588 DRM Display
    Dynamic Resolution Autodetection
   =====================================================

   [STEP 1] Opening DRM device via safe `drm` crate...
   [DRM SUCCESS] Opened display card: /dev/dri/card0
   [DRM] Found connected HDMI connector: connector::Handle(54)
   [DRM] Found PREFERRED mode: 2560x1440 @ 60Hz
   [DRM AUTODETECT SUCCESS] Screen Resolution: 2560x1440 @ 60Hz (Connector: connector::Handle(54))
   [DRM] Selected CRTC: crtc::Handle(39)

   [STEP 2] Opening V4L2 device and setting target 2560x1440 resolution...
   [V4L2] Target device node: /dev/video0
   [V4L2] Driver: rockchip-rga, Card: rockchip-rga
   [V4L2] Negotiated format: XRGB8888 (2560x1440), pitch: 10240
   [INFO] Allocating native DRM PRIME DMA-BUF buffer (2560x1440)...
   [DMA-BUF SUCCESS] Created native DMA-BUF fd = 4 (2560x1440) via PRIME export

   [STEP 3] Importing DMA-BUF fd (4) into DRM Framebuffer...
   [DRM SUCCESS] Converted DMA-BUF fd (4) -> GEM Handle (1)
   [DRM SUCCESS] Created DRM Framebuffer ID = 56 (2560x1440)

   [STEP 4] Executing DRM KMS Modeset & Display on CRTC crtc::Handle(39)...

   =====================================================
    [SUCCESS] DRM KMS Display Commit Successful!
    Screen Resolution: 2560x1440 @ 60Hz
    Frame Buffer Size: 2560x1440
   =====================================================

   Displaying rectangle on HDMI screen for 10 seconds...
   Done.
   ```
