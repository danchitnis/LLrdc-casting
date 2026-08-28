#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="llrdc-casting"
ROLLBACK_RUNTIME_IMAGE="$IMAGE"
SSH_OPTIONS=(
  -o BatchMode=yes
  -o ConnectTimeout=10
  -o ServerAliveInterval=5
  -o ServerAliveCountMax=3
)
DEPLOY_TMP_DIR=""
ARTIFACT_CONTAINER_ID=""
DEVICE_CONFIG_DIR="/var/lib/llrdc-config"
DEVICE_SECRETS_DIR="/var/lib/llrdc-secrets"
DEVICE_MANAGEMENT_DIR="/var/lib/llrdc-management"

die() {
  echo "[ERROR] $*" >&2
  exit 1
}

on_error() {
  local exit_code=$?
  trap - ERR
  echo "[ERROR] server.sh failed at line ${BASH_LINENO[0]:-unknown} (exit ${exit_code})." >&2
  exit "$exit_code"
}

cleanup() {
  if [[ -n "$ARTIFACT_CONTAINER_ID" ]]; then
    docker rm -f "$ARTIFACT_CONTAINER_ID" >/dev/null 2>&1 || true
  fi
  if [[ -n "$DEPLOY_TMP_DIR" && -d "$DEPLOY_TMP_DIR" ]]; then
    rm -rf -- "$DEPLOY_TMP_DIR"
  fi
}

trap on_error ERR
trap cleanup EXIT

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "Required command is unavailable: $1"
}

validate_positive_integer() {
  local name="$1"
  local value="$2"
  [[ "$value" =~ ^[1-9][0-9]*$ ]] || die "$name must be a positive integer (received '$value')."
}

validate_port() {
  local name="$1"
  local value="$2"
  validate_positive_integer "$name" "$value"
  ((value <= 65535)) || die "$name must be between 1 and 65535 (received '$value')."
}

validate_single_line() {
  local name="$1"
  local value="$2"
  [[ "$value" != *$'\n'* && "$value" != *$'\r'* ]] || die "$name must not contain a newline."
}

ensure_local_docker() {
  require_command docker
  docker info >/dev/null 2>&1 || die "Docker is not running or is not accessible."
  docker buildx version >/dev/null 2>&1 || die "Docker Buildx is unavailable."
}

# Load config.yaml if present
load_config() {
  local cfg="${SCRIPT_DIR}/config.yaml"
  if [ -f "$cfg" ]; then
    local parsed_config
    require_command python3
    if ! parsed_config="$(python3 -c '
import shlex
import re
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
            if not re.fullmatch(r"[A-Z_][A-Z0-9_]*", full_key):
                raise ValueError(f"invalid configuration key: {full_key}")
            if v:
                print(f"export {full_key}={shlex.quote(v)}")
' "$cfg")"; then
      die "Could not parse configuration file: $cfg"
    fi
    eval "$parsed_config"
  fi
}

