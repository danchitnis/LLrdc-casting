# Agent Setup Instructions: Radxa ROCK 4C+ (RK3399) V4L2 DRM Pipeline Initialization (Rust)

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

### Step 2: Install & Prepare Docker Engine on Target Board
Ensure `docker.io` and `docker-cli` are installed, the Docker service is active, and the SSH user belongs to the `docker` group.

Check Docker installation status:
```bash
ssh <BOARD_IP> "docker info"
```

If Docker or `docker-cli` is missing or user permissions are required, ask the user for `sudo` execution:
```bash
ssh -t <USER>@<BOARD_IP> "sudo apt update && sudo apt install -y docker.io docker-cli && sudo systemctl enable --now docker && sudo usermod -aG docker \$USER"
```

---

### Step 3: Inspect Display Subsystem, V4L2 Hardware Decoders & DRM Modes (`modetest`)
Inspect available DRM graphics cards, V4L2 hardware video nodes (`rkvdec`), and display connectors:

```bash
# 1. Query DRM card devices and driver paths
ssh <BOARD_IP> "ls -l /dev/dri/card* /dev/dri/render*"

# 2. Query active connectors, EDIDs, and supported screen modes (e.g., 1080p, 2K)
ssh <BOARD_IP> "docker run --rm --privileged -v /dev:/dev llrdc-casting modetest -M rockchip -c"

# 3. List V4L2 hardware video nodes
ssh <BOARD_IP> "ls -l /dev/video*"
```

*Target Drivers & Output*:
- DRM display card: `/dev/dri/card0` (Driver: `rockchip`)
- Active HDMI Connector ID: `54` (`HDMI-A-1`; current mode `3840x2160 @ 60Hz`)
- RK3399 Hardware Video Decoder Node: `/dev/video2` (`rkvdec`, V4L2 stateless)

---

### Step 4: Launch LLrdc Casting Server
To build the Docker image locally and transfer it to the board in background mode:

```bash
./server.sh --start
```

Retrieve the active local pairing code over SSH without Cloudflare:

```bash
pairing_code="$(./server.sh --get-pairing-code)"
```

For deliberate local stress testing, a fixed code can be selected for one
deployment:

```bash
CLOUD_DISCOVERY_ENABLED=0 ./server.sh --start --pairing-code=0000
```

Random rotating codes remain the default.

To stop the server container on the board:

```bash
./server.sh --stop
```

---

### Step 5: Execute Video Streamer Test Client

Run the HEVC smoke test at 1080p:

```bash
./test.sh --1080p --fps 60 --duration 20
```

---

### Step 6: Verify Pipeline Execution Logs
Verify that the output logs report success across all server stages:

```text
[STEP 1] Opening DRM device & autodetecting display mode...
[DRM SUCCESS] Opened display card: /dev/dri/card0
[DRM] Selected highest capacity HDMI mode: 3840x2160 @ 60Hz
[DRM AUTODETECT SUCCESS] Screen Resolution: 3840x2160 @ 60Hz

[IDLE DASHBOARD] HDMI IP screen active; waiting for HEVC stream.
[READY] waiting for H.265 UDP access units on port 4434
[PLAYBACK READY] HEVC -> v4l2slh265dec -> HDMI connector 54, plane 33
```
