# CAST Pairing Worker

This package implements the optional Cloudflare discovery control plane from
`CAST_PAIRING_PLAN.md`. It serves only a small fixed pairing bootstrap at
`https://cast.llrdc.com`; the full casting page is fetched from the receiver
over the authenticated LAN WebTransport session and replaces the bootstrap
without changing the browser URL. It is not required for direct-IP LAN
casting. A receiver must continue to work with no Internet connection when this
Worker is absent. D1 stores the active receiver, replay nonces, and rate-limit
counters. No media or WebTransport traffic passes through this Worker.

## Deploy

For the complete interactive setup, run this from the repository root:

```sh
./setup_cloudflare.sh
```

The script checks Wrangler login, creates or accepts a D1 database, applies the
migration, generates the Worker RSA key and receiver-specific HMAC credential,
uploads the public key to the board, deploys the custom domain, and starts the
receiver with cloud discovery enabled. It stores private setup state under
`.cloudflare/`, which is ignored by git.

Before running it, SSH to the receiver must already work using the address and
user you enter. The script does not install or configure SSH. If the receiver's
`/var/lib/llrdc-pairing` directory is root-protected, the script asks to use
your existing `sudo` access to install only `public.pem`; it does not change
the directory ownership.

The setup command is resumable. It writes non-secret checkpoints to
`.cloudflare/setup-state.json`, discovers a D1 database by name after an
interrupted create, and verifies the remote database, Worker secrets, receiver
key, deployment, registration, and public bootstrap before reporting success.
If a run stops after creating D1 or during deployment, run the same command
again; it will inspect the existing resources and continue from the first
incomplete phase. Use `./setup_cloudflare.sh --status` for a read-only report or
`./setup_cloudflare.sh --verify` to run checks without changing state.

Use `./setup_cloudflare.sh --rotate-credentials` only when intentionally
invalidating the existing Worker credentials. Replacement files are prepared
before they are promoted locally, and a failed run reports the phase to retry.

1. Copy `wrangler.toml.example` to `wrangler.toml` and set the D1 database ID.
2. Deploy from this directory. The Worker contains only the fixed bootstrap;
   receiver UI changes are embedded and deployed with the receiver binary.
3. Create the database and apply the migration:

   ```sh
   npx wrangler d1 create cast-pairing
   npm run db:migrate:remote
   ```

4. Put the secrets in the Worker, not in source control:

   ```sh
   npx wrangler secret put RECEIVER_REGISTRATION_SECRET
   npx wrangler secret put PAIRING_TOKEN_PRIVATE_KEY
   npm run deploy
   ```

5. Attach the Worker to the configured custom domain in Cloudflare. The
   root page and `/api/pair` must be served by this Worker. Do not create a DNS
   record pointing at the receiver's private IP.

## Test Before Production

Run the Worker locally with a local D1 database:

```sh
npm run db:migrate:local
npm run dev
```

Check that `/` serves the minimal pairing bootstrap and `/api/pair` rejects malformed
requests without exposing receiver data. A complete pairing test requires a
running receiver configured with `CLOUD_DISCOVERY_ENABLED=1`, its unique
`RECEIVER_ID`, the receiver-specific registration HMAC key, and the matching
RSA public key. After deployment, verify:

```sh
curl -I https://cast.llrdc.com/
curl -i -X POST https://cast.llrdc.com/api/pair \
  -H 'content-type: application/json' \
  --data '{"code":"A78Q"}'
```

The first request must return the small pairing bootstrap with a trusted certificate. The second
must return a generic invalid-code response until a receiver has registered a
matching code. Do not test WebTransport through the Worker: the browser must
connect directly to the private LAN address returned by `/api/pair` after the
bootstrap has loaded the receiver UI.

`RECEIVER_REGISTRATION_SECRET` is a base64url-encoded random root key of at
least 32 bytes. A receiver is provisioned with the derived device key, not the
root key. The device key is the raw HMAC-SHA-256 result of:

```text
HMAC-SHA256(root_key, UTF8("cast-registration-v1:" + receiver_id))
```

Provision that result as base64url in the receiver's
`RECEIVER_REGISTRATION_SECRET`. This permits one Worker root key while giving
each receiver a separate registration credential.

`PAIRING_TOKEN_PRIVATE_KEY` must be an RSA PKCS#8 PEM private key. Generate a
matching receiver public key as an RSA SPKI PEM. The private key is never sent
to a browser or receiver.

## Registration API

The receiver sends this JSON body, with no client-controlled expiry fields:

```json
{
  "receiver_id": "receiver-01",
  "ip_address": "192.168.1.42",
  "webtransport_port": 4433,
  "cert_hash_hex": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
}
```

Required headers are:

```text
X-Receiver-Timestamp: Unix seconds
X-Receiver-Nonce: 8-128 characters matching [A-Za-z0-9._~-]
X-Receiver-Signature: lowercase or uppercase HMAC-SHA-256 hex
```

The signed bytes are exactly the UTF-8 bytes of:

```text
<timestamp>\n<nonce>\n<raw request body bytes>
```

The HMAC key is the receiver-specific derived key described above. Timestamps
are accepted within five minutes, and `(receiver_id, nonce)` is stored in D1 to
reject replay. The Worker allocates the fleet-unique pairing code and returns
it with the authoritative code and registration expiries. Registration rejects
non-RFC1918 IPv4 addresses, reuses an unexpired code for the same receiver, and
retries random allocation when another receiver owns a candidate code.

## Pairing API

The browser sends `{ "code": "A78Q" }` to `/api/pair`. The code must be four
uppercase alphanumeric characters (`A-Z`, `0-9`); lowercase input is normalized
to uppercase. The Worker applies D1-backed limits of 10 attempts per client IP
and 5 attempts per code per 60 seconds. A successful match atomically clears the
code before returning. Expired, missing, consumed, and unavailable codes use a
generic error and never reveal another receiver's data.

Successful responses contain the current LAN endpoint, certificate hash, and a
short-lived token. All API responses, including errors, have
`Cache-Control: no-store`.

## Connection Token Format

The token is a compact, versioned RSA-PSS envelope:

```text
v1.<base64url(header JSON)>.<base64url(payload JSON)>.<base64url(signature)>
```

The header is:

```json
{"alg":"PS256","typ":"CAST-CONNECTION","v":1}
```

The payload is:

```json
{
  "receiver_id": "receiver-01",
  "purpose": "webtransport-connect",
  "iat": 1730000000,
  "exp": 1730000060,
  "jti": "32 lowercase random hex characters"
}
```

The signature is RSA-PSS with SHA-256 and a 32-byte salt over the exact UTF-8
bytes of `v1.<header segment>.<payload segment>`. It is the raw RSA signature
bytes, base64url encoded without padding. `exp - iat` is 60 seconds. The
receiver must verify the signature with its provisioned RSA SPKI public key,
require the matching receiver ID and purpose, enforce `iat`/`exp`, and keep a
bounded replay cache keyed by `jti` until expiration. The token is an
authorization credential, not a substitute for the pairing code.
