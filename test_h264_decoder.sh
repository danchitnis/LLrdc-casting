#!/usr/bin/env bash
set -euo pipefail

# Directly validates the RK3399 stateless H.264 decoder without KMS. The input
# must contain a coded 1920x1088 stream, which is the browser's 1080p H.264
# surface after macroblock alignment.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BOARD_IP="${BOARD_IP:-100.100.1.72}"
BOARD_CONTAINER="${BOARD_CONTAINER:-llrdc-casting}"
STREAM_FILE="${1:-/tmp/h264_1920x1088_30.264}"

if [[ ! -s "$STREAM_FILE" ]]; then
  echo "Missing H.264 stream: $STREAM_FILE" >&2
  echo "Generate one with ffmpeg at coded 1920x1088 before running this test." >&2
  exit 2
fi

REMOTE_FILE="/tmp/$(basename "$STREAM_FILE")"
scp -q "$STREAM_FILE" "$BOARD_IP:$REMOTE_FILE"
ssh -o BatchMode=yes "$BOARD_IP" "docker cp '$REMOTE_FILE' '$BOARD_CONTAINER:$REMOTE_FILE'"

ssh -o BatchMode=yes "$BOARD_IP" "docker exec '$BOARD_CONTAINER' sh -lc '
  GST_DEBUG=v4l2slh264dec:4,v4l2codecs:4,h264parse:4 \
  gst-launch-1.0 -m -v \
    filesrc location=$REMOTE_FILE ! \
    h264parse config-interval=-1 ! \
    video/x-h264,stream-format=byte-stream,alignment=au ! \
    v4l2slh264dec video-device=/dev/video2 media-device=/dev/media0 ! \
    fpsdisplaysink video-sink=fakesink sync=false text-overlay=false
'"

echo "H.264 hardware decoder test passed for coded 1920x1088."
