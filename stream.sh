#!/usr/bin/env bash
set -e

# WebTransport / UDP Video Streamer Script
# Supports flags (--h264, --h265, --1080p, --720p, --fps, --ip, --port, --file)
# as well as positional arguments.
#
# Examples:
#   ./stream.sh --h265
#   ./stream.sh --h264 --1080p
#   ./stream.sh --ip 192.168.1.72 --fps 60 --h265
#   ./stream.sh 192.168.1.72 4434 1080p 30 H265

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Default settings
BOARD_IP="${BOARD_IP:-192.168.1.72}"
PORT="${BOARD_PORT:-4434}"
RAW_RES="${BOARD_RES:-1280x720}"
FPS="${STREAM_FPS:-30}"
RAW_CODEC="${CODEC:-H264}"
CUSTOM_FILE="${STREAM_FILE:-}"

POSITIONAL_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      echo "Usage: ./stream.sh [OPTIONS] [BOARD_IP] [PORT] [RES] [FPS] [CODEC] [FILE]"
      echo ""
      echo "Options:"
      echo "  --h264                 Stream using H.264 video codec"
      echo "  --h265, --hevc         Stream using H.265 / HEVC video codec"
      echo "  --1080p, --720p, --4k  Set resolution preset"
      echo "  -r, --res, --resolution Set resolution (e.g. 1920x1080, 1080p, 1280x720)"
      echo "  -f, --fps              Set frame rate (default: 30)"
      echo "  -c, --codec            Set codec (H264 or H265)"
      echo "  -i, --ip, --board-ip   Set target board IP address (default: 192.168.1.72)"
      echo "  -p, --port             Set target UDP port (default: 4434)"
      echo "  --file, --stream-file  Set custom bitstream file path"
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
    -i|--ip|--board-ip)
      BOARD_IP="$2"
      shift 2
      ;;
    --ip=*|--board-ip=*)
      BOARD_IP="${1#*=}"
      shift
      ;;
    -p|--port)
      PORT="$2"
      shift 2
      ;;
    --port=*)
      PORT="${1#*=}"
      shift
      ;;
    --file|--stream-file)
      CUSTOM_FILE="$2"
      shift 2
      ;;
    --file=*|--stream-file=*)
      CUSTOM_FILE="${1#*=}"
      shift
      ;;
    *)
      POSITIONAL_ARGS+=("$1")
      shift
      ;;
  esac
done

# If positional arguments were provided, parse them intelligently
if [ ${#POSITIONAL_ARGS[@]} -gt 0 ]; then
  first_arg="${POSITIONAL_ARGS[0]}"
  if [[ "$first_arg" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]] || [[ "$first_arg" == "localhost" ]]; then
    BOARD_IP="$first_arg"
    [ -n "${POSITIONAL_ARGS[1]}" ] && PORT="${POSITIONAL_ARGS[1]}"
    [ -n "${POSITIONAL_ARGS[2]}" ] && RAW_RES="${POSITIONAL_ARGS[2]}"
    [ -n "${POSITIONAL_ARGS[3]}" ] && FPS="${POSITIONAL_ARGS[3]}"
    [ -n "${POSITIONAL_ARGS[4]}" ] && RAW_CODEC="${POSITIONAL_ARGS[4]}"
    [ -n "${POSITIONAL_ARGS[5]}" ] && CUSTOM_FILE="${POSITIONAL_ARGS[5]}"
  else
    for arg in "${POSITIONAL_ARGS[@]}"; do
      arg_upper=$(echo "$arg" | tr '[:lower:]' '[:upper:]')
      if [ "$arg_upper" = "H264" ] || [ "$arg_upper" = "H265" ] || [ "$arg_upper" = "HEVC" ]; then
        RAW_CODEC="$arg_upper"
      elif [[ "$arg" =~ ^[0-9]+x[0-9]+$ ]] || [[ "$arg" =~ ^[0-9]+p$ ]] || [ "$arg" = "4k" ]; then
        RAW_RES="$arg"
      elif [[ "$arg" =~ ^[0-9]+$ ]]; then
        if [ "$arg" -gt 100 ]; then
          PORT="$arg"
        else
          FPS="$arg"
        fi
      fi
    done
  fi
fi

# Normalize resolution string
case "$RAW_RES" in
  1080p|1080|1920x1080)
    RES="1920x1080"
    ;;
  720p|720|1280x720)
    RES="1280x720"
    ;;
  480p|480|854x480)
    RES="854x480"
    ;;
  360p|360|640x360)
    RES="640x360"
    ;;
  4k|2160p|2160|3840x2160)
    RES="3840x2160"
    ;;
  *)
    RES="$RAW_RES"
    ;;
esac

# Normalize codec
CODEC=$(echo "$RAW_CODEC" | tr '[:lower:]' '[:upper:]')
if [ "$CODEC" = "HEVC" ] || [ "$CODEC" = "H265" ]; then
  CODEC="H265"
else
  CODEC="H264"
fi

CODEC_LOWER=$(echo "$CODEC" | tr '[:upper:]' '[:lower:]')
EXT="264"
if [ "$CODEC" = "H265" ]; then
  EXT="265"
fi

# Input validation
if [ -z "$BOARD_IP" ]; then
  echo "[ERROR] Target Board IP is required."
  exit 1
fi

STREAM_FILE="$CUSTOM_FILE"
if [ -z "$STREAM_FILE" ]; then
  STREAM_FILE="${SCRIPT_DIR}/client/assets/stream_${RES}_${FPS}fps_${CODEC_LOWER}.${EXT}"
fi

# If prepared bitstream file doesn't exist, generate it now
if [ ! -f "$STREAM_FILE" ] || [ ! -s "$STREAM_FILE" ]; then
  echo "[INFO] Prepared bitstream file not found at: $STREAM_FILE"
  echo "[INFO] Invoking prepare_stream.sh $RES $FPS $CODEC ..."
  "${SCRIPT_DIR}/prepare_stream.sh" "$RES" "$FPS" "$CODEC"
fi

echo "====================================================="
echo " Launching WebTransport / UDP Video Streamer Client"
echo " Target Board : ${BOARD_IP}:${PORT}"
echo " Resolution   : ${RES}"
echo " Frame Rate   : ${FPS} FPS"
echo " Codec        : ${CODEC}"
echo " Video File   : ${STREAM_FILE}"
echo "====================================================="

node "${SCRIPT_DIR}/client/client.mjs" "$BOARD_IP" "$PORT" "$RES" "$FPS" "$CODEC" "$STREAM_FILE"
