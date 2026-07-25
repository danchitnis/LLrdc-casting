# Agent Setup Instructions: Radxa Rock 5C+ V4L2 DRM Pipeline Initialization (Rust)

This document outlines the step-by-step workflow for an **AI Agent** or automated system initializing and deploying the safe Rust WebTransport V4L2 DMA-BUF DRM Atomic pipeline on a fresh/blank Armbian installation upon acquiring SSH access.

---

## Agent Operational Rules

1. **Local Files First**: All project source files (`src/main.rs`, `Cargo.toml`, `Dockerfile`, `Makefile`, `server.sh`, `stream.sh`) must be maintained locally in the host workstation Git repository.
2. **No Sudo Workarounds**: If `sudo` privileges are required on the remote target board, request them from the user rather than bypassing security mechanisms.
3. **Container Isolation**: Compile and run all rendering applications inside a Docker container (`--net=host --privileged -v /dev:/dev`) to keep the host Armbian system clean.
4. **No Git Commits Without Explicit User Permission**: Only stage/commit when explicitly requested by the user.

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

# 2. Query active connectors, EDIDs, and supported screen modes (e.g., 2560x1440 2K)
ssh <BOARD_IP> "docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm modetest -M rockchip -c"

# 3. List V4L2 hardware video nodes
ssh <BOARD_IP> "ssh <BOARD_IP> ls -l /dev/video*"
```

*Target Drivers & Output*:
- DRM display card: `/dev/dri/card0` (Driver: `rockchip`)
- Active HDMI Connector ID: e.g. `54` (`HDMI-A-1`, preferred mode `2560x1440 @ 60Hz`)
- RK3588 Hardware Video Decoder Node: `/dev/video2` (`rkvdec`)

---

### Step 3: Launch WebTransport Screen Sharing Server
To sync repository workspace, build Docker image, and start the server on board in background mode:

```bash
./server.sh --start
```

To stop the server container on the board:

```bash
./server.sh --stop
```

---

### Step 4: Execute Video Streamer Test Client
To stream 30 FPS video to the board for 5 seconds:

```bash
./stream.sh
```

---

### Step 5: Verify Pipeline Execution Logs
Verify that the output logs report success across all server stages:

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
