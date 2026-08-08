#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKER_DIR="${SCRIPT_DIR}/cloudflare/worker"
STATE_DIR="${SCRIPT_DIR}/.cloudflare"
WORKER_CONFIG="${WORKER_DIR}/wrangler.toml"
PUBLIC_KEY_LOCAL="${STATE_DIR}/receiver-public.pem"
PRIVATE_KEY_LOCAL="${STATE_DIR}/worker-private.pem"
ROOT_SECRET_LOCAL="${STATE_DIR}/registration-root.secret"
RECEIVER_ENV="${STATE_DIR}/receiver.env"
REUSE_WORKER_CONFIG=0

if [[ ! -t 0 || ! -t 1 ]]; then
  echo "This setup must be run from an interactive terminal." >&2
  exit 1
fi

die() {
  echo "ERROR: $*" >&2
  exit 1
}

confirm() {
  local answer
  read -r -p "$1 [y/N] " answer
  [[ "$answer" =~ ^[Yy]([Ee][Ss])?$ ]]
}

prompt_default() {
  local prompt="$1"
  local default="$2"
  local value
  read -r -p "${prompt} [${default}]: " value
  printf '%s' "${value:-$default}"
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

url_safe_base64_from_hex() {
  printf '%s' "$1" | xxd -r -p | base64 | tr -d '=\n' | tr '+/' '-_'
}

generate_registration_credentials() {
  local message
  REGISTRATION_ROOT_SECRET="$(url_safe_base64_from_hex "$root_hex")"
  message="cast-registration-v1:${RECEIVER_ID}"
  RECEIVER_REGISTRATION_SECRET="$(node - "$REGISTRATION_ROOT_SECRET" "$message" <<'NODE'
const crypto = require('node:crypto');
const [rootSecret, message] = process.argv.slice(2);
const root = Buffer.from(rootSecret.replace(/-/g, '+').replace(/_/g, '/'), 'base64');
process.stdout.write(crypto.createHmac('sha256', root).update(message, 'utf8').digest('base64url'));
NODE
)"
}

echo "====================================================="
echo " LLrdc Cloudflare pairing setup"
echo "====================================================="
echo
echo "This will configure D1, Worker secrets, the custom domain, and one receiver."
echo "It will not put media or WebTransport traffic through Cloudflare."
echo "SSH must already be configured and working for the receiver."
echo "This script does not install SSH, create users, or change SSH settings."
echo

for command_name in node npm npx openssl xxd base64 sed ssh scp curl; do
  require_command "$command_name"
done

[[ -d "$WORKER_DIR" ]] || die "Worker directory not found: $WORKER_DIR"
[[ -f "${WORKER_DIR}/wrangler.toml.example" ]] || die "Missing wrangler.toml.example"

echo "Checking Wrangler authentication..."
(cd "$WORKER_DIR" && npx wrangler whoami) || die "Wrangler is not logged in. Run: npx wrangler login"

if [[ ! -d "${SCRIPT_DIR}/client/node_modules" ]]; then
  echo "Installing client dependencies..."
  (cd "${SCRIPT_DIR}/client" && npm ci)
fi
if [[ ! -d "${WORKER_DIR}/node_modules" ]]; then
  echo "Installing Worker dependencies..."
  (cd "$WORKER_DIR" && npm ci)
fi

DOMAIN="$(prompt_default "Cloudflare hostname" "cast.llrdc.com")"
WORKER_NAME="$(prompt_default "Worker name" "cast-pairing-worker")"
DB_NAME="$(prompt_default "D1 database name" "cast-pairing")"
[[ "$DOMAIN" =~ ^[A-Za-z0-9.-]+$ ]] || die "Invalid hostname"
[[ "$WORKER_NAME" =~ ^[A-Za-z0-9_-]{1,63}$ ]] || die "Worker name must contain only letters, numbers, '_' or '-'"
[[ "$DB_NAME" =~ ^[A-Za-z0-9_-]{1,63}$ ]] || die "D1 database name must contain only letters, numbers, '_' or '-'"

if [[ -f "$WORKER_CONFIG" ]]; then
  echo "Existing Worker configuration found: $WORKER_CONFIG"
  if ! confirm "Reuse this configuration and database ID"; then
    rm -f "$WORKER_CONFIG"
  else
    REUSE_WORKER_CONFIG=1
  fi
fi

