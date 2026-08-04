#!/usr/bin/env bash
# Parameterized HEVC HDMI smoke test for the RK3399 receiver.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Load config.yaml if present
load_config() {
  local cfg="${SCRIPT_DIR}/config.yaml"
  if [ -f "$cfg" ]; then
    eval "$(python3 -c '
import sys
path = sys.argv[1]
current_section = ""
with open(path) as f:
    for line in f:
        line = line.split("#")[0].rstrip()
        if not line: continue
        indent = len(line) - len(line.lstrip())
        line_str = line.strip()
        if indent == 0 and line_str.endswith(":") and ":" not in line_str[:-1]:
            current_section = line_str[:-1].strip().replace("-", "_")
            continue
        if ":" in line_str:
            k, v = line_str.split(":", 1)
            k = k.strip().replace("-", "_")
            v = v.strip().strip("\"'\''")
            full_key = f"{current_section}_{k}".upper() if indent > 0 and current_section else k.upper()
            if v:
                print(f"export {full_key}=\"{v}\"")
' "$cfg" 2>/dev/null || true)"
  fi
}

PRE_BOARD_IP="${BOARD_IP:-}"
load_config
if [ -n "$PRE_BOARD_IP" ]; then BOARD_IP="$PRE_BOARD_IP"; fi

BOARD_IP="${BOARD_IP:-}"
PORT="${SERVER_PORT:-${BOARD_PORT:-4434}}"
RESOLUTION="${TEST_STREAM_RESOLUTION:-${STREAM_RESOLUTION:-3840x2160}}"
FPS="${TEST_STREAM_FPS:-${STREAM_FPS:-60}}"
DURATION="${TEST_STREAM_DURATION:-${STREAM_DURATION:-20}}"
RESOLUTION="3840x2160"
FPS="60"
DURATION="20"
STREAM_FILE=""
DEPLOY=0

usage() {
  cat <<'EOF'
Usage: ./test.sh [options]
  -r, --res WIDTHxHEIGHT  Resolution (default: 3840x2160)
      --4k|--2160p        3840x2160 preset
      --2k|--1440p        2560x1440 preset
      --1080p             1920x1080 preset
      --720p              1280x720 preset
  -f, --fps FPS           Frame rate (default: 60)
  -d, --duration SEC      Duration (default: 20)
  -i, --ip IP             Board IP
  -p, --port PORT         UDP port
      --file PATH         Annex-B HEVC stream (.265)
      --deploy            Build/deploy before testing
  -h, --help              Show help

Without --file, uses client/assets/stream_<RES>_<FPS>fps_h265.265.
Create it with: ./prepare_stream.sh --h265 --res RES --fps FPS
EOF
}

while (($#)); do
  case "$1" in
    -r|--res|--resolution) RESOLUTION="$2"; shift 2 ;;
    --res=*|--resolution=*) RESOLUTION="${1#*=}"; shift ;;
    --4k|--2160p) RESOLUTION="3840x2160"; shift ;;
    --2k|--1440p) RESOLUTION="2560x1440"; shift ;;
    --1080p|--1080) RESOLUTION="1920x1080"; shift ;;
    --720p|--720) RESOLUTION="1280x720"; shift ;;
    -f|--fps) FPS="$2"; shift 2 ;;
    --fps=*) FPS="${1#*=}"; shift ;;
    -d|--duration) DURATION="$2"; shift 2 ;;
    --duration=*) DURATION="${1#*=}"; shift ;;
    -i|--ip|--board-ip) BOARD_IP="$2"; shift 2 ;;
    -p|--port) PORT="$2"; shift 2 ;;
    --file) STREAM_FILE="$2"; shift 2 ;;
    --deploy) DEPLOY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ ! "$RESOLUTION" =~ ^[1-9][0-9]*x[1-9][0-9]*$ ]]; then
  echo "Invalid resolution: $RESOLUTION (use WIDTHxHEIGHT)" >&2; exit 2
fi
if [[ ! "$FPS" =~ ^[1-9][0-9]*$ ]] || [[ ! "$DURATION" =~ ^[1-9][0-9]*$ ]]; then
  echo "FPS and duration must be positive integers." >&2; exit 2
fi
if [[ -z "$STREAM_FILE" ]]; then
  STREAM_FILE="${SCRIPT_DIR}/client/assets/stream_${RESOLUTION}_${FPS}fps_h265.265"
fi
if [[ ! -s "$STREAM_FILE" ]]; then
  echo "Missing HEVC stream: $STREAM_FILE" >&2
  echo "Create it with: ./prepare_stream.sh --h265 --res $RESOLUTION --fps $FPS" >&2
  exit 2
fi
if (( DEPLOY )); then
  BOARD_IP="$BOARD_IP" "${SCRIPT_DIR}/server.sh" --start
  sleep 3
fi
if ! ssh -o BatchMode=yes -o ConnectTimeout=5 "$BOARD_IP" 'docker ps --format "{{.Names}}" | grep -qx llrdc-casting'; then
  echo "Receiver container is not running; use --deploy to build and start it." >&2; exit 1
fi

echo "Testing HEVC ${RESOLUTION}@${FPS} for ${DURATION}s -> ${BOARD_IP}:${PORT}"
node "${SCRIPT_DIR}/client/client.mjs" "$BOARD_IP" "$PORT" "$RESOLUTION" "$FPS" H265 "$STREAM_FILE" -d "$DURATION"
sleep 1
memory=$(ssh -o BatchMode=yes "$BOARD_IP" 'docker stats --no-stream --format "{{.MemUsage}}" llrdc-casting')
playback=$(ssh -o BatchMode=yes "$BOARD_IP" 'docker logs llrdc-casting 2>&1 | grep "\[PLAYBACK" | tail -1 || true')
echo
echo "Receiver memory: ${memory}"
echo "Latest receiver state: ${playback:-no playback log emitted}"
echo "Board playback pipeline: HEVC -> rkvdec (V4L2 stateless) -> DMA-BUF -> HDMI"
