#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v node >/dev/null 2>&1; then
  echo "ERROR: Node.js is required to run Cloudflare setup." >&2
  echo "Install Node.js 18 or newer, then run ./setup_cloudflare.sh again." >&2
  exit 127
fi

node_version="$(node -p 'process.versions.node')"
node_major="${node_version%%.*}"
if [[ ! "$node_major" =~ ^[0-9]+$ ]] || (( node_major < 18 )); then
  echo "ERROR: Node.js 18 or newer is required (found ${node_version})." >&2
  exit 2
fi

exec node "${SCRIPT_DIR}/tools/setup_cloudflare.mjs" "$@"
