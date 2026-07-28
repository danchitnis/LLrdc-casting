#!/usr/bin/env bash
set -euo pipefail

BOARD_IP="${BOARD_IP:-192.168.1.72}"
IMAGE="rock5c-v4l2-drm"
CONNECTOR_ID="${DRM_CONNECTOR_ID:-54}"

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

    docker buildx build --build-arg BUILD_DATE="$(date +%s)" --platform linux/arm64 -t "$IMAGE" --load .
    docker save "$IMAGE" | gzip -1 | ssh -o BatchMode=yes "$BOARD_IP" 'gunzip | docker load'
    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' 2>/dev/null || true; docker rm -f '$IMAGE' 2>/dev/null || true; sleep 1; docker run -d --name '$IMAGE' --restart unless-stopped --net host --privileged -e DRM_CONNECTOR_ID='$CONNECTOR_ID' -e IDLE_DASHBOARD='$idle_dashboard' -v /dev:/dev -v /var/lib/rock5c-certs:/certs '$IMAGE'; sleep 2; docker logs --tail 30 '$IMAGE'"
    ;;
  --stop)
    (($# == 0)) || { usage; exit 2; }
    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' 2>/dev/null || true; docker rm -f '$IMAGE' 2>/dev/null || true"
    ;;
  *) usage; exit 2 ;;
esac