if [[ ! -f "$WORKER_CONFIG" ]]; then
  cp "${WORKER_DIR}/wrangler.toml.example" "$WORKER_CONFIG"
  sed -i.bak \
    -e "s/^name = .*/name = \"${WORKER_NAME}\"/" \
    -e "s/^database_name = .*/database_name = \"${DB_NAME}\"/" \
    "$WORKER_CONFIG"
  rm -f "${WORKER_CONFIG}.bak"

  echo
  echo "Create a new remote D1 database named '${DB_NAME}'?"
  if confirm "Create D1 database"; then
    DB_OUTPUT="$(mktemp)"
    trap 'rm -f "$DB_OUTPUT"' EXIT
    (cd "$WORKER_DIR" && npx wrangler d1 create "$DB_NAME" --binding DB --config "$WORKER_CONFIG") | tee "$DB_OUTPUT"
    DATABASE_ID="$(sed $'s/\033\\[[0-9;]*m//g' "$DB_OUTPUT" | sed -nE 's/.*database_id[[:space:]]*[=:][[:space:]]*"?([A-Za-z0-9-]+)"?.*/\1/p' | sed -n '1p')"
    [[ -n "$DATABASE_ID" ]] || die "Could not read the new D1 database ID from Wrangler output"
  else
    read -r -p "Existing D1 database ID: " DATABASE_ID
    [[ "$DATABASE_ID" =~ ^[A-Za-z0-9-]+$ ]] || die "Invalid D1 database ID"
  fi
  sed -i.bak "s/REPLACE_WITH_D1_DATABASE_ID/${DATABASE_ID}/" "$WORKER_CONFIG"
  rm -f "${WORKER_CONFIG}.bak"
else
  DB_NAME="$(sed -nE 's/^database_name[[:space:]]*=[[:space:]]*"([A-Za-z0-9_-]+)"/\1/p' "$WORKER_CONFIG")"
  [[ -n "$DB_NAME" ]] || die "Worker config has no usable database_name"
  DATABASE_ID="$(sed -nE 's/^database_id[[:space:]]*=[[:space:]]*"([A-Za-z0-9-]+)"/\1/p' "$WORKER_CONFIG")"
  if [[ "$DATABASE_ID" == "REPLACE_WITH_D1_DATABASE_ID" || -z "$DATABASE_ID" ]]; then
    echo "The Worker config has no D1 ID, likely because an earlier setup stopped after database creation."
    read -r -p "Enter the existing D1 database ID: " DATABASE_ID
    [[ "$DATABASE_ID" =~ ^[A-Za-z0-9-]+$ ]] || die "Invalid D1 database ID"
    sed -i.bak "s/REPLACE_WITH_D1_DATABASE_ID/${DATABASE_ID}/" "$WORKER_CONFIG"
    rm -f "${WORKER_CONFIG}.bak"
  fi
fi

echo
echo "Applying the remote D1 migration..."
(cd "$WORKER_DIR" && npx wrangler d1 migrations apply "$DB_NAME" --remote --config "$WORKER_CONFIG")

read -r -p "Receiver ID [receiver-01]: " RECEIVER_ID
RECEIVER_ID="${RECEIVER_ID:-receiver-01}"
[[ "$RECEIVER_ID" =~ ^[A-Za-z0-9_-]{1,128}$ ]] || die "Receiver ID must contain only letters, numbers, '_' or '-'"

