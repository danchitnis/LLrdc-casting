#!/usr/bin/env bash
set -e

BOARD_IP="192.168.1.72"
TARGET_DIR="~/rock5c-v4l2-drm"

echo "==> 1. Syncing local Git repository to board at $BOARD_IP:$TARGET_DIR..."
rsync -avz --exclude '.git' . "$BOARD_IP:$TARGET_DIR"

echo "==> 2. Building Docker image on board..."
ssh "$BOARD_IP" "cd $TARGET_DIR && docker build -t rock5c-v4l2-drm ."

echo "==> 3. Running Docker container on board with DRM and V4L2 device access..."
ssh "$BOARD_IP" "docker run --rm --privileged -v /dev:/dev rock5c-v4l2-drm"

echo "==> Deployment complete!"
