#!/usr/bin/env bash
set -e

BOARD_IP="192.168.1.72"
TARGET_DIR="~/rock5c-v4l2-drm"

echo "==> 1. Syncing local Git repository to board at $BOARD_IP:$TARGET_DIR..."
rsync -avz --exclude '.git' . "$BOARD_IP:$TARGET_DIR"

echo "==> 2. Building Docker image on board..."
ssh "$BOARD_IP" "cd $TARGET_DIR && docker build -t rock5c-v4l2-drm ."

echo "==> 3. Restarting Docker container on board in background mode..."
ssh "$BOARD_IP" "docker stop rock5c-v4l2-drm 2>/dev/null || true"
ssh "$BOARD_IP" "docker rm rock5c-v4l2-drm 2>/dev/null || true"
ssh "$BOARD_IP" "docker kill \$(docker ps -q) 2>/dev/null || true"
ssh "$BOARD_IP" "docker run -d --name rock5c-v4l2-drm --net=host --privileged -v /dev:/dev rock5c-v4l2-drm"

echo "==> 4. Waiting 2 seconds for server initialization and 1-second IP screen display..."
sleep 2

echo "==> 5. Executing local WebTransport client to transmit static H.264 frame to $BOARD_IP:4433..."
node client/client.mjs "$BOARD_IP" 4433

echo "==> 6. Fetching board container server logs..."
ssh "$BOARD_IP" "docker logs --tail 20 rock5c-v4l2-drm"

echo "==> Deployment & frame transmission complete!"
