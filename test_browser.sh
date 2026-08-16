#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MODE="${1:-}"
shift || true

usage() {
  cat <<'EOF'
Usage: ./test_browser.sh <codec|cloud|all> [chrome|safari] [--board-ip=<address>]

codec  Run the local codec suite in branded Chrome (default) or installed Safari.
cloud  Deploy with Cloudflare enabled and run pairing plus one HEVC stream.
all    Run the Chrome codec suite first, then cloud.

Safari is run separately: ./test_browser.sh codec safari
EOF
}

if [[ "$MODE" != "codec" && "$MODE" != "cloud" && "$MODE" != "all" ]]; then
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

on_exit() {
  stop_safari_driver
  collect_receiver_logs
}
trap on_exit EXIT

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
  "$SCRIPT_DIR/server.sh" --start --cloud="$cloud_flag" --board-ip="$board_ip" 2>&1 | tee "$artifact_dir/deploy.log"
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
expected_cloud_env="CLOUD_DISCOVERY_ENABLED=$([[ "$cloud_flag" == true ]] && echo 1 || echo 0)"
if ! ssh -o BatchMode=yes "$board_ip" "docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' llrdc-casting" 2>/dev/null \
  | grep -Fxq "$expected_cloud_env"; then
  echo "[E2E] Receiver cloud-discovery environment does not match the $MODE suite; see $artifact_dir/deploy.log" >&2
  exit 1
fi

if [[ "$MODE" == "codec" ]]; then
  echo "[E2E] Cloudflare disabled; retrieving the live local pairing code..."
  pairing_code="$("$SCRIPT_DIR/server.sh" --get-pairing-code --board-ip="$board_ip")"
else
  echo "[E2E] Cloudflare enabled; waiting for receiver registration..."
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
  pairing_code="$(
    cd "$SCRIPT_DIR/cloudflare/worker"
    ./node_modules/.bin/wrangler d1 execute cast-pairing --remote \
      --command "SELECT pairing_code FROM active_receivers WHERE pairing_code IS NOT NULL AND code_expires_at > unixepoch() AND registration_expires_at > unixepoch() LIMIT 1" \
      --json 2>"$artifact_dir/cloud-query.stderr" \
      | python3 -c 'import json, sys
payload = json.load(sys.stdin)
rows = payload[0].get("results", []) if payload else []
print(rows[0].get("pairing_code", "") if rows else "")'
  )"
fi

if [[ ! "$pairing_code" =~ ^[A-Z0-9]{4}$ ]]; then
  echo "[E2E] Could not obtain a valid live pairing code; see $artifact_dir." >&2
  exit 1
fi

echo "[E2E] Pairing code acquired privately. Starting the $codec_browser browser suite."
export E2E_MODE="$MODE"
export E2E_BOARD_IP="$board_ip"
export E2E_PAIRING_CODE="$pairing_code"

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
