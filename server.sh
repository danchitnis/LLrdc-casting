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
  echo "Usage:"
  echo "  $0 --test"
  echo "  $0 --start [options]"
  echo "  $0 --get-pairing-code [--board-ip=<address>]"
  echo "  $0 --stop [--board-ip=<address>]"
  echo "Start options:"
  echo "  --board-ip=<address>                         Target board address"
  echo "  --drm-connector-id=<id>                     DRM connector (default: auto)"
  echo "  --drm-plane-id=<id>                         DRM plane (default: 33)"
  echo "  --dashboard | --no-dashboard                Enable or disable idle dashboard"
  echo "  --dashboard-mode=<raw|hevc>                 Idle dashboard codec"
  echo "  --idle-timeout-sec=<seconds>                Idle timeout (default: 30)"
  echo "  --pairing-code-ttl-sec=<seconds>            Pairing-code lifetime (default: 3600)"
  echo "  --pairing-code=<alphanumeric>               Fixed test pairing code"
  echo "  --http-port=<port>                          HTTP/UI port (default: 8080)"
  echo "  --admin-bind-address=<address>              Tailscale-only admin bind address"
  echo "  --admin-port=<port>                         Tailscale-only admin port (default: 9090)"
  echo "  --webtransport-port=<port>                  WebTransport port (default: 4433)"
  echo "  --board-port=<port>                         Video UDP port (default: 4434)"
  echo "  --udp-buffer-size-mb=<megabytes>            UDP receive buffer (default: 8)"
  echo "  --cert-dir=<path>                           Certificate directory (default: /certs)"
  echo "  --cloud=true|false                          Enable or disable Cloudflare discovery"
  echo "  --pairing-worker-url=<url>                  Pairing Worker URL"
  echo "  --receiver-id=<id>                          Cloudflare receiver ID"
  echo "  --receiver-registration-secret=<secret>     Cloudflare registration secret"
  echo "  --pairing-token-public-key-file=<path>      Pairing token public-key file"
}

action="${1:-}"
[[ -n "$action" ]] || { usage; exit 2; }
shift || true

