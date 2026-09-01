#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MODE="${1:-}"
shift || true

usage() {
  cat <<'EOF'
Usage: ./test_browser.sh <codec|cloud|management|all> [chrome|safari] [--board-ip=<address>]

codec  Run the local codec suite in branded Chrome (default) or installed Safari.
cloud  Deploy with Cloudflare enabled and run pairing plus one HEVC stream.
management  Deploy a private fixed-code receiver and run the settings/pairing suite.
all    Run the Chrome codec suite first, then cloud.

Safari is run separately: ./test_browser.sh codec safari
EOF
}

if [[ "$MODE" != "codec" && "$MODE" != "cloud" && "$MODE" != "management" && "$MODE" != "all" ]]; then
  usage >&2
  exit 2
fi

codec_browser="chrome"
if [[ "$MODE" == "codec" && "${1:-}" != "" && "${1:-}" != --* ]]; then
  codec_browser="$1"
  shift
fi
if [[ "$MODE" != "codec" && "$codec_browser" != "chrome" ]]; then
  echo "A browser selector is only valid with codec: chrome or safari." >&2
  usage >&2
  exit 2
fi
if [[ "$MODE" == "codec" && "$codec_browser" != "chrome" && "$codec_browser" != "safari" ]]; then
  echo "Unknown codec browser '$codec_browser'; choose chrome or safari." >&2
  usage >&2
  exit 2
fi

