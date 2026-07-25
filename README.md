# Radxa Rock 5C+ V4L2 DMA-BUF DRM Atomic Display

A repeatable, containerized zero-copy rendering application for the **Radxa Rock 5C+** (Rockchip RK3588) running Armbian. It draws geometric shapes (rectangles) on an HDMI screen using a hardware-accelerated **V4L2 Decoder / M2M → DMA-BUF fd → DRM Atomic Commit → HDMI** pipeline inside Docker.

---

## Technical Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    V4L2 Decoder / M2M                       │
│  - Opens /dev/video0 (rockchip-iep / rga / rkvdec)           │
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

- **Dockerized & Isolated**: All compilation dependencies (`libdrm-dev`, `libv4l-dev`, `gcc`, `make`) run inside a Docker container, keeping the Armbian system clean.
- **Dynamic Resolution Autodetection**: Queries the connected HDMI display EDID/DRM mode and adjusts the frame buffer and atomic CRTC layout automatically (supporting 1080p, 2K/2560x1440, 4K, etc.).
- **Zero-Copy Memory Pipeline**: Uses DMA-BUF file descriptors exported from V4L2 memory and imported into DRM KMS objects without CPU memory copies.
- **One-Command Deployment**: Single script (`./deploy.sh`) syncs code from host machine, builds Docker image, and displays the output.

---

## Project Structure

```
.
├── Dockerfile              # Container definition (Debian Bookworm arm64)
├── Makefile                # Build rules for C binary
├── README.md               # User guide (this file)
├── SETUP.md                # AI Agent execution & initialization protocol
├── deploy.sh               # Local-to-board sync, build, and run script
└── src/
    └── v4l2_dmabuf_drm.c   # C application implementing V4L2 -> DMA-BUF -> DRM pipeline
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

1. **Clone & Edit Target IP**:
   In `deploy.sh`, adjust the target IP address if different from `192.168.1.72`:
   ```bash
   BOARD_IP="192.168.1.72"
   ```

2. **Deploy and Run**:
   From your local workstation terminal, run:
   ```bash
   ./deploy.sh
   ```

3. **Expected Output**:
   ```text
   =====================================================
    V4L2 Decoder -> DMA-BUF fd -> DRM Atomic Commit -> HDMI
    Radxa Rock 5C+ / Rockchip RK3588 DRM Display
    Dynamic Resolution Autodetection
   =====================================================

   [STEP 1] Opening DRM device and autodetecting display resolution...
   [DRM] Selected display card: /dev/dri/card0 (Driver: rockchip)
   [DRM] Found PREFERRED mode: 2560x1440 @ 60Hz
   [DRM AUTODETECT SUCCESS] Screen Resolution: 2560x1440 @ 60Hz (Connector ID: 54)

   [STEP 2] Opening V4L2 device and setting target 2560x1440 resolution...
   [V4L2] Selected V4L2 device node: /dev/video0
   [V4L2] Driver: rockchip-iep, Card: rockchip-iep
   [V4L2 SUCCESS] Exported DMA-BUF fd = 5, size = 3133440 bytes
   [DMA-BUF SUCCESS] Created native DMA-BUF fd = 5 (2560x1440) via PRIME export

   [STEP 3] Importing DMA-BUF fd (5) into DRM Framebuffer...
   [DRM SUCCESS] Converted DMA-BUF fd (5) -> GEM Handle (1)
   [DRM SUCCESS] Created DRM Framebuffer ID = 58 (2560x1440)

   [STEP 4] Executing DRM Atomic Commit on Connector 54 (CRTC 39, Plane 33)...

   =====================================================
    [SUCCESS] DRM Atomic Commit Successful!
    Screen Resolution: 2560x1440 @ 60Hz
    Frame Buffer Size: 2560x1440
   =====================================================
   ```
