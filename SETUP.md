# Agent Setup Instructions: Radxa Rock 5C+ V4L2 DRM Pipeline Initialization (Rust)

This document outlines the step-by-step workflow for an **AI Agent** or automated system initializing and deploying the safe Rust WebTransport V4L2 DMA-BUF DRM Atomic pipeline on a fresh/blank Armbian installation upon acquiring SSH access.

---

## Agent Operational Rules

1. **Local Files First**: All project source files (`src/main.rs`, `Cargo.toml`, `Dockerfile`, `Makefile`, `server.sh`, `stream.sh`, `download_bunny.sh`, `prepare_stream.sh`) must be maintained locally in the host workstation Git repository.
2. **Local Workstation Cross-Build**: Docker images should be built locally on the host workstation (`docker build --platform linux/arm64`) and transferred to the board via compressed stream (`docker save | gzip -1 | ssh ... docker load`) to prevent RAM exhaustion and thermal throttling on the Rock Pi board.
3. **No Sudo Workarounds**: If `sudo` privileges are required on the remote target board, request them from the user rather than bypassing security mechanisms.
4. **Container Isolation**: Compile and run all rendering applications inside a Docker container (`--net=host --privileged -v /dev:/dev`) to keep the host Armbian system clean.
5. **No Git Commits Without Explicit User Permission**: Only stage/commit when explicitly requested by the user.

---

## Agent Execution Workflow Once SSH Access is Established

### Step 1: Verify Remote Connectivity & System Architecture
Run a quick connectivity check to identify kernel version and CPU architecture:

```bash
ssh <BOARD_IP> "uname -a"
```
*Expected Output*: Linux kernel `6.x` on `aarch64`.

---

### Step 2: Inspect Display Subsystem, V4L2 Hardware Decoders & DRM Modes (`modetest`)
Inspect available DRM graphics cards, V4L2 hardware video nodes (`rkvdec`), and display connectors:

```bash
# 1. Query DRM card devices and driver paths
ssh <BOARD_IP> "ls -l /dev/dri/card* /dev/dri/render*"

# 2. Query active connectors, EDIDs, and supported screen modes (e.g., 1080p, 2K)
ssh <BOARD_IP> "docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm modetest -M rockchip -c"

# 3. List V4L2 hardware video nodes
ssh <BOARD_IP> "ls -l /dev/video*"
```

*Target Drivers & Output*:
- DRM display card: `/dev/dri/card0` (Driver: `rockchip`)
- Active HDMI Connector ID: e.g. `54` (`HDMI-A-1`, preferred mode `1920x1080 @ 60Hz`)
- RK3588 Hardware Video Decoder Node: `/dev/video2` (`rkvdec`)

---

### Step 3: Launch WebTransport Screen Sharing Server
To build the Docker image locally and transfer it to the board in background mode:

```bash
./server.sh --start
```

To stop the server container on the board:

```bash
./server.sh --stop
```

---

### Step 4: Execute Video Streamer Test Client

To stream H.264 video at 1080p:

```bash
./stream.sh --h264 --1080p
```

To stream H.265 / HEVC video at 1080p:

```bash
./stream.sh --h265 --1080p
```

---

### Step 5: Verify Pipeline Execution Logs
Verify that the output logs report success across all server stages:

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
