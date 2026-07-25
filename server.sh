#!/usr/bin/env bash
set -e

BOARD_IP="192.168.1.72"
TARGET_DIR="~/rock5c-v4l2-drm"

function usage() {
  echo "Usage: ./server.sh [--start | --strat] | [--stop]"
  echo "  --start, --strat  : Sync repo, build Docker image, and launch server on board"
  echo "  --stop            : Stop and remove running server container on board"
  exit 1
}

if [ "$#" -lt 1 ]; then
  usage
fi

ACTION="$1"

case "$ACTION" in
  --start|--strat)
    echo "==> 1. Syncing local Git repository to board at $BOARD_IP:$TARGET_DIR..."
    rsync -avz --exclude '.git' . "$BOARD_IP:$TARGET_DIR"

    echo "==> 2. Building Docker image on board..."
    ssh "$BOARD_IP" "docker image prune -f 2>/dev/null || true"
    ssh "$BOARD_IP" "cd $TARGET_DIR && docker build --build-arg BUILD_DATE=\$(date +%s) -t rock5c-v4l2-drm ."
    ssh "$BOARD_IP" "docker image prune -f 2>/dev/null || true"

    echo "==> 3. Restarting Docker container on board in background mode..."
    ssh "$BOARD_IP" "docker stop rock5c-v4l2-drm 2>/dev/null || true"
    ssh "$BOARD_IP" "docker rm rock5c-v4l2-drm 2>/dev/null || true"
    ssh "$BOARD_IP" "docker kill \$(docker ps -q) 2>/dev/null || true"
    ssh "$BOARD_IP" "docker run -d --name rock5c-v4l2-drm --net=host --privileged -v /dev:/dev rock5c-v4l2-drm"

    echo "==> 4. Display active on HDMI! Showing IP Dashboard & Real-Time Clock..."
    sleep 1
    ssh "$BOARD_IP" "docker logs --tail 15 rock5c-v4l2-drm"
    ;;

  --stop)
    echo "==> Stopping WebTransport server container on $BOARD_IP..."
    ssh "$BOARD_IP" "docker stop rock5c-v4l2-drm 2>/dev/null || true"
    ssh "$BOARD_IP" "docker rm rock5c-v4l2-drm 2>/dev/null || true"
    echo "==> Server stopped successfully!"
    ;;

  *)
    usage
    ;;
esac