board_ip="${BOARD_IP:-}"
while (($#)); do
  case "$1" in
    --board-ip=*) board_ip="${1#*=}" ;;
    --board-ip)
      (($# >= 2)) || { echo "--board-ip requires an address" >&2; exit 2; }
      board_ip="$2"
      shift
      ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [[ -z "$board_ip" && -f "$SCRIPT_DIR/config.yaml" ]]; then
  board_ip="$(awk '
    $0 ~ /^board:[[:space:]]*$/ { in_board=1; next }
    in_board && $0 ~ /^[^[:space:]]/ { in_board=0 }
    in_board && $1 == "ip:" { gsub(/"/, "", $2); print $2; exit }
  ' "$SCRIPT_DIR/config.yaml")"
fi
[[ -n "$board_ip" ]] || { echo "A board address is required; use --board-ip=<address>." >&2; exit 2; }
admin_ip="$board_ip"

network_type="unknown"
receiver_interface="unknown"
if [[ "$board_ip" =~ ^10\.[0-9]+\.[0-9]+\.[0-9]+$ \
   || "$board_ip" =~ ^192\.168\.[0-9]+\.[0-9]+$ \
   || "$board_ip" =~ ^172\.(1[6-9]|2[0-9]|3[01])\.[0-9]+\.[0-9]+$ ]]; then
  receiver_interface="$(ssh -o BatchMode=yes "$board_ip" "ip -o -4 addr show | awk -v address='$board_ip' '{ split(\$4, parts, \"/\"); if (parts[1] == address) { print \$2; exit } }'" 2>/dev/null || true)"
  case "$receiver_interface" in
    eth*|end*|enp*|eno*|ens*|enx*) network_type="wired_lan" ;;
    wlan*|wlp*|wl*) network_type="wifi_lan" ;;
    *) network_type="private_lan" ;;
  esac
elif [[ "$board_ip" =~ ^100\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  network_type="tailscale"
  receiver_interface="tailscale"
fi

resolve_admin_ip() {
  local configured
  configured="$(ssh -o BatchMode=yes "$board_ip" "docker exec llrdc-casting awk '/^[[:space:]]*admin_bind_address:/ { gsub(/[\"[:space:]]/, \"\", \$2); print \$2; exit }' /config/config.yaml" 2>/dev/null || true)"
  if [[ -n "$configured" && "$configured" != -* && "$configured" != *[[:space:]]* ]]; then
    admin_ip="$configured"
  fi
}

if [[ "$MODE" != "codec" || "$codec_browser" == "chrome" ]] && [[ ! -x "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" ]] && ! command -v google-chrome >/dev/null 2>&1; then
  echo "Installed branded Google Chrome is required; bundled Playwright Chromium is intentionally not used." >&2
  exit 2
fi
if [[ "$MODE" == "codec" && "$codec_browser" == "safari" ]] && ! command -v safaridriver >/dev/null 2>&1; then
  echo "Safari codec testing requires the locally installed safaridriver." >&2
  exit 2
fi

safari_remote_automation_hint() {
  echo "[E2E] Safari remote automation is required. Enable Safari > Settings > Advanced > Show features for web developers, then Develop > Allow Remote Automation." >&2
}

artifact_root="$SCRIPT_DIR/.artefact"
if [[ "${E2E_SKIP_CLEAN:-0}" != 1 ]]; then
  echo "[E2E] Cleaning artifact directory: $artifact_root"
  mkdir -p "$artifact_root"
  find "$artifact_root" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
fi

if [[ "$MODE" == "all" ]]; then
  all_run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
  all_artifact_dir="$artifact_root/all-$all_run_id"
  E2E_ARTIFACT_DIR="$all_artifact_dir/codec" E2E_SKIP_CLEAN=1 \
    "$SCRIPT_DIR/test_browser.sh" codec chrome --board-ip="$board_ip"
  E2E_ARTIFACT_DIR="$all_artifact_dir/cloud" E2E_SKIP_CLEAN=1 \
    "$SCRIPT_DIR/test_browser.sh" cloud --board-ip="$board_ip"
  exit 0
fi

run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
artifact_dir="${E2E_ARTIFACT_DIR:-$artifact_root/$MODE-$run_id}"
mkdir -p "$artifact_dir"

collect_receiver_logs() {
  local log_file="$artifact_dir/receiver.log"
  ssh -o BatchMode=yes "$board_ip" 'docker logs --timestamps llrdc-casting 2>&1' 2>/dev/null \
    | sed -E \
      -e 's/(token=)[^&[:space:]]+/\1[REDACTED]/g' \
      -e 's/(connection_token[^:]*: *"?)[A-Za-z0-9._-]+/\1[REDACTED]/g' \
      -e "s/${pairing_code:-UNSET}/[REDACTED-CODE]/g" \
    > "$log_file" || true
}

safari_driver_pid=""
stop_safari_driver() {
  if [[ -n "$safari_driver_pid" ]] && kill -0 "$safari_driver_pid" 2>/dev/null; then
    kill "$safari_driver_pid" 2>/dev/null || true
    wait "$safari_driver_pid" 2>/dev/null || true
  fi
  safari_driver_pid=""
}

management_backup="$artifact_dir/device-config.yaml"
management_runtime_backup=""
management_history_backup=""
management_secret_tmp=""
management_restore_needed=0

restore_management_config() {
  if [[ "$MODE" != "management" || "$management_restore_needed" != 1 || ! -s "$management_backup" ]]; then
    return 0
  fi
  echo "[E2E] Restoring the pre-test device configuration..."
  scp -q "$management_backup" "$board_ip:/var/tmp/llrdc-management-config.restore"
  scp -q "$management_runtime_backup" "$board_ip:/var/tmp/llrdc-management-runtime.restore"
  scp -q "$management_history_backup" "$board_ip:/var/tmp/llrdc-management-history.restore.tar"
  ssh -o BatchMode=yes "$board_ip" \
    "set -eu
     docker run --rm --entrypoint /bin/sh \
       -v /var/tmp:/stage:ro \
       -v /var/lib/llrdc-config:/config \
       -v /var/lib/llrdc-secrets:/secrets \
       -v /var/lib/llrdc-management:/management \
       llrdc-casting -c 'set -eu; cp -p /stage/llrdc-management-config.restore /config/config.yaml.restore; chmod 640 /config/config.yaml.restore; sync; mv -f /config/config.yaml.restore /config/config.yaml; cp -p /stage/llrdc-management-runtime.restore /secrets/runtime.env.restore; chmod 600 /secrets/runtime.env.restore; sync; mv -f /secrets/runtime.env.restore /secrets/runtime.env; find /management -mindepth 1 -maxdepth 1 -delete; tar -C /management -xf /stage/llrdc-management-history.restore.tar; chmod 700 /management; sync'
     docker stop -t 8 llrdc-casting >/dev/null
     rm -f /var/tmp/llrdc-management-config.restore /var/tmp/llrdc-management-runtime.restore /var/tmp/llrdc-management-history.restore.tar"
  for _ in {1..60}; do
    if curl -fsSk --connect-timeout 2 --max-time 3 "https://${admin_ip}:9090/health" >/dev/null 2>&1; then
      expected_hash="$(shasum -a 256 "$management_backup" | awk '{print $1}')"
      restored_hash="$(ssh -o BatchMode=yes "$board_ip" 'docker exec llrdc-casting sha256sum /config/config.yaml' | awk '{print $1}')"
      if [[ "$expected_hash" != "$restored_hash" ]]; then
        echo "[E2E] Restored device configuration checksum does not match the pre-test backup." >&2
        return 1
      fi
      expected_runtime_hash="$(shasum -a 256 "$management_runtime_backup" | awk '{print $1}')"
      restored_runtime_hash="$(ssh -o BatchMode=yes "$board_ip" 'docker exec llrdc-casting sha256sum /secrets/runtime.env' | awk '{print $1}')"
      if [[ "$expected_runtime_hash" != "$restored_runtime_hash" ]]; then
        echo "[E2E] Restored runtime secret checksum does not match the pre-test backup." >&2
        return 1
      fi
      management_restore_needed=0
      echo "[E2E] Pre-test device configuration restored."
      return 0
    fi
    sleep 1
  done
  echo "[E2E] Device configuration restoration did not become healthy." >&2
  return 1
}

on_exit() {
  local exit_status=$?
  stop_safari_driver
  if ! restore_management_config; then
    exit_status=1
  fi
  collect_receiver_logs
  if [[ -n "$management_secret_tmp" && -d "$management_secret_tmp" ]]; then
    rm -rf -- "$management_secret_tmp"
  fi
  exit "$exit_status"
}
trap on_exit EXIT

if [[ "$MODE" == "management" ]]; then
  echo "[E2E] Backing up the device configuration for the management suite..."
  ssh -o BatchMode=yes "$board_ip" 'docker exec llrdc-casting cat /config/config.yaml' > "$management_backup"
  [[ -s "$management_backup" ]] || { echo "[E2E] Could not back up /config/config.yaml." >&2; exit 1; }
  management_secret_tmp="$(mktemp -d "${TMPDIR:-/tmp}/llrdc-management.XXXXXX")"
  chmod 700 "$management_secret_tmp"
  management_runtime_backup="$management_secret_tmp/runtime.env"
  management_history_backup="$management_secret_tmp/management-history.tar"
  ssh -o BatchMode=yes "$board_ip" 'docker exec llrdc-casting cat /secrets/runtime.env' > "$management_runtime_backup"
  chmod 600 "$management_runtime_backup"
  [[ -s "$management_runtime_backup" ]] || { echo "[E2E] Could not back up the receiver runtime environment." >&2; exit 1; }
  ssh -o BatchMode=yes "$board_ip" 'docker run --rm --entrypoint /bin/sh -v /var/lib/llrdc-management:/management:ro llrdc-casting -c "tar -C /management -cf - ."' > "$management_history_backup"
  [[ -s "$management_history_backup" ]] || { echo "[E2E] Could not back up management history." >&2; exit 1; }
  management_restore_needed=1
  printf '%s\n' '[E2E] Backups captured; deploying a controlled cloud-disabled fixed-code receiver for the management suite.' > "$artifact_dir/deploy.log"
  set +e
  "$SCRIPT_DIR/server.sh" --start --cloud=false --pairing-code=AB12 --pairing-code-required=true --board-ip="$board_ip" 2>&1 | tee -a "$artifact_dir/deploy.log"
  management_deploy_status=${PIPESTATUS[0]}
  set -e
  if (( management_deploy_status != 0 )); then
    echo "[E2E] Controlled management deployment failed; see $artifact_dir/deploy.log" >&2
    exit 1
  fi
  resolve_admin_ip
  if ! ssh -o BatchMode=yes "$board_ip" "docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' llrdc-casting" 2>/dev/null | grep -Fxq 'PAIRING_CODE_FIXED=AB12'; then
    echo "[E2E] Management suite did not deploy the controlled fixed-code environment; see $artifact_dir/deploy.log" >&2
    exit 1
  fi
  export E2E_MODE=management
  export E2E_BOARD_IP="$board_ip"
  export E2E_ADMIN_IP="$admin_ip"
  export E2E_ARTIFACT_DIR="$artifact_dir"
  export E2E_MANAGEMENT_FIXED_CODE=AB12
  export E2E_MANAGEMENT_INITIAL_CONFIG="$management_backup"
  set +e
  (cd "$SCRIPT_DIR/client" && npm run test:e2e:admin)
  management_status=$?
  set -e
  exit "$management_status"
fi

cloud_flag=false
if [[ "$MODE" == "cloud" ]]; then cloud_flag=true; fi

reuse_deployment=0
if [[ "$MODE" == "codec" && "$codec_browser" == "safari" ]]; then
  reuse_deployment=1
  echo "[E2E] Reusing the existing cloud-disabled receiver for Safari; no redeploy."
else
  echo "[E2E] Deploying receiver for $MODE suite (cloud=$cloud_flag)."
  echo "[E2E] Build/deploy output follows live; a cold Docker build can take several minutes."
  set +e
  LLRDC_CODEC_DIAGNOSTICS=1 "$SCRIPT_DIR/server.sh" --start --cloud="$cloud_flag" --board-ip="$board_ip" 2>&1 | tee "$artifact_dir/deploy.log"
  deploy_status=${PIPESTATUS[0]}
  set -e
  if (( deploy_status != 0 )); then
    echo "[E2E] Receiver deployment failed; see $artifact_dir/deploy.log" >&2
    exit 1
  fi
fi

if (( reuse_deployment == 0 )); then
  echo "[E2E] Deployment complete. Verifying receiver configuration..."
else
  printf '%s\n' '[E2E] Reused the existing cloud-disabled receiver; no deployment was performed.' > "$artifact_dir/deploy.log"
  echo "[E2E] Verifying the existing receiver configuration..."
fi
resolve_admin_ip
expected_cloud_env="CLOUD_DISCOVERY_ENABLED=$([[ "$cloud_flag" == true ]] && echo 1 || echo 0)"
if ! ssh -o BatchMode=yes "$board_ip" "docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' llrdc-casting" 2>/dev/null \
  | grep -Fxq "$expected_cloud_env"; then
  echo "[E2E] Receiver cloud-discovery environment does not match the $MODE suite; see $artifact_dir/deploy.log" >&2
  exit 1
fi
expected_cloud_setting="$([[ "$cloud_flag" == true ]] && echo true || echo false)"
if ! ssh -o BatchMode=yes "$board_ip" "docker exec llrdc-casting awk '/^[[:space:]]*cloud_discovery_enabled:/ {print \$2; exit}' /config/config.yaml 2>/dev/null | tr -d '\"' | grep -Fxq '$expected_cloud_setting'" 2>/dev/null; then
  echo "[E2E] Receiver persisted cloud setting does not match the $MODE suite; see $artifact_dir/deploy.log" >&2
  exit 1
fi

if [[ "$MODE" == "codec" ]]; then
  echo "[E2E] Cloudflare disabled; retrieving the live local pairing code..."
  pairing_code="$("$SCRIPT_DIR/server.sh" --get-pairing-code --board-ip="$board_ip")"
else
  echo "[E2E] Cloudflare enabled; waiting for receiver registration..."
  cloud_receiver_id="$(ssh -o BatchMode=yes "$board_ip" "docker exec llrdc-casting awk '/^[[:space:]]*receiver_id:/ {gsub(/[\"[:space:]]/, \"\", \$2); print \$2; exit}' /config/config.yaml")"
  if [[ ! "$cloud_receiver_id" =~ ^[A-Za-z0-9_-]{1,128}$ ]]; then
    echo "[E2E] Receiver has no valid cloud device identity; see $artifact_dir/deploy.log" >&2
    exit 1
  fi
  registration_deadline=$((SECONDS + 90))
  last_registration_notice=0
  while (( SECONDS < registration_deadline )); do
    if ssh -o BatchMode=yes "$board_ip" 'docker logs llrdc-casting 2>&1' 2>/dev/null \
      | grep -Fq '[CLOUD DISCOVERY] Receiver registration succeeded'; then
      break
    fi
    if (( SECONDS - last_registration_notice >= 10 )); then
      echo "[E2E] Still waiting for cloud registration ($SECONDS seconds elapsed)..."
      last_registration_notice=$SECONDS
    fi
    sleep 2
  done
  if (( SECONDS >= registration_deadline )); then
    echo "[E2E] Receiver did not report successful cloud registration; see $artifact_dir/deploy.log" >&2
    exit 1
  fi

  echo "[E2E] Registration is ready; querying D1 for the unexpired pairing code..."
  : > "$artifact_dir/cloud-query.stderr"
  pairing_code=""
  for query_attempt in {1..5}; do
    pairing_candidate=""
    if pairing_candidate="$(
      cd "$SCRIPT_DIR/cloudflare/worker"
      ./node_modules/.bin/wrangler d1 execute cast-pairing --remote \
        --command "SELECT pairing_code FROM active_receivers WHERE receiver_id = '$cloud_receiver_id' AND pairing_code IS NOT NULL AND code_expires_at > unixepoch() AND registration_expires_at > unixepoch() LIMIT 1" \
        --json 2>>"$artifact_dir/cloud-query.stderr" \
        | python3 -c 'import json, sys
payload = json.load(sys.stdin)
if isinstance(payload, list):
    rows = payload[0].get("results", []) if payload else []
elif isinstance(payload, dict):
    rows = payload.get("results", [])
else:
    rows = []
print(rows[0].get("pairing_code", "") if rows else "")'
    )" && [[ "$pairing_candidate" =~ ^[A-Z0-9]{4}$ ]]; then
      pairing_code="$pairing_candidate"
      break
    fi
    echo "[E2E] D1 query attempt $query_attempt did not return a live code; retrying..."
    sleep 2
  done
fi

if [[ ! "$pairing_code" =~ ^[A-Z0-9]{4}$ ]]; then
  echo "[E2E] Could not obtain a valid live pairing code; see $artifact_dir." >&2
  exit 1
fi

echo "[E2E] Pairing code acquired privately. Starting the $codec_browser browser suite."
export E2E_MODE="$MODE"
export E2E_BOARD_IP="$board_ip"
export E2E_ADMIN_IP="$admin_ip"
export E2E_PAIRING_CODE="$pairing_code"
export E2E_NETWORK_TYPE="$network_type"
export E2E_RECEIVER_INTERFACE="$receiver_interface"

if [[ "$MODE" == "codec" ]]; then
  if [[ "$codec_browser" == "chrome" ]]; then
    chrome_artifact_dir="$artifact_dir/chrome"
    mkdir -p "$chrome_artifact_dir"
    export E2E_ARTIFACT_DIR="$chrome_artifact_dir"
    (cd "$SCRIPT_DIR/client" && npm run test:e2e:codec)
  else
    safari_artifact_dir="$artifact_dir/safari"
    mkdir -p "$safari_artifact_dir"
    safari_port="${SAFARI_WEBDRIVER_PORT:-4444}"
    safari_driver_log="$safari_artifact_dir/safaridriver.log"
    safaridriver --port "$safari_port" >"$safari_driver_log" 2>&1 &
    safari_driver_pid=$!
    safari_ready=0
    for _ in {1..30}; do
      if curl -fsS --max-time 1 "http://127.0.0.1:$safari_port/status" >/dev/null 2>&1; then
        safari_ready=1
        break
      fi
      sleep 1
    done
    if (( safari_ready != 1 )); then
      echo "[E2E] Safari WebDriver did not become ready; see $safari_driver_log" >&2
      safari_remote_automation_hint
      exit 1
    fi
    export E2E_ARTIFACT_DIR="$safari_artifact_dir"
    export SAFARI_WEBDRIVER_URL="http://127.0.0.1:$safari_port"
    if ! (cd "$SCRIPT_DIR/client" && npm run test:e2e:safari); then
      safari_remote_automation_hint
      exit 1
    fi
  fi
else
  export E2E_ARTIFACT_DIR="$artifact_dir"
  (cd "$SCRIPT_DIR/client" && npm run test:e2e:cloud)
fi