PRE_BOARD_IP="${BOARD_IP:-}"
PRE_CONNECTOR_ID="${DRM_CONNECTOR_ID:-}"
PRE_CLOUD_DISCOVERY_ENABLED="${CLOUD_DISCOVERY_ENABLED:-}"
PRE_PAIRING_WORKER_URL="${PAIRING_WORKER_URL:-}"
PRE_RECEIVER_ID="${RECEIVER_ID:-}"
PRE_RECEIVER_REGISTRATION_SECRET="${RECEIVER_REGISTRATION_SECRET:-}"
PRE_PAIRING_TOKEN_PUBLIC_KEY_FILE="${PAIRING_TOKEN_PUBLIC_KEY_FILE:-}"
PRE_LOCAL_PAIRING_CODE_REQUIRED="${LOCAL_PAIRING_CODE_REQUIRED:-}"
load_config
# Optional generated Cloudflare receiver credentials. Provisioning supplies
# identity and secret material, but never controls the cloud enable flag.
CONFIG_CLOUD_DISCOVERY_ENABLED="${SERVER_CLOUD_DISCOVERY_ENABLED:-}"
RECEIVER_ENV_FILE="${SCRIPT_DIR}/.cloudflare/receiver.env"
if [ -f "$RECEIVER_ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$RECEIVER_ENV_FILE"
  set +a
fi
if [[ -n "$CONFIG_CLOUD_DISCOVERY_ENABLED" ]]; then SERVER_CLOUD_DISCOVERY_ENABLED="$CONFIG_CLOUD_DISCOVERY_ENABLED"; else unset SERVER_CLOUD_DISCOVERY_ENABLED || true; fi
if [ -n "$PRE_BOARD_IP" ]; then BOARD_IP="$PRE_BOARD_IP"; fi
if [ -n "$PRE_PAIRING_WORKER_URL" ]; then SERVER_PAIRING_WORKER_URL="$PRE_PAIRING_WORKER_URL"; fi
if [ -n "$PRE_CLOUD_DISCOVERY_ENABLED" ]; then SERVER_CLOUD_DISCOVERY_ENABLED="$PRE_CLOUD_DISCOVERY_ENABLED"; fi
if [ -n "$PRE_RECEIVER_ID" ]; then SERVER_RECEIVER_ID="$PRE_RECEIVER_ID"; fi
if [ -n "$PRE_RECEIVER_REGISTRATION_SECRET" ]; then SERVER_RECEIVER_REGISTRATION_SECRET="$PRE_RECEIVER_REGISTRATION_SECRET"; fi
if [ -n "$PRE_PAIRING_TOKEN_PUBLIC_KEY_FILE" ]; then SERVER_PAIRING_TOKEN_PUBLIC_KEY_FILE="$PRE_PAIRING_TOKEN_PUBLIC_KEY_FILE"; fi
if [ -n "$PRE_LOCAL_PAIRING_CODE_REQUIRED" ]; then SERVER_LOCAL_PAIRING_CODE_REQUIRED="$PRE_LOCAL_PAIRING_CODE_REQUIRED"; fi
BOARD_IP="${BOARD_IP:-}"
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
  echo "  --sender-liveness-timeout-sec=<seconds>     Sender heartbeat grace (default: 90)"
  echo "  --pairing-code-ttl-sec=<seconds>            Pairing-code lifetime (default: 3600)"
  echo "  --pairing-code-required=true|false          Require a code for direct LAN clients"
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

remote_ssh() {
  ssh "${SSH_OPTIONS[@]}" "$board_ip" "$@"
}

check_remote_prerequisites() {
  echo "[PREFLIGHT] Checking Docker and disk space on ${board_ip}..."
  remote_ssh '
    set -eu
    command -v docker >/dev/null
    command -v sha256sum >/dev/null
    docker info >/dev/null
    available_kb=$(df -Pk /var/tmp | awk "NR==2 {print \$4}")
    [ -n "$available_kb" ] && [ "$available_kb" -ge 131072 ]
  ' || die "Board preflight failed: SSH, Docker, sha256sum, or 128 MiB of /var/tmp space is unavailable."
}

write_runtime_env() {
  local destination="$1"
  umask 077
  {
    printf 'DRM_CONNECTOR_ID=%s\n' "$drm_connector_id"
    printf 'DRM_PLANE_ID=%s\n' "$drm_plane_id"
    printf 'IDLE_DASHBOARD=%s\n' "$idle_dashboard"
    printf 'IDLE_DASHBOARD_MODE=%s\n' "$idle_dashboard_mode"
    printf 'IDLE_TIMEOUT_SEC=%s\n' "$idle_timeout_sec"
    printf 'SENDER_LIVENESS_TIMEOUT_SEC=%s\n' "$sender_liveness_timeout_sec"
    printf 'PAIRING_CODE_TTL_SEC=%s\n' "$pairing_code_ttl_sec"
    printf 'LOCAL_PAIRING_CODE_REQUIRED=%s\n' "$local_pairing_code_required"
    printf 'PAIRING_CODE_FIXED=%s\n' "$pairing_code_fixed"
    printf 'HTTP_PORT=%s\n' "$http_port"
    printf 'ADMIN_BIND_ADDR=%s\n' "$admin_bind_address"
    printf 'ADMIN_PORT=%s\n' "$admin_port"
    printf 'WEBTRANSPORT_PORT=%s\n' "$webtransport_port"
    printf 'BOARD_PORT=%s\n' "$board_port"
    printf 'UDP_BUFFER_SIZE_MB=%s\n' "$udp_buffer_size_mb"
    printf 'CERTS_DIR=%s\n' "$cert_dir"
    printf 'CLOUD_DISCOVERY_ENABLED=%s\n' "$cloud_discovery_enabled"
    printf 'PAIRING_WORKER_URL=%s\n' "$pairing_worker_url"
    printf 'RECEIVER_ID=%s\n' "$receiver_id"
    printf 'RECEIVER_REGISTRATION_SECRET=%s\n' "$receiver_registration_secret"
    printf 'PAIRING_TOKEN_PUBLIC_KEY_FILE=%s\n' "$pairing_token_public_key_file"
    printf 'LLRDC_CODEC_DIAGNOSTICS=%s\n' "$codec_diagnostics"
  } >"$destination"
}

write_device_config() {
  local destination="$1"
  python3 - "$destination" "$board_port" "$webtransport_port" "$http_port" "$admin_bind_address" "$admin_port" "$drm_connector_id" "$drm_plane_id" "$idle_dashboard" "$idle_dashboard_mode" "$idle_timeout_sec" "$sender_liveness_timeout_sec" "$udp_buffer_size_mb" "$cert_dir" "$pairing_worker_url" "$cloud_discovery_enabled" "$receiver_id" "$pairing_code_ttl_sec" "$local_pairing_code_required" "$pairing_token_public_key_file" <<'PY'
import json, sys
out, port, wt, http, bind, admin, connector, plane, dashboard, mode, idle, liveness, buffer, certs, worker, cloud, receiver, ttl, pairing_required, key = sys.argv[1:]
def scalar(name, value):
    if name in ('idle_dashboard', 'cloud_discovery_enabled', 'local_pairing_code_required'):
        return 'true' if value == '1' else 'false'
    if name in ('port', 'webtransport_port', 'http_port', 'admin_port', 'idle_timeout_sec', 'sender_liveness_timeout_sec', 'udp_buffer_size_mb', 'pairing_code_ttl_sec'):
        return value
    return json.dumps(value)
lines = ['version: 1', 'server:']
for name, value in [('port', port), ('webtransport_port', wt), ('http_port', http), ('admin_bind_address', bind), ('admin_port', admin), ('drm_connector_id', connector), ('drm_plane_id', plane), ('idle_dashboard', dashboard), ('idle_dashboard_mode', mode), ('idle_timeout_sec', idle), ('sender_liveness_timeout_sec', liveness), ('udp_buffer_size_mb', buffer), ('cert_dir', certs), ('pairing_worker_url', worker), ('cloud_discovery_enabled', cloud), ('receiver_id', receiver), ('pairing_code_ttl_sec', ttl), ('local_pairing_code_required', pairing_required), ('pairing_token_public_key_file', key)]:
    lines.append(f'  {name}: {scalar(name, value)}')
with open(out, 'w', encoding='utf-8') as stream:
    stream.write('\n'.join(lines) + '\n')
PY
}

start_remote_container() {
  local runtime_image="$1"
  remote_ssh "
    set -eu
    env_file='$DEVICE_SECRETS_DIR/runtime.env'
    if [ -s /var/tmp/llrdc-bin/runtime.env.new ]; then env_file=/var/tmp/llrdc-bin/runtime.env.new; fi
    for existing in '$IMAGE' rock5c-v4l2-drm; do
      if docker container inspect \"\$existing\" >/dev/null 2>&1; then
        docker stop -t 2 \"\$existing\" >/dev/null 2>&1 || docker kill \"\$existing\" >/dev/null
        docker rm -f \"\$existing\" >/dev/null
      fi
    done
    docker run -d --name '$IMAGE' --restart unless-stopped --net host --privileged \
      --env-file "\$env_file" \
      -v /dev:/dev \
      -v /var/lib/llrdc-certs:/certs \
      -v /var/lib/llrdc-pairing:/pairing:ro \
      -v '$DEVICE_SECRETS_DIR:/secrets:ro' \
      -v '$DEVICE_CONFIG_DIR:/config:rw' \
      -v '$DEVICE_MANAGEMENT_DIR:/management:rw' \
      -v /var/tmp/llrdc-bin/llrdc-casting:/usr/local/bin/llrdc-casting:ro \
      -v /var/tmp/llrdc-bin/llrdc-management:/usr/local/bin/llrdc-management:ro \
      '$runtime_image' >/dev/null
  "
}

wait_for_receiver() {
  local probe_host="$admin_bind_address"
  if [[ "$probe_host" == "0.0.0.0" || "$probe_host" == "::" || -z "$probe_host" ]]; then
    probe_host="$board_ip"
  fi
  local health_url="https://${probe_host}:${admin_port}/health"
  local attempt
  local healthy_streak=0
  for ((attempt = 1; attempt <= 25; attempt++)); do
    if remote_ssh "test \"\$(docker inspect -f '{{.State.Running}}' '$IMAGE' 2>/dev/null)\" = true" >/dev/null 2>&1 \
      && [[ "$(curl -fsSk --connect-timeout 2 --max-time 3 "$health_url" 2>/dev/null || true)" == "OK" ]]; then
      ((healthy_streak += 1))
      if ((healthy_streak >= 3)); then
        return 0
      fi
    else
      healthy_streak=0
    fi
    sleep 1
  done
  return 1
}

show_receiver_logs() {
  remote_ssh "docker logs --tail 80 '$IMAGE' 2>&1 || true" >&2 || true
}

rollback_remote_deployment() {
  echo "[ROLLBACK] Restoring the previous receiver binary and runtime configuration..." >&2
  if ! remote_ssh "
    set -eu
    test -s /var/tmp/llrdc-bin/llrdc-casting.previous
    test -s /var/tmp/llrdc-bin/llrdc-management.previous
    cp -p /var/tmp/llrdc-bin/llrdc-casting.previous /var/tmp/llrdc-bin/llrdc-casting.rollback
    mv -f /var/tmp/llrdc-bin/llrdc-casting.rollback /var/tmp/llrdc-bin/llrdc-casting
    cp -p /var/tmp/llrdc-bin/llrdc-management.previous /var/tmp/llrdc-bin/llrdc-management.rollback
    mv -f /var/tmp/llrdc-bin/llrdc-management.rollback /var/tmp/llrdc-bin/llrdc-management
    docker run --rm --entrypoint /bin/sh -v /var/tmp/llrdc-bin:/stage -v '$DEVICE_CONFIG_DIR:/config' -v '$DEVICE_SECRETS_DIR:/secrets' '$IMAGE' -c 'set -eu; if [ -s /stage/runtime.env.previous ]; then cp -p /stage/runtime.env.previous /secrets/runtime.env.rollback; mv -f /secrets/runtime.env.rollback /secrets/runtime.env; chown root:root /secrets/runtime.env; chmod 0600 /secrets/runtime.env; fi; if [ -s /stage/config.yaml.previous ]; then cp -p /stage/config.yaml.previous /config/config.yaml.rollback; mv -f /config/config.yaml.rollback /config/config.yaml; chown root:root /config/config.yaml; chmod 0640 /config/config.yaml; fi'
    rm -f /var/tmp/llrdc-bin/runtime.env.new
  "; then
    echo "[ROLLBACK] No valid previous deployment is available." >&2
    return 1
  fi

  start_remote_container "$ROLLBACK_RUNTIME_IMAGE" || return 1
  wait_for_receiver
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
    ensure_local_docker
    echo "[TEST] Running Rust unit tests in the ARM64 Docker builder..."
    docker buildx build \
      --build-arg BUILD_DATE="$(date +%s)" \
      --target tests \
      --platform linux/arm64 \
      --tag "${IMAGE}-tests" \
      --load \
      .
    echo "[TEST] ARM64 Rust tests passed."
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
    [[ -n "$board_ip" ]] || die "A board address is required; use --board-ip=<address>."
    [[ "$board_ip" != -* && "$board_ip" != *[[:space:]]* ]] || die "Invalid board address."
    require_command ssh
    pairing_code="$(remote_ssh "docker inspect -f '{{.State.Running}}' '$IMAGE' 2>/dev/null | grep -qx true && docker exec '$IMAGE' /usr/local/bin/llrdc-management admin pairing-code")" \
      || die "The receiver container is not running or its pairing-code command failed."
    [[ "$pairing_code" =~ ^[A-Z0-9]{4}$ ]] || die "The receiver returned an invalid pairing code."
    printf '%s\n' "$pairing_code"
    ;;
  --start)
    board_ip_override=""
    drm_connector_override=""
    drm_plane_override=""
    dashboard_override=""
    dashboard_mode_override=""
    idle_timeout_override=""
    sender_liveness_timeout_override=""
    pairing_code_ttl_override=""
    pairing_code_required_override=""
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
        --sender-liveness-timeout-sec=*) sender_liveness_timeout_override="${1#*=}" ;;
        --pairing-code-ttl-sec=*) pairing_code_ttl_override="${1#*=}" ;;
        --pairing-code-required=true) pairing_code_required_override=1 ;;
        --pairing-code-required=false) pairing_code_required_override=0 ;;
        --pairing-code-required=*)
          echo "Pairing-code requirement must be configured as true or false." >&2
          exit 2
          ;;
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
          if ! [[ "$pairing_code_fixed" =~ ^[A-Za-z0-9]{4}$ ]]; then
            echo "Pairing code must contain exactly four letters or numbers." >&2
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
    sender_liveness_timeout_sec="${SENDER_LIVENESS_TIMEOUT_SEC:-${SERVER_SENDER_LIVENESS_TIMEOUT_SEC:-${BOARD_SENDER_LIVENESS_TIMEOUT_SEC:-90}}}"
    pairing_code_ttl_sec="${SERVER_PAIRING_CODE_TTL_SEC:-${BOARD_PAIRING_CODE_TTL_SEC:-3600}}"
    local_pairing_code_required="${SERVER_LOCAL_PAIRING_CODE_REQUIRED:-${BOARD_LOCAL_PAIRING_CODE_REQUIRED:-1}}"
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
    codec_diagnostics="${LLRDC_CODEC_DIAGNOSTICS:-0}"

    if [[ -n "$dashboard_override" ]]; then idle_dashboard="$dashboard_override"; fi
    if [[ -n "$dashboard_mode_override" ]]; then idle_dashboard_mode="$dashboard_mode_override"; fi
    if [[ -n "$idle_timeout_override" ]]; then idle_timeout_sec="$idle_timeout_override"; fi
    if [[ -n "$sender_liveness_timeout_override" ]]; then sender_liveness_timeout_sec="$sender_liveness_timeout_override"; fi
    if [[ -n "$pairing_code_ttl_override" ]]; then pairing_code_ttl_sec="$pairing_code_ttl_override"; fi
    if [[ -n "$pairing_code_required_override" ]]; then local_pairing_code_required="$pairing_code_required_override"; fi
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

    [[ -n "$board_ip" ]] || die "A board address is required; use --board-ip=<address>."
    [[ "$board_ip" != -* && "$board_ip" != *[[:space:]]* ]] || die "Invalid board address."
    [[ "$drm_connector_id" == "auto" || "$drm_connector_id" =~ ^[0-9]+$ ]] || die "DRM connector must be 'auto' or a numeric ID."
    [[ "$drm_plane_id" =~ ^[0-9]+$ ]] || die "DRM plane must be a numeric ID."
    case "$idle_dashboard" in
      1|true|TRUE|yes|YES) idle_dashboard=1 ;;
      0|false|FALSE|no|NO) idle_dashboard=0 ;;
      *) die "Idle dashboard must be configured as true or false." ;;
    esac
    [[ "$idle_dashboard_mode" == "raw" || "$idle_dashboard_mode" == "hevc" ]] || die "Dashboard mode must be 'raw' or 'hevc'."
    validate_positive_integer "Idle timeout" "$idle_timeout_sec"
    validate_positive_integer "Sender liveness timeout" "$sender_liveness_timeout_sec"
    validate_positive_integer "Pairing-code lifetime" "$pairing_code_ttl_sec"
    case "$local_pairing_code_required" in
      1|true|TRUE|yes|YES) local_pairing_code_required=1 ;;
      0|false|FALSE|no|NO) local_pairing_code_required=0 ;;
      *) die "Pairing-code requirement must be configured as true or false." ;;
    esac
    validate_port "HTTP port" "$http_port"
    validate_port "Admin port" "$admin_port"
    validate_port "WebTransport port" "$webtransport_port"
    validate_port "Video UDP port" "$board_port"
    validate_positive_integer "UDP buffer size" "$udp_buffer_size_mb"
    for port_pair in "$http_port:$admin_port" "$http_port:$webtransport_port" "$http_port:$board_port" "$admin_port:$webtransport_port" "$admin_port:$board_port" "$webtransport_port:$board_port"; do
      [[ "${port_pair%%:*}" != "${port_pair##*:}" ]] || die "Receiver ports must be unique."
    done
    case "$cloud_discovery_enabled" in
      1|true|TRUE|yes|YES) cloud_discovery_enabled=1 ;;
      0|false|FALSE|no|NO|"") cloud_discovery_enabled=0 ;;
      *)
        echo "Cloud discovery must be configured as true or false." >&2
        exit 2
        ;;
    esac
    case "$codec_diagnostics" in
      1|true|TRUE|yes|YES) codec_diagnostics=1 ;;
      0|false|FALSE|no|NO|"") codec_diagnostics=0 ;;
      *) die "Codec diagnostics must be configured as true or false." ;;
    esac
    if [[ "$cloud_discovery_enabled" == 1 && -n "$pairing_code_fixed" ]]; then
      echo "Fixed pairing codes cannot be used with Cloudflare discovery enabled." >&2
      exit 2
    fi
    if [[ -n "$pairing_code_fixed" ]]; then
      echo "[WARNING] Fixed pairing code mode is enabled for this local test deployment."
    fi
    for env_value in \
      "$drm_connector_id" "$drm_plane_id" "$idle_dashboard_mode" "$pairing_code_fixed" \
      "$admin_bind_address" "$cert_dir" "$pairing_worker_url" "$receiver_id" \
      "$receiver_registration_secret" "$pairing_token_public_key_file"; do
      validate_single_line "Receiver configuration value" "$env_value"
    done

    require_command ssh
    require_command scp
    require_command shasum
    require_command gzip
    require_command curl
    require_command mktemp
    ensure_local_docker
    check_remote_prerequisites

    DEPLOY_TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/llrdc-deploy.XXXXXX")"
    local_binary="${DEPLOY_TMP_DIR}/llrdc-casting"
    local_management_binary="${DEPLOY_TMP_DIR}/llrdc-management"
    local_runtime_env="${DEPLOY_TMP_DIR}/runtime.env"
    local_device_config="${DEPLOY_TMP_DIR}/config.yaml"
    write_runtime_env "$local_runtime_env"
    write_device_config "$local_device_config"

    remote_ssh "mkdir -p /var/tmp/llrdc-bin && docker run --rm --entrypoint /bin/sh -v '$DEVICE_MANAGEMENT_DIR:/management' '$IMAGE' -c 'chmod 0700 /management' && rm -f /var/tmp/llrdc-bin/llrdc-casting.new /var/tmp/llrdc-bin/llrdc-management.new /var/tmp/llrdc-bin/runtime.env.new /var/tmp/llrdc-bin/config.yaml.new"

    # Hash Dockerfile to detect whether the complete runtime image must be transferred.
    dockerfile_hash="$(shasum -a 256 "${SCRIPT_DIR}/Dockerfile" | awk '{print $1}')"
    remote_hash="$(remote_ssh "cat /var/tmp/llrdc-bin/Dockerfile.sha256 2>/dev/null || true")"
    if [[ -n "$remote_hash" && ! "$remote_hash" =~ ^[0-9a-f]{64}$ ]]; then
      echo "[WARNING] Ignoring an invalid remote Dockerfile hash; forcing a full image deployment." >&2
      remote_hash=""
    fi

    build_date="$(date +%s)"
    if [[ "$dockerfile_hash" != "$remote_hash" ]]; then
      echo "[DEPLOY] Runtime dependencies changed; building and transferring the full ARM64 image..."
      if remote_ssh "docker image inspect '$IMAGE' >/dev/null 2>&1"; then
        remote_ssh "docker tag '$IMAGE' '${IMAGE}:rollback'"
        ROLLBACK_RUNTIME_IMAGE="${IMAGE}:rollback"
      fi
      docker buildx build --build-arg BUILD_DATE="$build_date" --platform linux/arm64 -t "$IMAGE" --load .
      docker save "$IMAGE" | gzip -1 | remote_ssh "bash -o pipefail -c 'gunzip | docker load'"
      artifact_image="$IMAGE"
      artifact_path="/usr/local/bin"
    else
      echo "[DEPLOY] Runtime dependencies unchanged; building the ARM64 binary..."
      docker buildx build --build-arg BUILD_DATE="$build_date" --target builder --platform linux/arm64 -t "${IMAGE}-builder" --load .
      artifact_image="${IMAGE}-builder"
      artifact_path="/app/target/release"
    fi

    artifact_arch="$(docker image inspect --format '{{.Architecture}}' "$artifact_image")"
    [[ "$artifact_arch" == "arm64" ]] || die "Built artifact has architecture '$artifact_arch', expected 'arm64'."
    ARTIFACT_CONTAINER_ID="$(docker create "$artifact_image")"
    docker cp "${ARTIFACT_CONTAINER_ID}:${artifact_path}/llrdc-casting" "$local_binary"
    docker cp "${ARTIFACT_CONTAINER_ID}:${artifact_path}/llrdc-management" "$local_management_binary"
    docker rm "$ARTIFACT_CONTAINER_ID" >/dev/null
    ARTIFACT_CONTAINER_ID=""
    [[ -s "$local_binary" ]] || die "The built receiver binary is empty or missing."
    [[ -s "$local_management_binary" ]] || die "The built management binary is empty or missing."

    local_binary_hash="$(shasum -a 256 "$local_binary" | awk '{print $1}')"
    local_management_hash="$(shasum -a 256 "$local_management_binary" | awk '{print $1}')"
    binary_size="$(ls -lh "$local_binary" | awk '{print $5}')"
    echo "[TRANSFER] Uploading verified receiver binary (${binary_size})..."
    scp "${SSH_OPTIONS[@]}" "$local_binary" "${board_ip}:/var/tmp/llrdc-bin/llrdc-casting.new"
    scp "${SSH_OPTIONS[@]}" "$local_management_binary" "${board_ip}:/var/tmp/llrdc-bin/llrdc-management.new"
    scp "${SSH_OPTIONS[@]}" "$local_runtime_env" "${board_ip}:/var/tmp/llrdc-bin/runtime.env.new"
    scp "${SSH_OPTIONS[@]}" "$local_device_config" "${board_ip}:/var/tmp/llrdc-bin/config.yaml.new"
    remote_binary_hash="$(remote_ssh "sha256sum /var/tmp/llrdc-bin/llrdc-casting.new | awk '{print \$1}'")"
    [[ "$remote_binary_hash" == "$local_binary_hash" ]] || die "Transferred binary checksum mismatch. The active receiver was not changed."
    remote_management_hash="$(remote_ssh "sha256sum /var/tmp/llrdc-bin/llrdc-management.new | awk '{print \$1}'")"
    [[ "$remote_management_hash" == "$local_management_hash" ]] || die "Transferred management binary checksum mismatch. The active receiver was not changed."

    remote_ssh "
      set -eu
      if [ -s /var/tmp/llrdc-bin/llrdc-casting ]; then cp -p /var/tmp/llrdc-bin/llrdc-casting /var/tmp/llrdc-bin/llrdc-casting.previous; fi
      if [ -s /var/tmp/llrdc-bin/llrdc-management ]; then cp -p /var/tmp/llrdc-bin/llrdc-management /var/tmp/llrdc-bin/llrdc-management.previous; fi
      chmod 0755 /var/tmp/llrdc-bin/llrdc-casting.new
      chmod 0755 /var/tmp/llrdc-bin/llrdc-management.new
      mv -f /var/tmp/llrdc-bin/llrdc-casting.new /var/tmp/llrdc-bin/llrdc-casting
      mv -f /var/tmp/llrdc-bin/llrdc-management.new /var/tmp/llrdc-bin/llrdc-management
      docker run --rm --entrypoint /bin/sh -v /var/tmp/llrdc-bin:/stage -v '$DEVICE_CONFIG_DIR:/config' -v '$DEVICE_SECRETS_DIR:/secrets' '$IMAGE' -c 'set -eu; if [ -s /secrets/runtime.env ]; then cp -p /secrets/runtime.env /stage/runtime.env.previous; elif [ -s /stage/runtime.env ]; then cp -p /stage/runtime.env /stage/runtime.env.previous; fi; if [ -s /config/config.yaml ]; then cp -p /config/config.yaml /stage/config.yaml.previous; fi; chmod 0600 /stage/runtime.env.new; chmod 0640 /stage/config.yaml.new; mv -f /stage/config.yaml.new /config/config.yaml; chown root:root /config/config.yaml; chmod 0640 /config/config.yaml'
    "

    if ! start_remote_container "$IMAGE"; then
      if rollback_remote_deployment; then
        die "New receiver container could not start; the previous deployment was restored."
      fi
      die "New receiver container could not start and rollback also failed."
    fi
    echo "[VERIFY] Waiting for watchdog readiness and stable HTTPS health..."
    if ! wait_for_receiver; then
      echo "[ERROR] Receiver did not become stably healthy within 25 seconds." >&2
      show_receiver_logs
      if rollback_remote_deployment; then
        die "Deployment failed readiness checks; the previous deployment was restored."
      fi
      die "Deployment failed readiness checks and rollback also failed."
    fi

    remote_ssh "docker run --rm --entrypoint /bin/sh -v /var/tmp/llrdc-bin:/stage -v '$DEVICE_SECRETS_DIR:/secrets' '$IMAGE' -c 'set -eu; mv -f /stage/runtime.env.new /secrets/runtime.env; chown root:root /secrets/runtime.env; chmod 0600 /secrets/runtime.env'"

    # Secrets and superseded configuration remain only in the durable,
    # root-owned locations after the new container is healthy.
    remote_ssh "rm -f /var/tmp/llrdc-bin/runtime.env /var/tmp/llrdc-bin/runtime.env.new /var/tmp/llrdc-bin/runtime.env.previous /var/tmp/llrdc-bin/config.yaml.previous"
    remote_ssh "printf '%s\\n' '$dockerfile_hash' > /var/tmp/llrdc-bin/Dockerfile.sha256 && printf '%s\\n' '$local_binary_hash' > /var/tmp/llrdc-bin/llrdc-casting.sha256 && printf '%s\\n' '$local_management_hash' > /var/tmp/llrdc-bin/llrdc-management.sha256"
    remote_ssh "docker logs --tail 30 '$IMAGE' 2>&1"
    echo "[DEPLOY] Manager and receiver are healthy; checksums ${local_management_hash:0:12}.../${local_binary_hash:0:12}..."
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
    [[ -n "$board_ip" ]] || die "A board address is required; use --board-ip=<address>."
    [[ "$board_ip" != -* && "$board_ip" != *[[:space:]]* ]] || die "Invalid board address."
    require_command ssh
    remote_ssh "
      set -eu
      for existing in '$IMAGE' rock5c-v4l2-drm; do
        if docker container inspect \"\$existing\" >/dev/null 2>&1; then
          docker stop -t 2 \"\$existing\" >/dev/null 2>&1 || docker kill \"\$existing\" >/dev/null
          docker rm -f \"\$existing\" >/dev/null
        fi
        if docker container inspect \"\$existing\" >/dev/null 2>&1; then
          echo \"Container \$existing could not be removed.\" >&2
          exit 1
        fi
      done
    "
    echo "[STOP] Receiver containers are stopped and removed."
    ;;
  *) usage; exit 2 ;;
esac