BOARD_DEFAULT="$(sed -nE 's/^[[:space:]]*ip:[[:space:]]*"([^"]+)".*/\1/p' "${SCRIPT_DIR}/config.yaml" | sed -n '1p')"
BOARD_IP="$(prompt_default "Receiver SSH address" "${BOARD_DEFAULT:-192.168.1.72}")"
echo "Checking existing SSH access to ${BOARD_IP}..."
ssh -o BatchMode=yes "$BOARD_IP" true || die "SSH access failed. Configure SSH access to the receiver before running setup."

mkdir -p "$STATE_DIR"
chmod 700 "$STATE_DIR"

if [[ -f "$PUBLIC_KEY_LOCAL" && -f "$PRIVATE_KEY_LOCAL" && -f "$ROOT_SECRET_LOCAL" ]]; then
  echo "Existing local Cloudflare credentials found in $STATE_DIR"
  if confirm "Generate new credentials and invalidate the old Worker credentials"; then
    rm -f "$PUBLIC_KEY_LOCAL" "$PRIVATE_KEY_LOCAL" "$ROOT_SECRET_LOCAL"
  else
    echo "Reusing existing credentials."
  fi
fi

if [[ ! -f "$PUBLIC_KEY_LOCAL" || ! -f "$PRIVATE_KEY_LOCAL" || ! -f "$ROOT_SECRET_LOCAL" ]]; then
  echo "Generating RSA token keys and registration credentials..."
  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$PRIVATE_KEY_LOCAL" 2>/dev/null
  openssl pkey -in "$PRIVATE_KEY_LOCAL" -pubout -out "$PUBLIC_KEY_LOCAL" 2>/dev/null
  root_hex="$(openssl rand -hex 32)"
  generate_registration_credentials
  printf '%s\n' "$REGISTRATION_ROOT_SECRET" > "$ROOT_SECRET_LOCAL"
  chmod 600 "$PRIVATE_KEY_LOCAL" "$ROOT_SECRET_LOCAL"
else
  REGISTRATION_ROOT_SECRET="$(tr -d '\r\n' < "$ROOT_SECRET_LOCAL")"
  RECEIVER_REGISTRATION_SECRET="$(node - "$REGISTRATION_ROOT_SECRET" "cast-registration-v1:${RECEIVER_ID}" <<'NODE'
const crypto = require('node:crypto');
const [rootSecret, message] = process.argv.slice(2);
const root = Buffer.from(rootSecret.replace(/-/g, '+').replace(/_/g, '/'), 'base64');
process.stdout.write(crypto.createHmac('sha256', root).update(message, 'utf8').digest('base64url'));
NODE
)"
fi

echo
echo "Uploading the receiver public key to ${BOARD_IP}..."
if ssh -o BatchMode=yes "$BOARD_IP" "test -d /var/lib/llrdc-pairing && test -w /var/lib/llrdc-pairing"; then
  scp -o BatchMode=yes "$PUBLIC_KEY_LOCAL" "${BOARD_IP}:/var/lib/llrdc-pairing/public.pem"
else
  echo "The pairing directory is protected or not writable by the SSH user."
  echo "The script will not change its permissions. It can use your existing sudo access"
  echo "to install only the public key with mode 0644."
  confirm "Install the public key using sudo on the receiver" || die "Public key installation cancelled"
  remote_key="/tmp/llrdc-receiver-public-$$.pem"
  scp -o BatchMode=yes "$PUBLIC_KEY_LOCAL" "${BOARD_IP}:${remote_key}"
  ssh -tt "$BOARD_IP" "sudo test -d /var/lib/llrdc-pairing && sudo install -m 644 '${remote_key}' /var/lib/llrdc-pairing/public.pem && rm -f '${remote_key}'"
fi

echo
echo "Uploading Worker secrets..."
(cd "$WORKER_DIR" && printf '%s' "$REGISTRATION_ROOT_SECRET" | npx wrangler secret put RECEIVER_REGISTRATION_SECRET --config "$WORKER_CONFIG")
(cd "$WORKER_DIR" && npx wrangler secret put PAIRING_TOKEN_PRIVATE_KEY --config "$WORKER_CONFIG" < "$PRIVATE_KEY_LOCAL")

cat > "$RECEIVER_ENV" <<EOF
# Generated by setup_cloudflare.sh. Do not commit this file.
SERVER_CLOUD_DISCOVERY_ENABLED=1
SERVER_PAIRING_WORKER_URL=https://${DOMAIN}
SERVER_RECEIVER_ID=${RECEIVER_ID}
SERVER_RECEIVER_REGISTRATION_SECRET=${RECEIVER_REGISTRATION_SECRET}
SERVER_PAIRING_TOKEN_PUBLIC_KEY_FILE=/pairing/public.pem
EOF
chmod 600 "$RECEIVER_ENV"

echo
echo "Deploying ${DOMAIN}..."
(cd "$WORKER_DIR" && npm run check && npm run deploy -- --config "$WORKER_CONFIG" --domains "$DOMAIN")

echo
echo "Starting the receiver with Cloud discovery enabled..."
(cd "$SCRIPT_DIR" && BOARD_IP="$BOARD_IP" ./server.sh --start)

echo
echo "Running public endpoint smoke tests..."
if curl -fsSI --max-time 20 "https://${DOMAIN}/" >/dev/null; then
  echo "PASS: https://${DOMAIN}/ is reachable"
else
  echo "WARN: ${DOMAIN} is not reachable yet. DNS/custom-domain propagation may still be pending."
fi

PAIR_STATUS="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 \
  -X POST "https://${DOMAIN}/api/pair" \
  -H 'content-type: application/json' \
  --data '{"code":"0000"}' || true)"
if [[ "$PAIR_STATUS" == "400" || "$PAIR_STATUS" == "429" ]]; then
  echo "PASS: pairing API rejects an invalid code safely (HTTP ${PAIR_STATUS})"
else
  echo "WARN: pairing API smoke test returned HTTP ${PAIR_STATUS}"
fi

echo
echo "Setup complete."
echo "Cloud URL: https://${DOMAIN}/"
echo "Receiver: ${BOARD_IP} (${RECEIVER_ID})"
echo "The HDMI screen will show the current four-digit pairing code."
echo "Local secrets and keys are stored under ${STATE_DIR} and ignored by git."
