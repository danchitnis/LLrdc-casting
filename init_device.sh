#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STATE_DIR="$SCRIPT_DIR/.cloudflare"
add_cloud=0
if [[ "${1:-}" == "--add-cloud" ]]; then add_cloud=1; shift; fi
board="${1:-}"
[[ -n "$board" ]] || read -r -p "Device SSH/Tailscale address: " board
[[ -n "$board" && "$board" != -* && "$board" != *[[:space:]]* ]] || { echo "Invalid device address." >&2; exit 2; }

echo "[MAC 1/5] Inspecting remote ROCK 4C+"
remote_info="$(ssh -o BatchMode=yes "$board" 'set -eu; . /etc/os-release; test "$ID" = debian; uname -m; tr "\000" "\n" </proc/device-tree/compatible; command -v systemctl >/dev/null; command -v tailscale >/dev/null; getent ahosts registry-1.docker.io >/dev/null; tailscale ip -4 | head -n1')" || { echo "Remote validation failed. Verify Debian, SSH, Internet access, and Tailscale, then rerun." >&2; exit 1; }
grep -qx aarch64 <<<"$remote_info" && grep -qx radxa,rock-4c-plus <<<"$remote_info" && grep -qx rockchip,rk3399 <<<"$remote_info" || { echo "Target is not the supported ARM64 ROCK 4C+." >&2; exit 1; }
tailscale_ip="$(tail -n1 <<<"$remote_info")"

echo "Validating sudo access (the device may prompt for its password)."
if ((add_cloud)); then
  ssh -t "$board" 'sudo -v && role="$(sudo cat /etc/llrdc/role)" && { test "$role" = independent || test "$role" = production; }' || { echo "Add-cloud mode requires an initialized device." >&2; exit 1; }
  default_id="$(ssh -o BatchMode=yes "$board" 'hostname -s')"
else
  ssh -t "$board" 'sudo -v'
  default_id="$(ssh -o BatchMode=yes "$board" 'hostname -s')"
fi
read -r -p "Device name [$default_id]: " device_id
device_id="${device_id:-$default_id}"
[[ "$device_id" =~ ^[A-Za-z0-9_-]{1,128}$ ]] || { echo "Device name must contain only letters, digits, underscore, or hyphen." >&2; exit 2; }
cloud_enabled=0
if ((add_cloud)); then
  cloud_enabled=1
else
  read -r -p "Enable cast.llrdc.com cloud discovery? [y/N]: " cloud_answer
  [[ "$cloud_answer" =~ ^[Yy]$ ]] && cloud_enabled=1
fi
echo "Device: $device_id"
echo "Management portal: https://${tailscale_ip}:9090/"
echo "Cloud discovery: $([[ $cloud_enabled == 1 ]] && echo enabled || echo disabled)"
read -r -p "Continue with independent device initialization? [y/N]: " confirmation
[[ "$confirmation" =~ ^[Yy]$ ]] || { echo "Initialization cancelled." >&2; exit 1; }

temporary="$(mktemp -d "${TMPDIR:-/tmp}/llrdc-init.XXXXXX")"
cleanup() {
  rm -rf "$temporary"
  ssh -o BatchMode=yes "$board" 'rm -rf /tmp/llrdc-init' >/dev/null 2>&1 || true
}
trap cleanup EXIT
cp "$SCRIPT_DIR/device/install_production.sh" "$SCRIPT_DIR/device/llrdc-update.sh" "$temporary/"
cp "$SCRIPT_DIR/device/helper-tools.manifest" "$temporary/"
mkdir -p "$temporary/tools"
while IFS= read -r helper; do
  [[ -n "$helper" && "$helper" != \#* ]] || continue
  cp "$SCRIPT_DIR/$helper" "$temporary/$helper"
done <"$SCRIPT_DIR/device/helper-tools.manifest"
remote_bundle=""
if ((cloud_enabled)); then
  echo "[MAC 2/5] Preparing device-scoped Cloudflare credentials"
  root_secret="$STATE_DIR/registration-root.secret"
  public_key="$STATE_DIR/receiver-public.pem"
  [[ -s "$root_secret" && -s "$public_key" ]] || { echo "Cloudflare fleet credentials are missing. Run ./setup_cloudflare.sh first, or choose local-only setup." >&2; exit 1; }
  command -v node >/dev/null || { echo "Node.js is required to derive the scoped device credential." >&2; exit 1; }
  registration_secret="$(node -e 'const fs=require("fs"),c=require("crypto"); const root=fs.readFileSync(process.argv[1],"utf8").trim(); const key=Buffer.from(root.replace(/-/g,"+").replace(/_/g,"/"),"base64"); process.stdout.write(c.createHmac("sha256",key).update("cast-registration-v1:"+process.argv[2]).digest("base64url"));' "$root_secret" "$device_id")"
  cp "$public_key" "$temporary/public.pem"
  cat >"$temporary/cloud.env" <<EOF
RECEIVER_REGISTRATION_SECRET=${registration_secret}
PAIRING_WORKER_URL=https://cast.llrdc.com
PAIRING_PUBLIC_KEY_FILE=/tmp/llrdc-init/public.pem
EOF
  chmod 0600 "$temporary/cloud.env"
  remote_bundle="--cloud-bundle=/tmp/llrdc-init/cloud.env"
else
  echo "[MAC 2/5] Configuring local-only operation"
fi

echo "[MAC 3/5] Uploading the production installer"
ssh -o BatchMode=yes "$board" 'rm -rf /tmp/llrdc-init && mkdir -m 700 /tmp/llrdc-init'
scp -qr "$temporary/install_production.sh" "$temporary/llrdc-update.sh" "$temporary/helper-tools.manifest" "$temporary/tools" "$board:/tmp/llrdc-init/"
if ((cloud_enabled)); then scp -q "$temporary/cloud.env" "$temporary/public.pem" "$board:/tmp/llrdc-init/"; fi

echo "[MAC 4/5] Installing production services (sudo may prompt on the device)"
ssh -t "$board" "cd /tmp/llrdc-init && sudo ./install_production.sh --device-id='$device_id' --admin-bind='$tailscale_ip' $remote_bundle --yes"

echo "[MAC 5/5] Verifying installation (read-only, no sudo)"
ssh -o BatchMode=yes "$board" "test \"\$(cat /etc/llrdc/role)\" = independent && test -x /usr/local/bin/fan_control.py && test -x /usr/local/bin/setup_pwm_fan.sh && systemctl is-enabled llrdc-casting llrdc-update.path >/dev/null && systemctl is-active llrdc-casting llrdc-update.path >/dev/null && curl --fail --silent --show-error --insecure 'https://$tailscale_ip:9090/health/manager' >/dev/null"
mkdir -p "$STATE_DIR/devices"
node -e 'const fs=require("fs"); const [path,id,address,cloud]=process.argv.slice(1); fs.writeFileSync(path, JSON.stringify({version:1,receiverId:id,address,cloudEnabled:cloud==="1",initializedAt:new Date().toISOString()},null,2)+"\n", {mode:0o600});' "$STATE_DIR/devices/$device_id.json" "$device_id" "$board" "$cloud_enabled"
cleanup
trap - EXIT
echo "Device ready: https://${tailscale_ip}:9090/"
