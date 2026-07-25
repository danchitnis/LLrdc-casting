#!/usr/bin/env bash
set -e

# Big Buck Bunny Downloader Script
# Usage: ./download_bunny.sh [RESOLUTION] or ./download_bunny.sh [OPTIONS]
# Example: ./download_bunny.sh --1080p
# Example: ./download_bunny.sh 720p

RAW_RES="1080p"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ASSETS_DIR="${SCRIPT_DIR}/client/assets"

mkdir -p "$ASSETS_DIR"

POSITIONAL_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      echo "Usage: ./download_bunny.sh [OPTIONS] [RESOLUTION]"
      echo ""
      echo "Options:"
      echo "  --1080p, --720p, --4k  Resolution presets"
      echo "  -r, --res, --resolution Set resolution (e.g. 1920x1080, 1080p)"
      echo "  -h, --help             Display this help message"
      exit 0
      ;;
    --1080p|--1080)
      RAW_RES="1080p"
      shift
      ;;
    --720p|--720)
      RAW_RES="720p"
      shift
      ;;
    --480p|--480)
      RAW_RES="480p"
      shift
      ;;
    --360p|--360)
      RAW_RES="360p"
      shift
      ;;
    --4k|--2160p)
      RAW_RES="4k"
      shift
      ;;
    -r|--res|--resolution)
      RAW_RES="$2"
      shift 2
      ;;
    --res=*|--resolution=*)
      RAW_RES="${1#*=}"
      shift
      ;;
    *)
      POSITIONAL_ARGS+=("$1")
      shift
      ;;
  esac
done

if [ ${#POSITIONAL_ARGS[@]} -gt 0 ]; then
  RAW_RES="${POSITIONAL_ARGS[0]}"
fi

# Parse resolution and normalize dimensions
case "$RAW_RES" in
  1080p|1080|1920x1080)
    RES_NAME="1080p"
    WIDTH=1920
    HEIGHT=1080
    ;;
  720p|720|1280x720)
    RES_NAME="720p"
    WIDTH=1280
    HEIGHT=720
    ;;
  480p|480|854x480|848x480)
    RES_NAME="480p"
    WIDTH=854
    HEIGHT=480
    ;;
  360p|360|640x360)
    RES_NAME="360p"
    WIDTH=640
    HEIGHT=360
    ;;
  4k|2160p|2160|3840x2160)
    RES_NAME="4k"
    WIDTH=3840
    HEIGHT=2160
    ;;
  *x*)
    WIDTH=$(echo "$RAW_RES" | cut -d'x' -f1)
    HEIGHT=$(echo "$RAW_RES" | cut -d'x' -f2)
    RES_NAME="${WIDTH}x${HEIGHT}"
    ;;
  *)
    echo "[WARN] Unknown resolution identifier '${RAW_RES}'. Defaulting to ${RAW_RES} (scaling if needed)."
    RES_NAME="$RAW_RES"
    WIDTH=1280
    HEIGHT=720
    ;;
esac

OUTPUT_FILE="${ASSETS_DIR}/bigbuckbunny_${RES_NAME}.mp4"
MASTER_1080P="${ASSETS_DIR}/bigbuckbunny_1080p.mp4"

echo "====================================================="
echo " Big Buck Bunny Downloader"
echo " Target Resolution: ${RES_NAME} (${WIDTH}x${HEIGHT})"
echo " Target File      : ${OUTPUT_FILE}"
echo "====================================================="

if [ -f "$OUTPUT_FILE" ] && [ -s "$OUTPUT_FILE" ]; then
  echo "[SUCCESS] File already exists: $OUTPUT_FILE"
  exit 0
fi

# Primary source URL for Big Buck Bunny 1080p MP4
SOURCE_URL="https://commondatastorage.googleapis.com/gtv-videos-bucket/sample/BigBuckBunny.mp4"

# Download 1080p master file if missing
if [ ! -f "$MASTER_1080P" ] || [ ! -s "$MASTER_1080P" ]; then
  echo "[DOWNLOAD] Fetching Big Buck Bunny 1080p source video from CDN..."
  if command -v curl >/dev/null 2>&1; then
    curl -L -o "$MASTER_1080P" "$SOURCE_URL"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$MASTER_1080P" "$SOURCE_URL"
  else
    echo "[ERROR] Neither curl nor wget is available to download video."
    exit 1
  fi
  echo "[DOWNLOAD] Master 1080p video saved to $MASTER_1080P"
fi

if [ "$RES_NAME" = "1080p" ] || [ "$WIDTH" -eq 1920 -a "$HEIGHT" -eq 1080 ]; then
  if [ "$OUTPUT_FILE" != "$MASTER_1080P" ]; then
    cp "$MASTER_1080P" "$OUTPUT_FILE"
  fi
  echo "[SUCCESS] Big Buck Bunny 1080p ready at $OUTPUT_FILE"
  exit 0
fi

# Locate ffmpeg executable
FFMPEG_BIN=""
if [ -f "${SCRIPT_DIR}/client/node_modules/ffmpeg-static/ffmpeg" ]; then
  FFMPEG_BIN="${SCRIPT_DIR}/client/node_modules/ffmpeg-static/ffmpeg"
elif command -v ffmpeg >/dev/null 2>&1; then
  FFMPEG_BIN="ffmpeg"
else
  echo "[ERROR] ffmpeg binary not found."
  exit 1
fi

echo "[FFMPEG SCALE] Scaling Big Buck Bunny to ${WIDTH}x${HEIGHT} (${RES_NAME})..."
"$FFMPEG_BIN" -y -i "$MASTER_1080P" -vf "scale=${WIDTH}:${HEIGHT}" -c:v libx264 -preset fast -crf 20 -c:a copy "$OUTPUT_FILE"

echo "[SUCCESS] Generated ${RES_NAME} video: ${OUTPUT_FILE}"
