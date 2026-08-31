#!/usr/bin/env bash
set -euo pipefail

IMAGE="danchitnis/llrdc-casting:latest"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
device_id=""
admin_bind=""
cloud_bundle=""
assume_yes=0

fail() { echo "ERROR [$1]: $2" >&2; echo "Recovery: ${3:-correct the problem and rerun this installer; completed phases are safe to repeat.}" >&2; exit 1; }
phase() { echo; echo "[DEVICE $1/7] $2"; }
prompt() { local answer; read -r -p "$1" answer </dev/tty; printf '%s' "$answer"; }

for argument in "$@"; do
  case "$argument" in
    --device-id=*) device_id="${argument#*=}" ;;
    --admin-bind=*) admin_bind="${argument#*=}" ;;
    --cloud-bundle=*) cloud_bundle="${argument#*=}" ;;
    --yes) assume_yes=1 ;;
    --local-only) cloud_bundle="" ;;
    *) fail arguments "Unknown option: $argument" ;;
  esac
done

[[ "${EUID}" == 0 ]] || fail privileges "Run with sudo."

phase 1 "Validating ROCK 4C+ and host services"
[[ "$(uname -m)" == "aarch64" ]] || fail hardware "Expected aarch64, found $(uname -m)."
. /etc/os-release
[[ "${ID:-}" == "debian" ]] || fail host "Expected Debian, found ${ID:-unknown}."
compatible="$(tr '\0' '\n' </proc/device-tree/compatible 2>/dev/null || true)"
grep -qx 'radxa,rock-4c-plus' <<<"$compatible" || fail hardware "This is not a Radxa ROCK 4C+."
grep -qx 'rockchip,rk3399' <<<"$compatible" || fail hardware "This device does not report Rockchip RK3399."
command -v systemctl >/dev/null || fail host "systemd is required."
command -v tailscale >/dev/null || fail tailscale "Tailscale must be installed and joined by the user."
tailscale_ip="$(tailscale ip -4 2>/dev/null | head -n1)"
[[ "$tailscale_ip" =~ ^100\. ]] || fail tailscale "No active Tailscale IPv4 was found." "Join Tailscale, verify 'tailscale ip -4', then rerun."
[[ -n "$admin_bind" ]] || admin_bind="$tailscale_ip"
[[ -n "$device_id" ]] || device_id="$(prompt "Device name [$(hostname -s)]: ")"
[[ -n "$device_id" ]] || device_id="$(hostname -s)"
[[ "$device_id" =~ ^[A-Za-z0-9_-]{1,128}$ ]] || fail identity "Device name must contain only letters, digits, underscore, or hyphen."

if ((assume_yes == 0)); then
  echo "Device: $device_id"
  echo "Management: https://${admin_bind}:9090/"
  echo "Cloud: $([[ -n "$cloud_bundle" ]] && echo enabled || echo disabled)"
  [[ "$(prompt "Install or reconcile LLrdc production services? [y/N] ")" =~ ^[Yy]$ ]] || fail cancelled "Installation cancelled."
fi

phase 2 "Installing host dependencies"
if ! command -v docker >/dev/null || ! command -v dockerd >/dev/null || ! command -v jq >/dev/null || ! command -v curl >/dev/null || ! command -v python3 >/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update || fail packages "apt update failed."
  # Debian 13 splits the daemon and client into docker.io and docker-cli.
  apt-get install -y docker.io docker-cli jq curl ca-certificates python3 || fail packages "Docker, helper-tool, and updater dependencies could not be installed."
  hash -r
fi
command -v dockerd >/dev/null || fail docker "Docker daemon executable was not installed."
command -v docker >/dev/null || fail docker "Docker CLI was not installed."
command -v python3 >/dev/null || fail tools "Python 3 was not installed."
systemctl enable --now docker || fail docker "Docker could not be started."
docker info >/dev/null || fail docker "Docker daemon is unavailable."

phase 3 "Writing durable production configuration"
install -d -m 0755 /etc/llrdc /var/lib/llrdc-config /var/lib/llrdc-certs /var/lib/llrdc-pairing
install -d -m 0700 /var/lib/llrdc-secrets /var/lib/llrdc-management /var/lib/llrdc-update
install -d -m 0770 /var/lib/llrdc-update/requests
install -d -m 0755 /var/lib/llrdc-update/status
install -d -o root -g docker -m 0770 /var/tmp/llrdc-bin
cloud_enabled=false
registration_secret=""
worker_url="https://cast.llrdc.com"
if [[ -n "$cloud_bundle" ]]; then
  [[ -r "$cloud_bundle" ]] || fail cloud "Cloud bundle is unreadable."
  # shellcheck disable=SC1090
  source "$cloud_bundle"
  registration_secret="${RECEIVER_REGISTRATION_SECRET:-}"
  worker_url="${PAIRING_WORKER_URL:-$worker_url}"
  [[ -n "$registration_secret" && -r "${PAIRING_PUBLIC_KEY_FILE:-}" ]] || fail cloud "Cloud bundle is incomplete."
  install -m 0644 "${PAIRING_PUBLIC_KEY_FILE}" /var/lib/llrdc-pairing/public.pem
  cloud_enabled=true
