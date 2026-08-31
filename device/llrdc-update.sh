#!/usr/bin/env bash
set -euo pipefail

IMAGE="danchitnis/llrdc-casting:latest"
REQUEST_DIR="/var/lib/llrdc-update/requests"
STATUS_FILE="/var/lib/llrdc-update/status/status.json"
ACTIVE_IMAGE_FILE="/etc/llrdc/active-image"
CONFIG_FILE="/var/lib/llrdc-config/config.yaml"
DEVELOPMENT_MARKER="/var/tmp/llrdc-bin/development.enabled"

write_status() {
  local state="$1" current="${2:-}" available="${3:-}" message="${4:-}"
  local temporary="${STATUS_FILE}.new"
  jq -n --arg state "$state" --arg current "$current" --arg available "$available" \
    --arg message "$message" --arg version "${LLRDC_BUILD_REVISION:-}" --argjson updated "$(date +%s)" \
    '{state:$state,current_digest:(if $current=="" then null else $current end),available_digest:(if $available=="" then null else $available end),current_version:(if $version=="" then null else $version end),message:(if $message=="" then null else $message end),updated_at_unix:$updated,managed:true}' >"$temporary"
  chmod 0644 "$temporary"
  mv -f "$temporary" "$STATUS_FILE"
}

image_id() { docker image inspect --format '{{.Id}}' "$1"; }
current_image() {
  if docker container inspect llrdc-casting >/dev/null 2>&1; then
    docker container inspect --format '{{.Image}}' llrdc-casting
  elif [[ -s "$ACTIVE_IMAGE_FILE" ]]; then
    cat "$ACTIVE_IMAGE_FILE"
  fi
}

pull_candidate() {
  docker pull "$IMAGE" >/dev/null
  local architecture
  architecture="$(docker image inspect --format '{{.Architecture}}' "$IMAGE")"
  [[ "$architecture" == "arm64" ]] || { echo "unsupported image architecture: $architecture" >&2; return 1; }
  image_id "$IMAGE"
}

wait_healthy() {
  local admin_bind admin_port streak=0
  admin_bind="$(awk '/admin_bind_address:/ {gsub(/["\047]/, "", $2); print $2; exit}' "$CONFIG_FILE")"
  admin_port="$(awk '/admin_port:/ {print $2; exit}' "$CONFIG_FILE")"
  for _ in $(seq 1 60); do
    if curl -fsSk --connect-timeout 2 --max-time 3 "https://${admin_bind}:${admin_port}/health" >/dev/null; then
      streak=$((streak + 1))
      ((streak >= 3)) && return 0
    else
      streak=0
    fi
    sleep 1
  done
  return 1
}

casting_active() {
  local admin_bind admin_port
  admin_bind="$(awk '/admin_bind_address:/ {gsub(/["\047]/, "", $2); print $2; exit}' "$CONFIG_FILE")"
  admin_port="$(awk '/admin_port:/ {print $2; exit}' "$CONFIG_FILE")"
  curl -fsSk --connect-timeout 2 --max-time 3 "https://${admin_bind}:${admin_port}/api/snapshot" \
    | jq -e '.management.active_stream != null' >/dev/null 2>&1
}

check_update() {
  local current candidate
  current="$(current_image || true)"
  write_status checking "$current" "" "Checking Docker Hub for an ARM64 image."
  if ! candidate="$(pull_candidate)"; then
    write_status failed "$current" "" "Image pull or architecture verification failed."
    return 1
  fi
  if [[ -f "$DEVELOPMENT_MARKER" ]]; then
    write_status available "$current" "$candidate" "The published release is ready to replace the temporary Mac development build."
  elif [[ "$candidate" == "$current" ]]; then
    write_status current "$current" "$candidate" "The device is already current."
  else
    write_status available "$current" "$candidate" "An update is ready to install."
  fi
}

apply_update() {
  local current candidate previous_active was_development=0
  current="$(current_image || true)"
  previous_active="$(cat "$ACTIVE_IMAGE_FILE" 2>/dev/null || true)"
  [[ -f "$DEVELOPMENT_MARKER" ]] && was_development=1
  if casting_active; then
    candidate="$(jq -r '.available_digest // empty' "$STATUS_FILE" 2>/dev/null || true)"
    write_status available "$current" "$candidate" "Update deferred because a cast became active."
    return 1
  fi
  candidate="$(pull_candidate)" || { write_status failed "$current" "" "Image pull or architecture verification failed."; return 1; }
  if [[ "$was_development" == 0 && "$candidate" == "$current" ]]; then
    write_status current "$current" "$candidate" "The device is already current."
    return 0
  fi
  if casting_active; then
    write_status available "$current" "$candidate" "Update deferred because a cast became active."
    return 1
  fi
  write_status updating "$current" "$candidate" "Installing the update and checking receiver health."
  rm -f "$DEVELOPMENT_MARKER"
  printf '%s\n' "$candidate" >"${ACTIVE_IMAGE_FILE}.new"
  chmod 0644 "${ACTIVE_IMAGE_FILE}.new"
  mv -f "${ACTIVE_IMAGE_FILE}.new" "$ACTIVE_IMAGE_FILE"
  if systemctl restart llrdc-casting.service && wait_healthy; then
    write_status succeeded "$candidate" "$candidate" "Update installed successfully."
    return 0
  fi
  printf '%s\n' "${previous_active:-$current}" >"${ACTIVE_IMAGE_FILE}.new"
  mv -f "${ACTIVE_IMAGE_FILE}.new" "$ACTIVE_IMAGE_FILE"
  if [[ "$was_development" == 1 ]]; then
    printf 'development\n' >"${DEVELOPMENT_MARKER}.new"
    mv -f "${DEVELOPMENT_MARKER}.new" "$DEVELOPMENT_MARKER"
  fi
  if systemctl restart llrdc-casting.service && wait_healthy; then
    write_status rolled_back "$current" "$candidate" "The update failed health checks; the previous image was restored."
  else
    write_status failed "$current" "$candidate" "The update and automatic rollback both failed; inspect systemd and Docker logs."
  fi
  return 1
}

mkdir -p "$REQUEST_DIR" "$(dirname "$STATUS_FILE")"
shopt -s nullglob
for request in "$REQUEST_DIR"/*.request; do
  name="$(basename "$request")"
  rm -f "$request"
  case "$name" in
    check-*.request) check_update || true ;;
    apply-*.request) apply_update || true ;;
  esac
done
