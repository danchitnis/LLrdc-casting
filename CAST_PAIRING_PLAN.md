# Direct-IP LAN WebTransport Pairing Plan

## Goal

Keep the existing direct-IP LAN workflow as the mandatory, offline-capable mode. Optionally provide a fixed `https://cast.llrdc.com` entry point using Cloudflare discovery when the user enables it.

The receiver always displays a four-digit code on its HDMI idle screen. In direct-IP mode, the user opens `https://<receiver-ip>:8080/`, accepts the receiver's local certificate warning as before, enters the code, and streams without Internet access. In optional Cloudflare mode, the user may open `https://cast.llrdc.com`, enter the same code, and the page silently opens direct WebTransport to the LAN receiver.

All video, control, and WebTransport payloads travel directly between the browser and receiver on the LAN. Cloudflare is optional metadata discovery only and is never required for receiver startup or direct-IP streaming.

## Architecture

```text
Mandatory offline mode:

```text
Receiver -> HDMI dashboard: current LAN IP + four-digit code
Browser  -> https://<receiver-ip>:8080/: local UI
Browser  -> Receiver private IP: local code-authenticated WebTransport
```

Optional Cloudflare mode:

```text
Receiver -> Cloudflare Worker/D1: registration metadata only
Browser  -> https://cast.llrdc.com: static UI and optional lookup
Browser  -> Receiver private IP: local-code-authenticated WebTransport
```
```

Optional Cloudflare registration contains:

- Receiver ID
- Current private IPv4 address
- WebTransport UDP port
- Current certificate SHA-256 fingerprint
- Current four-digit pairing code
- Code expiry
- Registration expiry

Direct-IP flow:

1. The receiver generates and displays a local four-digit code.
2. The browser loads the local page from the receiver IP.
3. The browser fetches the local certificate fingerprint and opens direct WebTransport with the entered code.
4. The receiver validates the code locally before accepting the session.
5. Browser and receiver use that WebTransport connection for video, control commands, and telemetry.

Optional Cloudflare flow:

1. The Worker looks up the receiver using the entered code.
2. The page receives the LAN endpoint and certificate fingerprint.
3. The page opens direct WebTransport with both the local four-digit code and optional short-lived Worker token.
4. The receiver still validates the local code. The Worker token is an additional optional authorization check, never the only local requirement.

The visible page remains `https://cast.llrdc.com`, protected by Cloudflare's normal certificate. The receiver's short-lived self-signed certificate is authenticated through WebTransport certificate pinning, so no browser certificate warning is shown.

## Mandatory Design Rules

- Do not update Cloudflare DNS records.
- Do not redirect the browser to a private IP.
- Do not use WebRTC.
- Do not proxy media, QUIC, or video through Cloudflare.
- Cloudflare configuration must never be required to start the receiver.
- Direct-IP streaming must work with the receiver and browser disconnected from the Internet.
- Local code generation, expiry, throttling, and validation must remain in a core LAN module.
- Cloudflare registration and token validation must be separate optional modules.
- Do not retain the direct `wss://<private-ip>:8080/ws` control channel.

## Optional User-Owned Cloudflare Tasks

These tasks are required only for the optional fixed-URL mode. They are not required for direct-IP LAN mode.

1. Add `cast.llrdc.com` to Cloudflare DNS as a proxied hostname.
2. Deploy the Cloudflare Worker static asset site in `cloudflare/worker/`, serving the built casting UI at `https://cast.llrdc.com`.
3. Create a Cloudflare Worker for pairing and registration APIs.
4. Create a D1 database and bind it to the Worker.
5. Create Worker secrets:
   - `RECEIVER_REGISTRATION_SECRET`
   - `PAIRING_TOKEN_PRIVATE_KEY`
6. Give each receiver:
   - A unique `RECEIVER_ID`
   - The registration secret or a receiver-specific derived secret
   - The public key corresponding to `PAIRING_TOKEN_PRIVATE_KEY`
7. Restrict Worker routes to the required API paths.
8. Set production rate limits in Worker code:
   - Pair attempts per client IP
   - Pair attempts per code
   - Registration attempts per receiver ID
9. Configure Cloudflare Access only if the casting page itself should require organization login. Pairing code validation remains required even if Access is enabled.

Do not create an `A`, `AAAA`, or CNAME record for the receiver's changing LAN address.