fi
if [[ ! -s /var/lib/llrdc-config/config.yaml ]]; then
  cat >/var/lib/llrdc-config/config.yaml <<EOF
version: 1
server:
  port: 4434
  webtransport_port: 4433
  http_port: 8080
  admin_bind_address: "${admin_bind}"
  admin_port: 9090
  drm_connector_id: "auto"
  drm_plane_id: "33"
  idle_dashboard: true
  idle_dashboard_mode: "raw"
  idle_timeout_sec: 30
  sender_liveness_timeout_sec: 90
  udp_buffer_size_mb: 8
  cert_dir: "/certs"
  pairing_worker_url: "${worker_url}"
  cloud_discovery_enabled: ${cloud_enabled}
  receiver_id: "${device_id}"
  pairing_code_ttl_sec: 3600
  local_pairing_code_required: true
  pairing_token_public_key_file: "/pairing/public.pem"
EOF
else
  # Preserve portal-edited operational settings during an idempotent rerun;
  # only reconcile identity and cloud provisioning fields owned by init.
  sed -i -E \
    -e "s|^  pairing_worker_url:.*|  pairing_worker_url: \"${worker_url}\"|" \
    -e "s|^  cloud_discovery_enabled:.*|  cloud_discovery_enabled: ${cloud_enabled}|" \
    -e "s|^  receiver_id:.*|  receiver_id: \"${device_id}\"|" \
    -e 's|^  pairing_token_public_key_file:.*|  pairing_token_public_key_file: "/pairing/public.pem"|' \
    /var/lib/llrdc-config/config.yaml
fi
chmod 0640 /var/lib/llrdc-config/config.yaml
cat >/var/lib/llrdc-secrets/runtime.env <<EOF
CLOUD_DISCOVERY_ENABLED=$([[ "$cloud_enabled" == true ]] && echo 1 || echo 0)
PAIRING_WORKER_URL=${worker_url}
RECEIVER_ID=${device_id}
RECEIVER_REGISTRATION_SECRET=${registration_secret}
PAIRING_TOKEN_PUBLIC_KEY_FILE=/pairing/public.pem
LLRDC_UPDATE_REQUEST_DIR=/updates/requests
LLRDC_UPDATE_STATUS_FILE=/updates/status/status.json
EOF
chmod 0600 /var/lib/llrdc-secrets/runtime.env
printf 'independent\n' >/etc/llrdc/role
printf 'LLRDC_IMAGE=%s\n' "$IMAGE" >/etc/llrdc/image.env

