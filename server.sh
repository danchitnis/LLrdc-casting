#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Load .env if present
if [ -f "${SCRIPT_DIR}/.env" ]; then
  PRE_BOARD_IP="${BOARD_IP:-}"
  set -a
  source "${SCRIPT_DIR}/.env"
  set +a
  if [ -n "$PRE_BOARD_IP" ]; then
    BOARD_IP="$PRE_BOARD_IP"
  fi
fi

BOARD_IP="${BOARD_IP:-}"
IMAGE="llrdc-casting"
CONNECTOR_ID="${DRM_CONNECTOR_ID:-auto}"

usage() {
  echo "Usage: $0 --start [--dashboard|--no-dashboard] | --stop"
}

action="${1:-}"
[[ -n "$action" ]] || { usage; exit 2; }
shift || true

case "$action" in
  --start)
    idle_dashboard=1
    while (($#)); do
      case "$1" in
        --dashboard) idle_dashboard=1 ;;
        --no-dashboard) idle_dashboard=0 ;;
        *) usage; exit 2 ;;
      esac
      shift
    done

    # Hash Dockerfile to detect if GStreamer / OS dependencies changed
    DOCKERFILE_HASH=$(shasum -a 256 "${SCRIPT_DIR}/Dockerfile" | awk '{print $1}')
    REMOTE_HASH=$(ssh -o BatchMode=yes "$BOARD_IP" "cat /var/tmp/llrdc-bin/Dockerfile.sha256 2>/dev/null || true")

    ssh -o BatchMode=yes "$BOARD_IP" "mkdir -p /var/tmp/llrdc-bin && rm -rf /var/tmp/llrdc-bin/llrdc-casting"

    if [ "$DOCKERFILE_HASH" != "$REMOTE_HASH" ]; then
      echo "[DEPLOY] Dockerfile changed or first deploy -> Transferring full base Docker image (278MB)..."
      docker buildx build --build-arg BUILD_DATE="$(date +%s)" --platform linux/arm64 -t "$IMAGE" --load .
      docker save "$IMAGE" | gzip -1 | ssh -o BatchMode=yes "$BOARD_IP" 'gunzip | docker load'
      ssh -o BatchMode=yes "$BOARD_IP" "echo '$DOCKERFILE_HASH' > /var/tmp/llrdc-bin/Dockerfile.sha256"
    else
      echo "[DEPLOY] Dockerfile unchanged -> Fast deploy: building and copying only binary (~3.9MB)..."
      docker buildx build --target builder --platform linux/arm64 -t "${IMAGE}-builder" --load .
    fi

    # Extract compiled binary and copy to board host path
    tmp_id=$(docker create "${IMAGE}-builder")
    docker cp "${tmp_id}:/app/target/release/llrdc-casting" /tmp/llrdc-casting
    docker rm "$tmp_id" >/dev/null
    bin_size=$(ls -lh /tmp/llrdc-casting | awk '{print $5}')
    echo "[TRANSFER] Transferring only binary (/tmp/llrdc-casting: ${bin_size}) to board..."
    scp -o BatchMode=yes /tmp/llrdc-casting "${BOARD_IP}:/var/tmp/llrdc-bin/llrdc-casting"
    rm -f /tmp/llrdc-casting

    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; sleep 1; docker run -d --name '$IMAGE' --restart unless-stopped --net host --privileged -e DRM_CONNECTOR_ID='$CONNECTOR_ID' -e IDLE_DASHBOARD='$idle_dashboard' -v /dev:/dev -v /var/lib/llrdc-certs:/certs -v /var/tmp/llrdc-bin/llrdc-casting:/usr/local/bin/llrdc-casting '$IMAGE'; sleep 2; docker logs --tail 30 '$IMAGE'"
    ;;
  --stop)
    (($# == 0)) || { usage; exit 2; }
    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true"
    ;;
  *) usage; exit 2 ;;
esac
