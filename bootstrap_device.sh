#!/usr/bin/env bash
set -euo pipefail

BASE_URL="https://raw.githubusercontent.com/danchitnis/LLrdc-casting/main"
temporary="$(mktemp -d /tmp/llrdc-bootstrap.XXXXXX)"
trap 'rm -rf "$temporary"' EXIT
echo "Downloading the public LLrdc local-only installer from $BASE_URL"
curl -fsSL "$BASE_URL/device/install_production.sh" -o "$temporary/install_production.sh"
curl -fsSL "$BASE_URL/device/llrdc-update.sh" -o "$temporary/llrdc-update.sh"
curl -fsSL "$BASE_URL/device/helper-tools.manifest" -o "$temporary/helper-tools.manifest"
mkdir -p "$temporary/tools"
while IFS= read -r helper; do
  [[ -n "$helper" && "$helper" != \#* ]] || continue
  curl -fsSL "$BASE_URL/$helper" -o "$temporary/$helper"
done <"$temporary/helper-tools.manifest"
chmod 0755 "$temporary/install_production.sh" "$temporary/llrdc-update.sh"
sudo "$temporary/install_production.sh" --local-only