If these tasks are not completed, the receiver and direct-IP mode must continue working normally.

## Worker and D1 Design

Use D1 rather than Workers KV for active receiver state. Pairing needs consistent reads and atomic code consumption; KV eventual consistency can return stale address or code state after a network change.

Create an `active_receivers` table with:

- `receiver_id` as primary key
- `pairing_code`
- `ip_address`
- `webtransport_port`
- `cert_hash_hex`
- `code_expires_at`
- `registration_expires_at`
- `updated_at`

Enforce one active receiver per pairing code. On collision, reject registration with a retry response so the receiver generates another code.

Expose these endpoints:

```text
POST /api/receiver/register
POST /api/pair
```

`POST /api/receiver/register` requirements:

- Authenticate the receiver with an HMAC signature over canonical request bytes.
- Reject stale timestamps and replayed requests.
- Store the current address, certificate fingerprint, and pairing code.
- Accept only RFC1918 IPv4 addresses.
- Never return another receiver's registration data.

`POST /api/pair` requirements:

- Accept a four-digit code.
- Rate-limit invalid attempts.
- Reject expired or unavailable receivers.
- Atomically consume or rotate the submitted code.
- Return only the matched receiver endpoint, certificate hash, and a short-lived signed connection token.
- Set a no-store response header.

The signed connection token must include:

- Receiver ID
- Expiration, no more than 60 seconds
- Issued-at time
- Random token ID
- Intended purpose, such as `webtransport-connect`

Sign tokens using an asymmetric signature. The Worker holds the private key; each receiver contains only the public verification key. Do not place a shared Worker signing secret in browser code.

## Receiver Changes

### Configuration

Core local mode requires no Cloudflare variables. Optional discovery uses:

```text
CLOUD_DISCOVERY_ENABLED=0
PAIRING_WORKER_URL=https://cast.llrdc.com
RECEIVER_ID=<provisioned-unique-id>
RECEIVER_REGISTRATION_SECRET=<provisioned-secret>
PAIRING_TOKEN_PUBLIC_KEY=<provisioned-public-key>
PAIRING_CODE_TTL_SEC=120
PAIRING_REGISTRATION_TTL_SEC=90
WEBTRANSPORT_PORT=4433
```

Do not put Cloudflare API tokens or Worker signing private keys on the receiver. The receiver may contain only its registration credential and optional Worker public key.

### Local Pairing Core

Add `src/local_pairing.rs`. This module is mandatory and must not import Cloudflare, HTTP clients, D1, or Worker token code.

The module must:

1. Generate a cryptographically random four-digit code at startup.
2. Display the code even when there is no Internet connection.
3. Validate the code locally on the first WebTransport request.
4. Expire and rotate the code locally.
5. Rate-limit failed attempts by peer address.
6. Authorize the session only after successful local validation.
7. Keep local pairing state in memory and invalidate it on receiver restart.
8. Never use the current fallback address in `src/net.rs`; if no address exists, show offline status but keep the receiver process alive.

### Optional Cloud Discovery

Add `src/cloud_discovery.rs`. This module may import the HTTP client and Worker token verifier, but the core receiver must not depend on it.

1. Start only when `CLOUD_DISCOVERY_ENABLED=1`.
2. Register the current LAN endpoint and local code with the Worker.
3. Refresh or retry independently with backoff.
4. Update dashboard status without changing local pairing state.
5. Treat Worker outage, missing credentials, and Internet loss as non-fatal.

Use a network-change watcher when practical. Keep a periodic address check as a fallback because Wi-Fi roaming and DHCP lease changes may not produce reliable events on every target image.

### Idle Dashboard

Update `src/dashboard.rs` and `src/text.rs`.

Display:

```text
CAST: https://cast.llrdc.com
CODE: 4827
STATUS: Ready
```

Do not display a pairing URL containing a token or private IP as part of the normal user workflow.

Show clear offline states:

```text
STATUS: Waiting for network
STATUS: Registering receiver
STATUS: Pairing service unavailable
```

The dashboard should update immediately when the code rotates, registration fails, or the selected network address changes.

### WebTransport Authorization

Update `src/webtransport_server.rs`.

Before `session_request.accept()`:

1. Parse the WebTransport request path and query string.
2. Require and validate the local four-digit code in every mode.
3. If an optional Cloudflare token is present and optional verification is configured, verify its signature, receiver ID, purpose, expiry, and token ID.
4. Never reject direct-IP local mode because Cloudflare variables are missing.
5. Log rejection reason without logging codes, tokens, or private addresses.

Keep a bounded in-memory replay cache of recently accepted token IDs until their expiration. Allow a limited reconnect policy only if required by browser behavior.

The current server accepts every session at `src/webtransport_server.rs:117`. Add local code validation before accepting sessions.

### Control and Telemetry Migration

Remove the dependency on the direct WebSocket control channel:

- `client/src/pages/index.astro` currently starts `initControlSocket()`.
- `client/src/lib/streamer.ts` currently opens `wss://<private-ip>:8080/ws`.
- `src/http_server.rs` currently serves the WebSocket control endpoint.

Add one bidirectional WebTransport control stream per casting session.

Use length-prefixed UTF-8 JSON control messages. Reuse the existing `ControlCommand` and `TelemetryMessage` formats from `src/control.rs` where possible.

Required control behavior:

- Client sends `start`, `stop`, `ping`, and `get_status`.
- Receiver returns status and telemetry on the control stream.
- Receiver broadcasts active/in-use status to every authenticated session.
- Closing the WebTransport session must stop or safely detach associated control state.
- Video unidirectional streams remain separate from the control stream.

Do not use a WebSocket to the private IP. Browser WebSockets cannot use WebTransport certificate hashes and would show a certificate error for the self-signed receiver certificate.

### Client Pairing UI

Update the Astro client, primarily:

- `client/src/pages/index.astro`
- `client/src/lib/streamer.ts`

Required UI flow:

1. Direct-IP page loads at `https://<receiver-ip>:8080/` and remains the existing offline path.
2. Fixed-URL page loads at `https://cast.llrdc.com` only when optional Cloudflare mode is desired.
3. Both pages show a four-digit code input before casting settings.
4. Direct-IP mode fetches `/cert_hash` locally and connects to the current page host on WebTransport.
5. Cloudflare mode submits the code to `/api/pair`, then connects to the returned LAN endpoint.
6. Both modes include the four-digit code in the WebTransport request.
7. Disable all casting controls until local code validation and WebTransport connection succeed.
8. Never change `window.location` in Cloudflare mode.
9. Keep endpoint, certificate hash, and optional token only in memory.
10. On failure, show a generic receiver-unreachable or invalid-code error.

Replace the current assumptions that the page host is the receiver:

- `window.location.hostname` remains the WebTransport destination in direct-IP mode.
- `fetch('/cert_hash')` remains required for direct-IP mode.
- `initControlSocket()` must be replaced by WebTransport control initialization.

## Certificate Requirements

Keep the receiver certificate short-lived and persistent enough to avoid unnecessary churn.

WebTransport certificate pinning requires:

- Valid X.509v3 certificate
- Validity period under 14 days
- SHA-256 certificate fingerprint
- Compatible key algorithm, using ECDSA P-256 as the interoperable choice

The current 13-day receiver certificate behavior in `src/cert.rs` is aligned with this model. Ensure a new certificate fingerprint triggers receiver re-registration before the old certificate expires.

The public Cloudflare certificate applies only to the visible `cast.llrdc.com` page. The direct receiver certificate is not shown as a navigated site certificate.

## Testing Stages

### Stage 1: Unit Tests

Test code generation:

- Exactly four decimal digits.
- Cryptographically secure source.
- Collision retry behavior.
- Expiry behavior.

Test receiver registration signing:

- Valid signature accepted.
- Wrong secret rejected.
- Stale timestamp rejected.
- Modified body rejected.
- Replay rejected.

Test signed connection tokens:

- Valid token accepted.
- Expired token rejected.
- Wrong receiver ID rejected.
- Wrong purpose rejected.
- Invalid signature rejected.
- Replayed token rejected.

Test interface selection:

- Prefer reachable private Wi-Fi/Ethernet address.
- Ignore loopback and `0.0.0.0`.
- Ignore Docker and container addresses.
- No interface results in offline state, never the hard-coded fallback IP.

### Stage 2: Cloudflare Worker Tests

Test registration:

- Valid registration creates and refreshes a receiver record.
- Pairing code collision produces retry behavior.
- Expired records cannot pair.
- Registration expiration makes receiver unavailable.

Test pairing:

- Correct code returns current endpoint and a token.
- Wrong code returns a generic error.
- Rate limit activates after repeated incorrect entries.
- A consumed code cannot be used again.
- Response contains `Cache-Control: no-store`.
- Worker never logs raw secrets or complete connection tokens.

### Stage 3: Local Integration

Use a browser and receiver on the same test LAN.

Verify:

- Receiver has no Internet or Cloudflare configuration.
- Browser opens `https://<receiver-ip>:8080/`.
- The expected local certificate warning is shown once and can be accepted as in the previous workflow.
- User enters displayed code successfully.
- Browser establishes WebTransport directly to the private receiver IP.
- Screen capture reaches the receiver.
- QUIC media packets remain LAN-local using packet capture on the receiver or laptop.
- Start/stop/status work over the WebTransport control stream.

### Stage 4: Optional Cloudflare Integration

With Cloudflare configuration enabled separately, verify:

- Browser opens `https://cast.llrdc.com`.
- Page has a trusted Cloudflare certificate.
- The code lookup returns the current LAN endpoint.
- Browser address bar never changes to the private IP.
- WebTransport still connects directly to the receiver.
- Cloudflare receives only registration and pairing metadata, never media.

### Stage 5: Offline and Failure Testing

Verify:

- Disconnect receiver Internet before startup.
- Remove all Cloudflare environment variables.
- Direct-IP page still displays a code and accepts a valid code.
- Incorrect and expired local codes are rejected.
- Repeated failed local attempts are throttled.
- Cloudflare Worker outage does not affect direct-IP streaming.
- Receiver restart invalidates prior local pairing state.
- A missing optional public key does not stop WebTransport local mode.

### Stage 6: Network Change

While receiver is idle:

- Change DHCP address.
- Move from Ethernet to Wi-Fi.
- Disconnect and reconnect Wi-Fi.
- Change Wi-Fi SSID.
- Renew the DHCP lease.

Verify:

- Old code is invalidated.
- Display changes to registering, then shows a new code.
- Direct-IP users see the new address on the HDMI dashboard and reconnect using it.
- Existing browser sessions fail safely and do not send media to an old address.
- No Cloudflare DNS record changes occur.

### Stage 7: Security and Failure Testing

Verify:

- Attempting direct WebTransport without a local code is rejected.
- Reusing an expired local code is rejected.
- Brute-force local attempts are throttled.
- Optional Worker outage does not stop direct-IP access.
- Receiver restart invalidates prior local pairing state.
- Optional token replay is rejected when Cloudflare mode is enabled.
- Private IP is hidden only in the fixed-URL UI; it remains intentionally available in direct-IP mode and on the HDMI dashboard.

### Stage 8: Enterprise Wi-Fi Validation

Test on a representative authenticated enterprise Wi-Fi.

Verify:

- Browser and receiver can reach each other directly.
- UDP 4433 is permitted between Wi-Fi clients.
- HTTP/3/WebTransport is not blocked.
- Client isolation is disabled for the relevant SSID/VLAN.
- Browser Private Network Access behavior allows the direct WebTransport request.
- Roaming and DHCP renewal recover by reading the new IP from the HDMI dashboard.

## Corner Cases

- Four-digit codes collide. Handle collision during registration; never overwrite another active receiver.
- Four-digit codes are weak. Pairing rate limits and short expirations are mandatory.
- The local code is the mandatory LAN session authorization. It is short-lived and rate-limited.
- Wi-Fi can permit TCP but block UDP or QUIC. Detect WebTransport failure and give an actionable message.
- Enterprise client isolation can prevent all peer-to-peer traffic. This cannot be fixed in application code.
- The receiver may have multiple private addresses. Select one consistently and verify it is usable.
- Cloudflare Worker or D1 outage must not affect direct-IP mode or cause stale endpoint reuse.
- D1 pairing operations must be atomic so two clients cannot successfully consume the same code unexpectedly.
- Device clock drift can affect optional Worker tokens; local code expiry must use a monotonic/local expiry strategy.
- Browser refresh loses the in-memory pairing state. Require entering the current displayed code again.
- Certificate hash changes while the page is open. Direct-IP mode refetches `/cert_hash`; Cloudflare mode requires a new optional lookup.
- Do not log pairing codes, receiver secrets, certificate hashes, private IPs, or full tokens in production logs.
