#!/usr/bin/env bash
set -e

# Prepare Streamable Bitstream Video Script
# Supports flags (--h264, --h265, --1080p, --720p, --fps, --codec, --res, --input)
# as well as positional arguments.
#
# Examples:
#   ./prepare_stream.sh --h265 --1080p
#   ./prepare_stream.sh 1280x720 30 H264
#   ./prepare_stream.sh -r 1080p -f 60 -c H265

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ASSETS_DIR="${SCRIPT_DIR}/client/assets"

mkdir -p "$ASSETS_DIR"

RAW_RES="1280x720"
FPS="30"
RAW_CODEC="H264"
CUSTOM_INPUT=""

POSITIONAL_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      echo "Usage: ./prepare_stream.sh [OPTIONS] [RES] [FPS] [CODEC] [INPUT_FILE]"
      echo ""
      echo "Options:"
      echo "  --h264                 Encode using H.264 video codec"
      echo "  --h265, --hevc         Encode using H.265 / HEVC video codec"
      echo "  --1080p, --720p, --4k  Set resolution preset"
      echo "  -r, --res, --resolution Set resolution (e.g. 1920x1080, 1080p, 1280x720)"
      echo "  -f, --fps              Set frame rate (default: 30)"
      echo "  -c, --codec            Set codec (H264 or H265)"
      echo "  -i, --input            Set input video file path"
      echo "  -h, --help             Display this help message"
      exit 0
      ;;
    --h264)
      RAW_CODEC="H264"
      shift
      ;;
    --h265|--hevc)
      RAW_CODEC="H265"
      shift
      ;;
    --1080p|--1080)
      RAW_RES="1920x1080"
      shift
      ;;
    --720p|--720)
      RAW_RES="1280x720"
      shift
      ;;
    --480p|--480)
      RAW_RES="854x480"
      shift
      ;;
    --360p|--360)
      RAW_RES="640x360"
      shift
      ;;
    --4k|--2160p)
      RAW_RES="3840x2160"
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
    -f|--fps)
      FPS="$2"
      shift 2
      ;;
    --fps=*)
      FPS="${1#*=}"
      shift
      ;;
    -c|--codec)
      RAW_CODEC="$2"
      shift 2
      ;;
    --codec=*)
      RAW_CODEC="${1#*=}"
      shift
      ;;
    -i|--input)
      CUSTOM_INPUT="$2"
      shift 2
      ;;
    --input=*)
      CUSTOM_INPUT="${1#*=}"
      shift
      ;;
    *)
      POSITIONAL_ARGS+=("$1")
      shift
      ;;
  esac
done

if [ ${#POSITIONAL_ARGS[@]} -gt 0 ]; then
  [ -n "${POSITIONAL_ARGS[0]}" ] && RAW_RES="${POSITIONAL_ARGS[0]}"
  [ -n "${POSITIONAL_ARGS[1]}" ] && FPS="${POSITIONAL_ARGS[1]}"
  [ -n "${POSITIONAL_ARGS[2]}" ] && RAW_CODEC="${POSITIONAL_ARGS[2]}"
  [ -n "${POSITIONAL_ARGS[3]}" ] && CUSTOM_INPUT="${POSITIONAL_ARGS[3]}"
fi

# Parse resolution
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
    WIDTH=1280
    HEIGHT=720
    RES_NAME="${RAW_RES}"
    ;;
esac

# Parse codec
CODEC_UPPER=$(echo "$RAW_CODEC" | tr '[:lower:]' '[:upper:]')
if [ "$CODEC_UPPER" = "H265" ] || [ "$CODEC_UPPER" = "HEVC" ]; then
  CODEC_NAME="H265"
  CODEC_LOWER="h265"
  FFMPEG_CODEC="libx265"
  BSF="hevc_mp4toannexb"
  EXT="265"
else
  CODEC_NAME="H264"
  CODEC_LOWER="h264"
  FFMPEG_CODEC="libx264"
  BSF="h264_mp4toannexb"
  EXT="264"
fi

OUTPUT_FILE="${ASSETS_DIR}/stream_${WIDTH}x${HEIGHT}_${FPS}fps_${CODEC_LOWER}.${EXT}"

# Check input MP4 file
INPUT_FILE="$CUSTOM_INPUT"
if [ -z "$INPUT_FILE" ] || [ ! -f "$INPUT_FILE" ]; then
  INPUT_FILE="${ASSETS_DIR}/bigbuckbunny_${RES_NAME}.mp4"
  if [ ! -f "$INPUT_FILE" ] || [ ! -s "$INPUT_FILE" ]; then
    echo "[INFO] Source video not found. Triggering download_bunny.sh for ${RAW_RES}..."
    "${SCRIPT_DIR}/download_bunny.sh" "$RAW_RES"
  fi
fi

if [ ! -f "$INPUT_FILE" ] || [ ! -s "$INPUT_FILE" ]; then
  echo "[ERROR] Could not find or download source video file: $INPUT_FILE"
  exit 1
fi

echo "====================================================="
echo " Preparing Stream-Ready Bitstream File"
echo " Source Video : ${INPUT_FILE}"
echo " Resolution   : ${WIDTH}x${HEIGHT} (${RES_NAME})"
echo " Frame Rate   : ${FPS} FPS"
echo " Codec        : ${CODEC_NAME} (${FFMPEG_CODEC})"
echo " Output File  : ${OUTPUT_FILE}"
echo "====================================================="

# Locate ffmpeg
FFMPEG_BIN=""
if [ -f "${SCRIPT_DIR}/client/node_modules/ffmpeg-static/ffmpeg" ]; then
  FFMPEG_BIN="${SCRIPT_DIR}/client/node_modules/ffmpeg-static/ffmpeg"
elif command -v ffmpeg >/dev/null 2>&1; then
  FFMPEG_BIN="ffmpeg"
else
  echo "[ERROR] ffmpeg binary not found."
  exit 1
fi

if [ "$CODEC_NAME" = "H265" ]; then
  "$FFMPEG_BIN" -y -i "$INPUT_FILE" \
    -vf "scale=${WIDTH}:${HEIGHT}" \
    -r "$FPS" \
    -c:v "$FFMPEG_CODEC" \
    -preset ultrafast \
    -tune zerolatency \
    -x265-params "keyint=${FPS}:min-keyint=${FPS}:no-scenecut=1:aud=1:repeat-headers=1" \
    -b:v 2M -maxrate 2.5M -bufsize 2M \
    -aud 1 \
    -bsf:v "$BSF" \
    "$OUTPUT_FILE"
else
  "$FFMPEG_BIN" -y -i "$INPUT_FILE" \
    -vf "scale=${WIDTH}:${HEIGHT}" \
    -r "$FPS" \
    -c:v "$FFMPEG_CODEC" \
    -preset ultrafast \
    -tune zerolatency \
    -x264-params "keyint=${FPS}:min-keyint=${FPS}:no-scenecut=1:repeat-headers=1" \
    -b:v 2M -maxrate 2.5M -bufsize 2M \
    -g "$FPS" -keyint_min "$FPS" -sc_threshold 0 \
    -aud 1 \
    -bsf:v "$BSF" \
    "$OUTPUT_FILE"
fi

echo ""
echo "[SUCCESS] Ready-for-streaming bitstream file created!"
echo "  File: $OUTPUT_FILE"
