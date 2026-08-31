#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
board="100.100.1.72"

usage() {
  cat <<'EOF'
Usage: ./test_release.sh [--board-ip=<development-board-address>]

Runs every production release gate, including uncommitted developer changes.
This command does not publish an image and never runs sudo.
EOF
}

for argument in "$@"; do
  case "$argument" in
    --board-ip=*) board="${argument#*=}" ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $argument" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -n "$board" && "$board" != -* && "$board" != *[[:space:]]* ]] || {
  echo "Invalid development board address." >&2
  exit 2
}
echo "[1/5] Client checks"
npm --prefix client run check
echo "[2/5] ARM64 Rust tests"
./server.sh --test
echo "[3/5] Hardware codec gate"
./test_browser.sh codec chrome --board-ip="$board"
echo "[4/5] Management gate"
./test_browser.sh management --board-ip="$board"
echo "[5/5] Cloudflare gate"
./test_browser.sh cloud --board-ip="$board"

echo "Release tests passed."
echo "You may now run ./publish_docker_image.sh"
