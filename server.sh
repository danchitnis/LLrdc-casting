#!/usr/bin/env bash
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
PRE_CONNECTOR_ID="${DRM_CONNECTOR_ID:-}"
load_config
if [ -n "$PRE_BOARD_IP" ]; then BOARD_IP="$PRE_BOARD_IP"; fi
BOARD_IP="${BOARD_IP:-}"
IMAGE="llrdc-casting"
CONNECTOR_ID="${PRE_CONNECTOR_ID:-${BOARD_DRM_CONNECTOR_ID:-${DRM_CONNECTOR_ID:-auto}}}"

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

    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; sleep 1; docker run -d --name '$IMAGE' --restart unless-stopped --net host --privileged -e DRM_CONNECTOR_ID='${SERVER_DRM_CONNECTOR_ID:-${BOARD_DRM_CONNECTOR_ID:-${DRM_CONNECTOR_ID:-auto}}}' -e DRM_PLANE_ID='${SERVER_DRM_PLANE_ID:-${BOARD_DRM_PLANE_ID:-33}}' -e IDLE_DASHBOARD='${idle_dashboard:-${SERVER_IDLE_DASHBOARD:-${BOARD_IDLE_DASHBOARD:-1}}}' -e IDLE_TIMEOUT_SEC='${SERVER_IDLE_TIMEOUT_SEC:-${BOARD_IDLE_TIMEOUT_SEC:-30}}' -e HTTP_PORT='${SERVER_HTTP_PORT:-${BOARD_HTTP_PORT:-8080}}' -e WEBTRANSPORT_PORT='${SERVER_WEBTRANSPORT_PORT:-${BOARD_WEBTRANSPORT_PORT:-4433}}' -e BOARD_PORT='${SERVER_PORT:-${BOARD_PORT:-4434}}' -e UDP_BUFFER_SIZE_MB='${SERVER_UDP_BUFFER_SIZE_MB:-${BOARD_UDP_BUFFER_SIZE_MB:-8}}' -e CERTS_DIR='${SERVER_CERT_DIR:-${BOARD_CERT_DIR:-/certs}}' -v /dev:/dev -v /var/lib/llrdc-certs:/certs -v /var/tmp/llrdc-bin/llrdc-casting:/usr/local/bin/llrdc-casting '$IMAGE'; sleep 2; docker logs --tail 30 '$IMAGE'"
    ;;
  --stop)
    (($# == 0)) || { usage; exit 2; }
    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true"
    ;;
  *) usage; exit 2 ;;
esac
