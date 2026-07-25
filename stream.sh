#!/usr/bin/env bash
set -e

BOARD_IP="${1:-192.168.1.72}"
PORT="${2:-4434}"

echo "====================================================="
echo " Launching WebTransport / UDP Video Streamer Client"
echo " Target Board: $BOARD_IP:$PORT"
echo "====================================================="

node client/client.mjs "$BOARD_IP" "$PORT"