phase 4 "Installing helper tools"
helper_manifest="$SCRIPT_DIR/helper-tools.manifest"
[[ -r "$helper_manifest" ]] || fail tools "Helper-tool manifest is missing from the installer bundle."
install -d -m 0755 /usr/local/lib/llrdc-tools
while IFS= read -r helper; do
  [[ -n "$helper" && "$helper" != \#* ]] || continue
  helper_source="$SCRIPT_DIR/$helper"
  helper_name="$(basename "$helper")"
  [[ -f "$helper_source" ]] || fail tools "Helper tool is missing from the installer bundle: $helper"
  install -m 0755 "$helper_source" "/usr/local/lib/llrdc-tools/$helper_name"
  ln -sfn "/usr/local/lib/llrdc-tools/$helper_name" "/usr/local/bin/$helper_name"
done <"$helper_manifest"

phase 5 "Installing container and updater services"
install -m 0755 "$SCRIPT_DIR/llrdc-update.sh" /usr/local/sbin/llrdc-update
cat >/usr/local/sbin/llrdc-container-start <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
source /etc/llrdc/image.env
image_ref="$(cat /etc/llrdc/active-image 2>/dev/null || echo "$LLRDC_IMAGE")"
development_mounts=()
if [[ -f /var/tmp/llrdc-bin/development.enabled ]]; then
  [[ -x /var/tmp/llrdc-bin/llrdc-casting && -x /var/tmp/llrdc-bin/llrdc-management ]] || {
    echo "Development override is enabled but its binaries are unavailable." >&2
    exit 1
  }
  development_image="$(cat /var/tmp/llrdc-bin/development.image 2>/dev/null || echo llrdc-casting)"
  docker image inspect "$development_image" >/dev/null
  image_ref="$development_image"
  development_mounts=(
    -v /var/tmp/llrdc-bin/llrdc-casting:/usr/local/bin/llrdc-casting:ro
    -v /var/tmp/llrdc-bin/llrdc-management:/usr/local/bin/llrdc-management:ro
  )
fi
docker rm -f llrdc-casting >/dev/null 2>&1 || true
exec docker run --name llrdc-casting --restart unless-stopped --net host --privileged \
  --env-file /var/lib/llrdc-secrets/runtime.env -v /dev:/dev \
  -v /var/lib/llrdc-certs:/certs -v /var/lib/llrdc-pairing:/pairing:ro \
  -v /var/lib/llrdc-secrets:/secrets:ro -v /var/lib/llrdc-config:/config:rw \
  -v /var/lib/llrdc-management:/management:rw \
  -v /var/lib/llrdc-update/requests:/updates/requests:rw \
  -v /var/lib/llrdc-update/status:/updates/status:ro \
  "${development_mounts[@]}" "$image_ref"
EOF
chmod 0755 /usr/local/sbin/llrdc-container-start
cat >/etc/systemd/system/llrdc-casting.service <<'EOF'
[Unit]
Description=LLrdc production casting receiver
After=docker.service tailscaled.service network-online.target
Requires=docker.service
[Service]
Type=simple
ExecStart=/usr/local/sbin/llrdc-container-start
ExecStop=-/usr/bin/docker stop -t 8 llrdc-casting
Restart=always
RestartSec=3
[Install]
WantedBy=multi-user.target
EOF
cat >/etc/systemd/system/llrdc-update.service <<'EOF'
[Unit]
Description=LLrdc constrained Docker image updater
After=docker.service
[Service]
Type=oneshot
ExecStart=/usr/local/sbin/llrdc-update
EOF
cat >/etc/systemd/system/llrdc-update.path <<'EOF'
[Unit]
Description=Watch for LLrdc portal update requests
[Path]
PathChanged=/var/lib/llrdc-update/requests
Unit=llrdc-update.service
[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload

phase 6 "Pulling and starting the production image"
docker pull "$IMAGE" >/dev/null || fail image "Could not pull $IMAGE."
architecture="$(docker image inspect --format '{{.Architecture}}' "$IMAGE")"
[[ "$architecture" == "arm64" ]] || fail image "Docker Hub image architecture is $architecture, expected arm64."
docker image inspect --format '{{.Id}}' "$IMAGE" >/etc/llrdc/active-image
current="$(cat /etc/llrdc/active-image)"
jq -n --arg current "$current" --argjson now "$(date +%s)" '{state:"idle",current_digest:$current,available_digest:null,current_version:null,message:"Use the management portal to check for updates.",updated_at_unix:$now,managed:true}' >/var/lib/llrdc-update/status/status.json
systemctl enable --now llrdc-update.path llrdc-casting.service || fail service "Production services could not be started."

phase 7 "Verifying receiver health"
healthy=0
for attempt in $(seq 1 60); do
  if curl -fk --silent --connect-timeout 2 --max-time 3 "https://${admin_bind}:9090/health" >/dev/null 2>&1; then
    healthy=$((healthy + 1))
    ((healthy >= 3)) && break
  else
    healthy=0
    ((attempt % 10 == 0)) && echo "Still waiting for stable receiver health (${attempt}/60)..."
  fi
  sleep 1
done
((healthy >= 3)) || fail health "Receiver was not stably healthy within 60 seconds." "Run 'systemctl status llrdc-casting' and 'docker logs llrdc-casting', fix the reported issue, then rerun."
if [[ "$cloud_enabled" == true ]]; then
  registered=0
  for attempt in $(seq 1 60); do
    cloud_snapshot="$(curl -fk --silent --connect-timeout 2 --max-time 3 "https://${admin_bind}:9090/api/snapshot" 2>/dev/null || true)"
    if jq -e '.pairing.cloud_status == "READY" and .settings.cloud_state == "READY"' <<<"$cloud_snapshot" >/dev/null 2>&1; then
      registered=1
      break
    fi
    ((attempt % 10 == 0)) && echo "Still waiting for Cloudflare registration (${attempt}/60)..."
    sleep 1
  done
  ((registered == 1)) || fail cloud "Receiver health passed, but Cloudflare registration was not observed." "Check the scoped device credential and Worker status, then rerun initialization."
fi
echo
echo "LLrdc production device is ready."
echo "Management portal: https://${admin_bind}:9090/"
echo "Local casting: https://<device-lan-ip>:8080/"
echo "Image: ${current}"
echo "Helper tools: /usr/local/lib/llrdc-tools (also linked in /usr/local/bin)."
echo "Fan curve: unchanged. To install it manually, run 'sudo setup_pwm_fan.sh setup', then reboot the board."