case "$action" in
  --help|-h)
    usage
    ;;
  --test)
    (($# == 0)) || { usage; exit 2; }
    echo "[TEST] Running Rust unit tests in the ARM64 Docker builder..."
    docker buildx build \
      --build-arg BUILD_DATE="$(date +%s)" \
      --target tests \
      --platform linux/arm64 \
      --tag "${IMAGE}-tests" \
      --load \
      .
    ;;
  --get-pairing-code)
    board_ip_override=""
    while (($#)); do
      case "$1" in
        --help|-h) usage; exit 0 ;;
        --board-ip=*) board_ip_override="${1#*=}" ;;
        *) usage; exit 2 ;;
      esac
      shift
    done
    board_ip="${board_ip_override:-$BOARD_IP}"
    [[ -n "$board_ip" ]] || { echo "A board address is required; use --board-ip=<address>." >&2; exit 2; }
    ssh -o BatchMode=yes "$board_ip" "docker exec '$IMAGE' /usr/local/bin/llrdc-casting admin pairing-code"
    ;;
  --start)
    board_ip_override=""
    drm_connector_override=""
    drm_plane_override=""
    dashboard_override=""
    dashboard_mode_override=""
    idle_timeout_override=""
    pairing_code_ttl_override=""
    http_port_override=""
    admin_bind_address_override=""
    admin_port_override=""
    webtransport_port_override=""
    board_port_override=""
    udp_buffer_size_override=""
    cert_dir_override=""
    cloud_override=""
    pairing_worker_url_override=""
    receiver_id_override=""
    receiver_registration_secret_override=""
    pairing_token_public_key_file_override=""
    pairing_code_fixed=""
    while (($#)); do
      case "$1" in
        --board-ip=*) board_ip_override="${1#*=}" ;;
        --drm-connector-id=*) drm_connector_override="${1#*=}" ;;
        --drm-plane-id=*) drm_plane_override="${1#*=}" ;;
        --dashboard) dashboard_override=1 ;;
        --no-dashboard) dashboard_override=0 ;;
        --dashboard-mode=*) dashboard_mode_override="${1#*=}" ;;
        --idle-timeout-sec=*) idle_timeout_override="${1#*=}" ;;
        --pairing-code-ttl-sec=*) pairing_code_ttl_override="${1#*=}" ;;
        --cloud=true) cloud_override=1 ;;
        --cloud=false) cloud_override=0 ;;
        --cloud=*)
          echo "Cloud discovery must be enabled with --cloud=true or disabled with --cloud=false." >&2
          exit 2
          ;;
        --http-port=*) http_port_override="${1#*=}" ;;
        --admin-bind-address=*) admin_bind_address_override="${1#*=}" ;;
        --admin-port=*) admin_port_override="${1#*=}" ;;
        --webtransport-port=*) webtransport_port_override="${1#*=}" ;;
        --board-port=*) board_port_override="${1#*=}" ;;
        --udp-buffer-size-mb=*) udp_buffer_size_override="${1#*=}" ;;
        --cert-dir=*) cert_dir_override="${1#*=}" ;;
        --pairing-worker-url=*) pairing_worker_url_override="${1#*=}" ;;
        --receiver-id=*) receiver_id_override="${1#*=}" ;;
        --receiver-registration-secret=*) receiver_registration_secret_override="${1#*=}" ;;
        --pairing-token-public-key-file=*) pairing_token_public_key_file_override="${1#*=}" ;;
        --pairing-code=*)
          pairing_code_fixed="${1#*=}"
          if ! [[ "$pairing_code_fixed" =~ ^[A-Za-z0-9]+$ ]]; then
            echo "Pairing code must contain only letters and numbers." >&2
            exit 2
          fi
          pairing_code_fixed="$(printf '%s' "$pairing_code_fixed" | tr '[:lower:]' '[:upper:]')"
          ;;
        *) usage; exit 2 ;;
      esac
      shift
    done

    board_ip="${board_ip_override:-$BOARD_IP}"
    drm_connector_id="${drm_connector_override:-${SERVER_DRM_CONNECTOR_ID:-${BOARD_DRM_CONNECTOR_ID:-${DRM_CONNECTOR_ID:-auto}}}}"
    drm_plane_id="${drm_plane_override:-${SERVER_DRM_PLANE_ID:-${BOARD_DRM_PLANE_ID:-33}}}"
    idle_dashboard="${SERVER_IDLE_DASHBOARD:-${BOARD_IDLE_DASHBOARD:-1}}"
    idle_dashboard_mode="${SERVER_IDLE_DASHBOARD_MODE:-${BOARD_IDLE_DASHBOARD_MODE:-raw}}"
    idle_timeout_sec="${SERVER_IDLE_TIMEOUT_SEC:-${BOARD_IDLE_TIMEOUT_SEC:-30}}"
    pairing_code_ttl_sec="${SERVER_PAIRING_CODE_TTL_SEC:-${BOARD_PAIRING_CODE_TTL_SEC:-3600}}"
    http_port="${SERVER_HTTP_PORT:-${BOARD_HTTP_PORT:-8080}}"
    admin_bind_address="${SERVER_ADMIN_BIND_ADDRESS:-${BOARD_ADMIN_BIND_ADDRESS:-$board_ip}}"
    admin_port="${SERVER_ADMIN_PORT:-${BOARD_ADMIN_PORT:-9090}}"
    webtransport_port="${SERVER_WEBTRANSPORT_PORT:-${BOARD_WEBTRANSPORT_PORT:-4433}}"
    board_port="${SERVER_PORT:-${BOARD_PORT:-4434}}"
    udp_buffer_size_mb="${SERVER_UDP_BUFFER_SIZE_MB:-${BOARD_UDP_BUFFER_SIZE_MB:-8}}"
    cert_dir="${SERVER_CERT_DIR:-${BOARD_CERT_DIR:-/certs}}"
    cloud_discovery_enabled="${SERVER_CLOUD_DISCOVERY_ENABLED:-0}"
    pairing_worker_url="${SERVER_PAIRING_WORKER_URL:-https://cast.llrdc.com}"
    receiver_id="${SERVER_RECEIVER_ID:-}"
    receiver_registration_secret="${SERVER_RECEIVER_REGISTRATION_SECRET:-}"
    pairing_token_public_key_file="${SERVER_PAIRING_TOKEN_PUBLIC_KEY_FILE:-/pairing/public.pem}"

    if [[ -n "$dashboard_override" ]]; then idle_dashboard="$dashboard_override"; fi
    if [[ -n "$dashboard_mode_override" ]]; then idle_dashboard_mode="$dashboard_mode_override"; fi
    if [[ -n "$idle_timeout_override" ]]; then idle_timeout_sec="$idle_timeout_override"; fi
    if [[ -n "$pairing_code_ttl_override" ]]; then pairing_code_ttl_sec="$pairing_code_ttl_override"; fi
    if [[ -n "$http_port_override" ]]; then http_port="$http_port_override"; fi
    if [[ -n "$admin_bind_address_override" ]]; then admin_bind_address="$admin_bind_address_override"; fi
    if [[ -n "$admin_port_override" ]]; then admin_port="$admin_port_override"; fi
    if [[ -n "$webtransport_port_override" ]]; then webtransport_port="$webtransport_port_override"; fi
    if [[ -n "$board_port_override" ]]; then board_port="$board_port_override"; fi
    if [[ -n "$udp_buffer_size_override" ]]; then udp_buffer_size_mb="$udp_buffer_size_override"; fi
    if [[ -n "$cert_dir_override" ]]; then cert_dir="$cert_dir_override"; fi
    if [[ -n "$cloud_override" ]]; then
      cloud_discovery_enabled="$cloud_override"
    fi
    if [[ -n "$pairing_worker_url_override" ]]; then pairing_worker_url="$pairing_worker_url_override"; fi
    if [[ -n "$receiver_id_override" ]]; then receiver_id="$receiver_id_override"; fi
    if [[ -n "$receiver_registration_secret_override" ]]; then receiver_registration_secret="$receiver_registration_secret_override"; fi
    if [[ -n "$pairing_token_public_key_file_override" ]]; then pairing_token_public_key_file="$pairing_token_public_key_file_override"; fi

    [[ -n "$board_ip" ]] || { echo "A board address is required; use --board-ip=<address>." >&2; exit 2; }
    case "$cloud_discovery_enabled" in
      1|true|TRUE|yes|YES) cloud_discovery_enabled=1 ;;
      0|false|FALSE|no|NO|"") cloud_discovery_enabled=0 ;;
      *)
        echo "Cloud discovery must be configured as true or false." >&2
        exit 2
        ;;
    esac
    if [[ "$cloud_discovery_enabled" == 1 && -n "$pairing_code_fixed" ]]; then
      echo "Fixed pairing codes cannot be used with Cloudflare discovery enabled." >&2
      exit 2
    fi
    if [[ -n "$pairing_code_fixed" ]]; then
      echo "[WARNING] Fixed pairing code mode is enabled for this local test deployment."
    fi

    # Hash Dockerfile to detect if GStreamer / OS dependencies changed
    DOCKERFILE_HASH=$(shasum -a 256 "${SCRIPT_DIR}/Dockerfile" | awk '{print $1}')
    REMOTE_HASH=$(ssh -o BatchMode=yes "$board_ip" "cat /var/tmp/llrdc-bin/Dockerfile.sha256 2>/dev/null || true")

    ssh -o BatchMode=yes "$board_ip" "mkdir -p /var/tmp/llrdc-bin && rm -rf /var/tmp/llrdc-bin/llrdc-casting"

    if [ "$DOCKERFILE_HASH" != "$REMOTE_HASH" ]; then
      echo "[DEPLOY] Dockerfile changed or first deploy -> Transferring full base Docker image (278MB)..."
      docker buildx build --build-arg BUILD_DATE="$(date +%s)" --platform linux/arm64 -t "$IMAGE" --load .
      docker save "$IMAGE" | gzip -1 | ssh -o BatchMode=yes "$board_ip" 'gunzip | docker load'
      ssh -o BatchMode=yes "$board_ip" "echo '$DOCKERFILE_HASH' > /var/tmp/llrdc-bin/Dockerfile.sha256"
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
    scp -o BatchMode=yes /tmp/llrdc-casting "${board_ip}:/var/tmp/llrdc-bin/llrdc-casting"
    rm -f /tmp/llrdc-casting

    ssh -o BatchMode=yes "$board_ip" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; sleep 1; docker run -d --name '$IMAGE' --restart unless-stopped --net host --privileged -e DRM_CONNECTOR_ID='$drm_connector_id' -e DRM_PLANE_ID='$drm_plane_id' -e IDLE_DASHBOARD='$idle_dashboard' -e IDLE_DASHBOARD_MODE='$idle_dashboard_mode' -e IDLE_TIMEOUT_SEC='$idle_timeout_sec' -e PAIRING_CODE_TTL_SEC='$pairing_code_ttl_sec' -e PAIRING_CODE_FIXED='$pairing_code_fixed' -e HTTP_PORT='$http_port' -e ADMIN_BIND_ADDR='$admin_bind_address' -e ADMIN_PORT='$admin_port' -e WEBTRANSPORT_PORT='$webtransport_port' -e BOARD_PORT='$board_port' -e UDP_BUFFER_SIZE_MB='$udp_buffer_size_mb' -e CERTS_DIR='$cert_dir' -e CLOUD_DISCOVERY_ENABLED='$cloud_discovery_enabled' -e PAIRING_WORKER_URL='$pairing_worker_url' -e RECEIVER_ID='$receiver_id' -e RECEIVER_REGISTRATION_SECRET='$receiver_registration_secret' -e PAIRING_TOKEN_PUBLIC_KEY_FILE='$pairing_token_public_key_file' -v /dev:/dev -v /var/lib/llrdc-certs:/certs -v /var/lib/llrdc-pairing:/pairing:ro -v /var/tmp/llrdc-bin/llrdc-casting:/usr/local/bin/llrdc-casting '$IMAGE'; sleep 2; docker logs --tail 30 '$IMAGE'"
    ;;
  --stop)
    board_ip_override=""
    while (($#)); do
      case "$1" in
        --board-ip=*) board_ip_override="${1#*=}" ;;
        *) usage; exit 2 ;;
      esac
      shift
    done
    board_ip="${board_ip_override:-$BOARD_IP}"
    [[ -n "$board_ip" ]] || { echo "A board address is required; use --board-ip=<address>." >&2; exit 2; }
    ssh -o BatchMode=yes "$board_ip" "docker stop -t 2 '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true; docker rm -f '$IMAGE' rock5c-v4l2-drm 2>/dev/null || true"
    ;;
  *) usage; exit 2 ;;
esac
