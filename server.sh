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
PRE_CLOUD_DISCOVERY_ENABLED="${CLOUD_DISCOVERY_ENABLED:-}"
PRE_PAIRING_WORKER_URL="${PAIRING_WORKER_URL:-}"
PRE_RECEIVER_ID="${RECEIVER_ID:-}"
PRE_RECEIVER_REGISTRATION_SECRET="${RECEIVER_REGISTRATION_SECRET:-}"
PRE_PAIRING_TOKEN_PUBLIC_KEY_FILE="${PAIRING_TOKEN_PUBLIC_KEY_FILE:-}"
load_config
# Optional generated Cloudflare receiver credentials. Load these after the
# YAML defaults so setup values are not overwritten by cloud_discovery_enabled:
# false in config.yaml.
RECEIVER_ENV_FILE="${SCRIPT_DIR}/.cloudflare/receiver.env"
if [ -f "$RECEIVER_ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$RECEIVER_ENV_FILE"
  set +a
fi
if [ -n "$PRE_BOARD_IP" ]; then BOARD_IP="$PRE_BOARD_IP"; fi
if [ -n "$PRE_PAIRING_WORKER_URL" ]; then SERVER_PAIRING_WORKER_URL="$PRE_PAIRING_WORKER_URL"; fi
if [ -n "$PRE_CLOUD_DISCOVERY_ENABLED" ]; then SERVER_CLOUD_DISCOVERY_ENABLED="$PRE_CLOUD_DISCOVERY_ENABLED"; fi
if [ -n "$PRE_RECEIVER_ID" ]; then SERVER_RECEIVER_ID="$PRE_RECEIVER_ID"; fi
if [ -n "$PRE_RECEIVER_REGISTRATION_SECRET" ]; then SERVER_RECEIVER_REGISTRATION_SECRET="$PRE_RECEIVER_REGISTRATION_SECRET"; fi
if [ -n "$PRE_PAIRING_TOKEN_PUBLIC_KEY_FILE" ]; then SERVER_PAIRING_TOKEN_PUBLIC_KEY_FILE="$PRE_PAIRING_TOKEN_PUBLIC_KEY_FILE"; fi
BOARD_IP="${BOARD_IP:-}"
IMAGE="llrdc-casting"
CONNECTOR_ID="${PRE_CONNECTOR_ID:-${BOARD_DRM_CONNECTOR_ID:-${DRM_CONNECTOR_ID:-auto}}}"

usage() {
  echo "Usage: $0 --start [--dashboard|--no-dashboard] [--pairing-code=<4 alphanumeric>] | --get-pairing-code | --stop"
}

action="${1:-}"
[[ -n "$action" ]] || { usage; exit 2; }
shift || true

case "$action" in
  --get-pairing-code)
    (($# == 0)) || { usage; exit 2; }
    ssh -o BatchMode=yes "$BOARD_IP" "docker exec '$IMAGE' /usr/local/bin/llrdc-casting admin pairing-code"
    ;;
  --start)
    idle_dashboard=1
    pairing_code_fixed=""
    while (($#)); do
      case "$1" in
        --dashboard) idle_dashboard=1 ;;
        --no-dashboard) idle_dashboard=0 ;;
        --pairing-code=*)
          pairing_code_fixed="${1#*=}"
          if ! [[ "$pairing_code_fixed" =~ ^[A-Za-z0-9]{4}$ ]]; then
            echo "Pairing code must be exactly four alphanumeric characters." >&2
            exit 2
          fi
          pairing_code_fixed="$(printf '%s' "$pairing_code_fixed" | tr '[:lower:]' '[:upper:]')"
          ;;
        *) usage; exit 2 ;;
      esac
      shift
    done

    case "${SERVER_CLOUD_DISCOVERY_ENABLED:-0}" in
      1|true|TRUE|yes|YES)
        if [[ -n "$pairing_code_fixed" ]]; then
          echo "Fixed pairing codes cannot be used with Cloudflare discovery enabled." >&2
          exit 2
        fi
        ;;
    esac
    if [[ -n "$pairing_code_fixed" ]]; then
      echo "[WARNING] Fixed pairing code mode is enabled for this local test deployment."
    fi

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
      docker buildx build --build-arg BUILD_DATE="$(date +%s)" --target builder --platform linux/arm64 -t "${IMAGE}-builder" --load .
    fi

    # Extract compiled binary and copy to board host path
    tmp_id=$(docker create "${IMAGE}-builder")
    docker cp "${tmp_id}:/app/target/release/llrdc-casting" /tmp/llrdc-casting
    docker rm "$tmp_id" >/dev/null
    bin_size=$(ls -lh /tmp/llrdc-casting | awk '{print $5}')
    echo "[TRANSFER] Transferring only binary (/tmp/llrdc-casting: ${bin_size}) to board..."
    scp -o BatchMode=yes /tmp/llrdc-casting "${BOARD_IP}:/var/tmp/llrdc-bin/llrdc-casting"
    rm -f /tmp/llrdc-casting

    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; sleep 1; docker run -d --name '$IMAGE' --restart unless-stopped --net host --privileged -e DRM_CONNECTOR_ID='${SERVER_DRM_CONNECTOR_ID:-${BOARD_DRM_CONNECTOR_ID:-${DRM_CONNECTOR_ID:-auto}}}' -e DRM_PLANE_ID='${SERVER_DRM_PLANE_ID:-${BOARD_DRM_PLANE_ID:-33}}' -e IDLE_DASHBOARD='${idle_dashboard:-${SERVER_IDLE_DASHBOARD:-${BOARD_IDLE_DASHBOARD:-1}}}' -e IDLE_DASHBOARD_MODE='${SERVER_IDLE_DASHBOARD_MODE:-${BOARD_IDLE_DASHBOARD_MODE:-raw}}' -e IDLE_TIMEOUT_SEC='${SERVER_IDLE_TIMEOUT_SEC:-${BOARD_IDLE_TIMEOUT_SEC:-30}}' -e PAIRING_CODE_TTL_SEC='${SERVER_PAIRING_CODE_TTL_SEC:-${BOARD_PAIRING_CODE_TTL_SEC:-3600}}' -e PAIRING_CODE_FIXED='$pairing_code_fixed' -e HTTP_PORT='${SERVER_HTTP_PORT:-${BOARD_HTTP_PORT:-8080}}' -e WEBTRANSPORT_PORT='${SERVER_WEBTRANSPORT_PORT:-${BOARD_WEBTRANSPORT_PORT:-4433}}' -e BOARD_PORT='${SERVER_PORT:-${BOARD_PORT:-4434}}' -e UDP_BUFFER_SIZE_MB='${SERVER_UDP_BUFFER_SIZE_MB:-${BOARD_UDP_BUFFER_SIZE_MB:-8}}' -e CERTS_DIR='${SERVER_CERT_DIR:-${BOARD_CERT_DIR:-/certs}}' -e CLOUD_DISCOVERY_ENABLED='${SERVER_CLOUD_DISCOVERY_ENABLED:-0}' -e PAIRING_WORKER_URL='${SERVER_PAIRING_WORKER_URL:-https://cast.llrdc.com}' -e RECEIVER_ID='${SERVER_RECEIVER_ID:-}' -e RECEIVER_REGISTRATION_SECRET='${SERVER_RECEIVER_REGISTRATION_SECRET:-}' -e PAIRING_TOKEN_PUBLIC_KEY_FILE='${SERVER_PAIRING_TOKEN_PUBLIC_KEY_FILE:-/pairing/public.pem}' -v /dev:/dev -v /var/lib/llrdc-certs:/certs -v /var/lib/llrdc-pairing:/pairing:ro -v /var/tmp/llrdc-bin/llrdc-casting:/usr/local/bin/llrdc-casting '$IMAGE'; sleep 2; docker logs --tail 30 '$IMAGE'"
    ;;
  --stop)
    (($# == 0)) || { usage; exit 2; }
    ssh -o BatchMode=yes "$BOARD_IP" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true"
    ;;
  *) usage; exit 2 ;;
esac
