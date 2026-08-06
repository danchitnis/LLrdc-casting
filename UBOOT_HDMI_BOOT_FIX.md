# ROCK 4C+ HDMI Boot Fix

## Problem

The Radxa ROCK 4C+ could fail before Linux started when it was powered on with
an HDMI connection to the HG560T34 display. The board was unreachable over both
LAN and Tailscale until HDMI was disconnected and reconnected during boot.

This was a pre-kernel failure: SSH, networking, and Linux DRM had not started,
so changes to the receiver application could not resolve it.

## Root Cause

The installed Armbian U-Boot build included the Rockchip pre-kernel HDMI stack:

```text
CONFIG_VIDEO=y
CONFIG_DISPLAY=y
CONFIG_VIDEO_ROCKCHIP=y
CONFIG_DISPLAY_ROCKCHIP_HDMI=y
CONFIG_VIDEO_ROCKCHIP_MAX_XRES=3840
CONFIG_VIDEO_ROCKCHIP_MAX_YRES=2160
```

U-Boot reads HDMI hot-plug and EDID before loading Linux. The display's handshake
could leave that pre-kernel HDMI path stuck. Disconnecting HDMI changed the HPD
timing and let U-Boot continue, which is why the workaround appeared to repair
the board.

The failure was unrelated to the earlier temporary Linux boot argument:

```text
video=HDMI-A-1:4096x2160M@30
```

That argument created an unsupported 362.1 MHz CVT mode and was removed. It was
not the original source of the pre-kernel boot failure.

## Resolution

U-Boot was rebuilt from Armbian's `v2025.04` source configuration at commit
`34820924edbc4ec7803eb89d9852f4b870fa760a` for `rock-4c-plus-rk3399`, with all
U-Boot video and HDMI options disabled. The matching Rockchip binary firmware
is pinned at `ecb4fcbe954edf38b3ae037d5de6d9f5bccf81f4`. Linux still initializes
Rockchip DRM/KMS normally after boot, so the dashboard and video pipeline retain
HDMI output.

The modified bootloader is written only to the SD card boot area:

| Image | SD sector offset |
| --- | ---: |
| `idbloader.img` | 64 |
| `u-boot.itb` | 16384 |

The tool backs up the first 16 MiB of the SD card before changing either image.
Since this board boots from SD, removing the card and restoring or re-imaging it
from another machine remains a recovery path.

## Reproduce

From the repository root, with Docker and SSH access to the board:

```bash
./host-tools/flash_no_video_uboot.sh danial@100.100.1.72
```

The script:

1. Verifies that the board root filesystem is on `/dev/mmcblk1p1`.
2. Downloads the pinned U-Boot source and Rockchip binary firmware blobs.
3. Builds U-Boot in Docker with U-Boot video disabled.
4. Confirms that `CONFIG_VIDEO` and `CONFIG_DISPLAY_ROCKCHIP_HDMI` are absent.
5. Shows the generated image hashes and requires a `y` confirmation.
6. Uploads the images, saves a remote 16 MiB SD backup, and downloads that backup locally.
7. Requests the board user's interactive `sudo` password to flash and hash-verify both images.
8. Optionally reboots with HDMI connected and checks that no forced-mode rejection occurred.

The tool deliberately uses an interactive remote `sudo` prompt. Do not provide
the sudo password to an automation process.

## Why The Output Is 3840x2160

The display EDID contains no 4096x2160 timing. It advertises 3840x2160 at 30,
50, and 60 Hz, with a maximum TMDS rate of 600 MHz. Its preferred base timing is
3840x2400 at 60 Hz, but the RK3399 VOP-B scanout limit is 4096x2160, so Linux
correctly filters out 3840x2400 and selects 3840x2160 at 60 Hz.

The application previously displayed a misleading 4096x2160 EDID maximum due to
an incorrect CTA VIC mapping. CTA VICs 103 through 107 are 3840x2160 64:27
variants, not DCI 4096 modes; the parser has been corrected in `src/drm_kms.rs`.
