# Agent Setup Instructions: Radxa Rock 5C+ V4L2 DRM Pipeline Initialization

This document outlines the step-by-step workflow for an **AI Agent** or automated system initializing and deploying the V4L2 DMA-BUF DRM Atomic pipeline on a fresh/blank Armbian installation upon acquiring SSH access.

---

## Agent Operational Rules

1. **Local Files First**: All project source files (`src/v4l2_dmabuf_drm.c`, `Dockerfile`, `Makefile`, `deploy.sh`) must be maintained locally in the host workstation Git repository.
2. **No Sudo Workarounds**: If `sudo` privileges are required on the remote target board, request them from the user rather than bypassing security mechanisms.
3. **Container Isolation**: Compile and run all rendering applications inside a Docker container (`--privileged -v /dev:/dev`) to keep the host Armbian system clean.

---

## Agent Execution Workflow Once SSH Access is Established

### Step 1: Verify Remote Connectivity & System Architecture
Run a quick connectivity check to identify kernel version and CPU architecture:

```bash
ssh <BOARD_IP> "uname -a"
```
*Expected Output*: Linux kernel `6.x` on `aarch64`.

---

### Step 2: Inspect Display Subsystem & V4L2 Hardware Devices
Inspect available DRM graphics cards and V4L2 video nodes:

```bash
# Query DRM card devices and driver drivers
ssh <BOARD_IP> "ls -l /dev/dri/card* /dev/dri/render*"

# Check connected connector status
ssh <BOARD_IP> "cat /sys/class/drm/card*-*/status"

# List V4L2 hardware video nodes
ssh <BOARD_IP> "ls -l /dev/video*"
```
*Target Drivers*:
- DRM display card: `/dev/dri/card0` (Driver: `rockchip`)
- V4L2 nodes: `/dev/video0` (`rockchip-iep`), `/dev/video1` (`rockchip-rga`), `/dev/video2` (`rkvdec`)

---

### Step 3: Verify & Initialize Docker Environment on Remote Target

1. **Check Docker Status**:
   ```bash
   ssh <BOARD_IP> "docker --version && systemctl is-active docker"
   ```

2. **If Docker or `rsync` is Missing**:
   Ask user for permissions or execute:
   ```bash
   ssh <BOARD_IP> "sudo apt update && sudo apt install -y docker.io rsync v4l-utils && sudo systemctl enable --now docker"
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
Build the ARM64 Docker image on the board containing `libdrm-dev` and `libv4l-dev`:

```bash
ssh <BOARD_IP> "cd ~/rock5c-v4l2-drm && docker build -t rock5c-v4l2-drm ."
```

---

### Step 6: Run Container & Execute Hardware Pipeline
Execute the containerized pipeline with hardware device access:

```bash
ssh <BOARD_IP> "docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm"
```

---

### Step 7: Verify Pipeline Execution Logs
Verify that the output logs report success across all four pipeline stages:

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
[V4L2] Negotiated format: NV12 (1920x1088), pitch: 1920
[V4L2 SUCCESS] Exported DMA-BUF fd = 5, size = 3133440 bytes
[V4L2] Drawn rectangle on V4L2 DMA-BUF frame memory (1920x1088).
[INFO] Allocating native DRM PRIME DMA-BUF buffer (2560x1440)...
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
