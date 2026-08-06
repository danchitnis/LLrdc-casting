#!/usr/bin/env bash
# Build and flash U-Boot without its pre-kernel Rockchip HDMI stack.
set -euo pipefail

BOARD_HOST="${1:-${BOARD_HOST:-100.100.1.72}}"
SD_DEVICE="${SD_DEVICE:-/dev/mmcblk1}"
WORK_DIR="${WORK_DIR:-${TMPDIR:-/tmp}/llrdc-rock4cplus-uboot}"
UBOOT_TAG="v2025.04"
UBOOT_COMMIT="34820924edbc4ec7803eb89d9852f4b870fa760a"
RKBIN_COMMIT="ecb4fcbe954edf38b3ae037d5de6d9f5bccf81f4"
RKBIN_REPO="https://github.com/rockchip-linux/rkbin.git"
UBOOT_REPO="https://source.denx.de/u-boot/u-boot.git"
BACKUP_NAME="rock4cplus-bootloader-backup-$(date +%Y%m%d-%H%M%S).img"

usage() {
    cat <<EOF
Usage: $0 [user@host]

Builds U-Boot ${UBOOT_TAG} for the Radxa ROCK 4C+ with all U-Boot video/HDMI
support disabled, then interactively backs up and flashes the boot sectors on
the board's SD card.

Environment overrides:
  BOARD_HOST  SSH host when no positional host is supplied
  SD_DEVICE   Board SD device (default: ${SD_DEVICE})
  WORK_DIR    Local build directory (default: ${WORK_DIR})
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    usage
    exit 0
fi

require_command() {
    command -v "$1" >/dev/null || {
        echo "Missing required command: $1" >&2
        exit 1
    }
}

for command in docker git scp sha256sum ssh; do
    require_command "$command"
done

remote() {
    ssh -o BatchMode=yes "$BOARD_HOST" "$@"
}

echo "== ROCK 4C+ no-video U-Boot recovery =="
echo "Board:       ${BOARD_HOST}"
echo "SD device:   ${SD_DEVICE}"
echo "Build path:  ${WORK_DIR}"
echo
echo "This removes U-Boot HDMI initialization only. Linux DRM/KMS HDMI output"
echo "remains enabled after the kernel starts."
echo

echo "[1/7] Checking the board and SD boot device..."
REMOTE_HOME="$(remote 'printf %s "$HOME"')"
root_source="$(remote 'findmnt -no SOURCE /')"
if [[ "$root_source" != "${SD_DEVICE}p1" ]]; then
    echo "Refusing to flash: root filesystem is ${root_source}, expected ${SD_DEVICE}p1." >&2
    exit 1
fi
remote "test -b '${SD_DEVICE}'"
echo "Root filesystem verified on ${root_source}."

mkdir -p "$WORK_DIR"

if [[ ! -d "$WORK_DIR/u-boot/.git" ]]; then
    echo "[2/7] Downloading U-Boot ${UBOOT_TAG}..."
    git clone --depth 1 --branch "$UBOOT_TAG" "$UBOOT_REPO" "$WORK_DIR/u-boot"
fi
git -C "$WORK_DIR/u-boot" fetch --depth 1 origin "refs/tags/${UBOOT_TAG}:refs/tags/${UBOOT_TAG}"
git -C "$WORK_DIR/u-boot" checkout --detach "$UBOOT_COMMIT"

if [[ ! -d "$WORK_DIR/rkbin/.git" ]]; then
    echo "[3/7] Downloading Rockchip DDR and trusted-firmware blobs..."
    git clone --depth 1 "$RKBIN_REPO" "$WORK_DIR/rkbin"
fi
git -C "$WORK_DIR/rkbin" fetch --depth 1 origin "$RKBIN_COMMIT"
git -C "$WORK_DIR/rkbin" checkout --detach "$RKBIN_COMMIT"

cat >"$WORK_DIR/Dockerfile" <<'EOF'
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    bc bison build-essential ca-certificates flex gcc-aarch64-linux-gnu \
    libgnutls28-dev libssl-dev python3 python3-dev python3-pyelftools \
    python3-setuptools swig \
    && rm -rf /var/lib/apt/lists/*
EOF

echo "[4/7] Building no-video U-Boot in Docker..."
docker build -t llrdc-rock4cplus-uboot-build -f "$WORK_DIR/Dockerfile" "$WORK_DIR"
docker run --rm \
    -v "$WORK_DIR/u-boot:/src" \
    -v "$WORK_DIR/rkbin:/rkbin:ro" \
    -w /src \
    llrdc-rock4cplus-uboot-build \
    bash -lc '
        make ARCH=arm CROSS_COMPILE=aarch64-linux-gnu- rock-4c-plus-rk3399_defconfig
        scripts/config --disable VIDEO --disable DISPLAY --disable VIDEO_ROCKCHIP \
            --disable DISPLAY_ROCKCHIP_HDMI --disable VIDEO_DW_HDMI
        make ARCH=arm CROSS_COMPILE=aarch64-linux-gnu- olddefconfig
        make -j"$(nproc)" ARCH=arm CROSS_COMPILE=aarch64-linux-gnu- \
            ROCKCHIP_TPL=/rkbin/bin/rk33/rk3399_ddr_933MHz_v1.30.bin \
            BL31=/rkbin/bin/rk33/rk3399_bl31_v1.36.elf
        ! grep -q "^CONFIG_VIDEO=y" .config
        ! grep -q "^CONFIG_DISPLAY_ROCKCHIP_HDMI=y" .config
    '

idb_image="$WORK_DIR/u-boot/idbloader.img"
uboot_image="$WORK_DIR/u-boot/u-boot.itb"
test -s "$idb_image"
test -s "$uboot_image"

read -r idb_hash _ < <(sha256sum "$idb_image")
read -r uboot_hash _ < <(sha256sum "$uboot_image")
idb_blocks=$(( $(wc -c <"$idb_image") / 512 ))
uboot_blocks=$(( $(wc -c <"$uboot_image") / 512 ))

if (( $(wc -c <"$idb_image") % 512 != 0 || $(wc -c <"$uboot_image") % 512 != 0 )); then
    echo "Generated boot images are not sector-aligned." >&2
    exit 1
fi
if (( uboot_blocks > 8192 )); then
    echo "Generated U-Boot image exceeds its 4 MiB SD boot region." >&2
    exit 1
fi

echo "[5/7] Generated images:"
echo "  idbloader.img: ${idb_hash} (${idb_blocks} sectors)"
echo "  u-boot.itb:    ${uboot_hash} (${uboot_blocks} sectors)"

read -r -p "Flash this U-Boot to ${BOARD_HOST}:${SD_DEVICE}? [y/N] " answer
if [[ "$answer" != "y" && "$answer" != "Y" ]]; then
    echo "Cancelled before any board changes."
    exit 0
fi

echo "[6/7] Uploading images and backing up the first 16 MiB of the SD card..."
scp -o BatchMode=yes "$idb_image" "$uboot_image" "${BOARD_HOST}:${REMOTE_HOME}/"
remote "dd if='${SD_DEVICE}' of='${REMOTE_HOME}/${BACKUP_NAME}' bs=512 count=32768 status=none && sha256sum '${REMOTE_HOME}/${BACKUP_NAME}'"
scp -o BatchMode=yes "${BOARD_HOST}:${REMOTE_HOME}/${BACKUP_NAME}" "$WORK_DIR/${BACKUP_NAME}"

echo "The backup is stored locally at: $WORK_DIR/${BACKUP_NAME}"
echo "An interactive sudo prompt will now flash the SD boot sectors."

ssh -t "$BOARD_HOST" "sudo sh -s -- '${SD_DEVICE}' '${REMOTE_HOME}' '${idb_hash}' '${uboot_hash}' '${idb_blocks}' '${uboot_blocks}'" <<'REMOTE_SCRIPT'
set -eu
device="$1"
remote_home="$2"
idb_hash="$3"
uboot_hash="$4"
idb_blocks="$5"
uboot_blocks="$6"

dd if="$remote_home/idbloader.img" of="$device" bs=512 seek=64 conv=fsync status=progress
dd if="$remote_home/u-boot.itb" of="$device" bs=512 seek=16384 conv=fsync status=progress
# U-Boot reserves 4 MiB from sector 16384; clear stale bytes from a larger prior image.
dd if=/dev/zero of="$device" bs=512 seek=$((16384 + uboot_blocks)) count=$((8192 - uboot_blocks)) conv=fsync status=progress
sync

dd if="$device" bs=512 skip=64 count="$idb_blocks" status=none | sha256sum | grep -F "$idb_hash"
dd if="$device" bs=512 skip=16384 count="$uboot_blocks" status=none | sha256sum | grep -F "$uboot_hash"
REMOTE_SCRIPT

echo "[7/7] Flash verified. Keep HDMI connected and reboot the board."
read -r -p "Reboot now? [y/N] " answer
if [[ "$answer" == "y" || "$answer" == "Y" ]]; then
    ssh -t "$BOARD_HOST" 'sudo reboot'
    echo "Waiting for SSH to return..."
    until remote 'true' 2>/dev/null; do sleep 2; done
    remote "printf 'Kernel command line: '; tr '\0' ' ' </proc/cmdline; if journalctl -b -k --no-pager | grep -q 'User-defined mode not supported'; then exit 1; fi"
    echo "Boot verification passed."
fi
