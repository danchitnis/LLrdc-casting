# Agent Setup Instructions: Radxa Rock 5C+ V4L2 DRM Pipeline Initialization (Rust)

This document outlines the step-by-step workflow for an **AI Agent** or automated system initializing and deploying the safe Rust V4L2 DMA-BUF DRM Atomic pipeline on a fresh/blank Armbian installation upon acquiring SSH access.

---

## Agent Operational Rules

1. **Local Files First**: All project source files (`src/main.rs`, `Cargo.toml`, `Dockerfile`, `Makefile`, `deploy.sh`) must be maintained locally in the host workstation Git repository.
2. **No Sudo Workarounds**: If `sudo` privileges are required on the remote target board, request them from the user rather than bypassing security mechanisms.
3. **Container Isolation**: Compile and run all rendering applications inside a Docker container (`--privileged -v /dev:/dev`) to keep the host Armbian system clean.
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

### Step 2: Inspect Display Subsystem, V4L2 Devices & DRM Modes (`modetest`)
Inspect available DRM graphics cards, V4L2 video nodes, and display connectors using `modetest`:

```bash
# 1. Query DRM card devices and driver paths
ssh <BOARD_IP> "ls -l /dev/dri/card* /dev/dri/render*"

# 2. Query active connectors, EDIDs, and supported screen modes (e.g., 2560x1440 2K)
ssh <BOARD_IP> "docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm modetest -M rockchip -c"

# 3. Query active CRTCs and Primary/Overlay Plane IDs
ssh <BOARD_IP> "docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm modetest -M rockchip -p"

# 4. List V4L2 hardware video nodes
ssh <BOARD_IP> "ls -l /dev/video*"
```

*Target Drivers & Output*:
- DRM display card: `/dev/dri/card0` (Driver: `rockchip`)
- Active HDMI Connector ID: e.g. `54` (`HDMI-A-1`, preferred mode `2560x1440 @ 59.95Hz`)
- Primary Plane ID: e.g. `33` or `17` bound to CRTC `39`
- V4L2 nodes: `/dev/video0` (`rockchip-rga` / `iep`), `/dev/video2` (`rkvdec`)

---

### Step 3: Verify & Initialize Docker Environment on Remote Target

1. **Check Docker Status**:
   ```bash
   ssh <BOARD_IP> "docker --version && systemctl is-active docker"
   ```

2. **If Docker or `rsync` is Missing**:
   Ask user for permissions or execute:
   ```bash
   ssh <BOARD_IP> "sudo apt update && sudo apt install -y docker.io rsync v4l-utils libdrm-tests && sudo systemctl enable --now docker"
   ```

3. **Ensure Group Membership**:
   Confirm current remote user belongs to `docker`, `video`, and `render` groups:
   ```bash
   ssh <BOARD_IP> "sudo usermod -aG docker,video,render \$USER"
   ```

---

### Step 4: Synchronize Repository to Remote Board
Sync the local repository workspace to the remote target directory `~/rock5c-v4l2-drm`:

```bash
rsync -avz --exclude '.git' . <BOARD_IP>:~/rock5c-v4l2-drm
```

---

### Step 5: Build Container Image Remotely
Build the multi-stage Rust Docker image on the board containing `libdrm-dev` and `libv4l-dev`:

```bash
ssh <BOARD_IP> "cd ~/rock5c-v4l2-drm && docker build -t rock5c-v4l2-drm ."
```

---

### Step 6: Run Container & Execute Hardware Pipeline
Execute the containerized Rust pipeline with hardware device access:

```bash
ssh <BOARD_IP> "docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm"
```

---

### Step 7: Verify Pipeline Execution Logs
Verify that the output logs report success across all four pipeline stages and list active IPv4 addresses:

```text
=====================================================
 Safe Rust Pipeline: V4L2 -> DMA-BUF -> DRM
 Radxa Rock 5C+ / Rockchip RK3588 DRM Display
=====================================================

[STEP 1] Opening DRM device & autodetecting display mode...
[DRM SUCCESS] Opened display card: /dev/dri/card0
[DRM] Found connected HDMI connector: connector::Handle(54)
[DRM] Found PREFERRED mode: 2560x1440 @ 60Hz
[DRM AUTODETECT SUCCESS] Screen Resolution: 2560x1440 @ 60Hz (Connector: connector::Handle(54), CRTC: crtc::Handle(39))

[STEP 2] Allocating & exporting V4L2 DMA-BUF frame memory...
[V4L2] Driver: rockchip-rga, Card: rockchip-rga
[V4L2] Negotiated format: XRGB8888 (2560x1440), pitch: 10240
[DMA-BUF SUCCESS] Created native DMA-BUF fd = 4 (2560x1440) via PRIME export

[NETWORK] Active IPv4 Addresses detected on device:
  - lo         : 127.0.0.1
  - end0       : 192.168.1.72
  - docker0    : 172.17.0.1

[STEP 3] Importing DMA-BUF fd (4) into DRM Framebuffer...
[DRM SUCCESS] Converted DMA-BUF fd (4) -> GEM Handle (1)
[DRM SUCCESS] Created DRM Framebuffer Handle = framebuffer::Handle(60) (2560x1440)

[STEP 4] Executing DRM KMS Modeset & Display on CRTC crtc::Handle(39)...

=====================================================
 [SUCCESS] DRM KMS Display Commit Successful!
 Screen Resolution: 2560x1440 @ 60Hz
 Frame Buffer Size: 2560x1440
=====================================================

Displaying active device IP addresses on HDMI screen for 10 seconds...
Done.
```
