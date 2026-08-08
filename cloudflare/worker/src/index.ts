interface Env {
  DB: D1Database;
  ASSETS: Fetcher;
  RECEIVER_REGISTRATION_SECRET: string;
  PAIRING_TOKEN_PRIVATE_KEY: string;
}

interface JsonObject {
  [key: string]: unknown;
}

interface ReceiverRegistration {
  receiverId: string;
  ipAddress: string;
  webtransportPort: number;
  certHashHex: string;
  pairingCode: string;
}

interface PairResult {
  receiver_id: string;
  ip_address: string;
  webtransport_port: number;
  cert_hash_hex: string;
}

interface RateLimitRow {
  attempt_count: number;
}

const CODE_TTL_SECONDS = 3600;
const REGISTRATION_TTL_SECONDS = 3600;
const REGISTRATION_TIMESTAMP_SKEW_SECONDS = 300;
const REGISTRATION_REPLAY_TTL_SECONDS = 600;
const CONNECTION_TOKEN_TTL_SECONDS = 60;
const MAX_BODY_BYTES = 16 * 1024;

const RATE_LIMITS = {
  pairIp: { maxAttempts: 10, windowSeconds: 60 },
  pairCode: { maxAttempts: 5, windowSeconds: 60 },
  registration: { maxAttempts: 20, windowSeconds: 60 },
} as const;

const JSON_HEADERS = {
  "cache-control": "no-store",
  "content-type": "application/json; charset=utf-8",
  pragma: "no-cache",
};

function jsonResponse(
  body: JsonObject,
  status = 200,
  extraHeaders: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { ...JSON_HEADERS, ...extraHeaders },
  });
}

function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

async function readJsonBody(
  request: Request,
): Promise<{ text: string; bytes: Uint8Array } | null> {
  const contentLength = request.headers.get("content-length");
  if (contentLength !== null && Number(contentLength) > MAX_BODY_BYTES) {
    return null;
  }

  const body = new Uint8Array(await request.arrayBuffer());
  if (body.byteLength > MAX_BODY_BYTES) {
    return null;
  }

  try {
    return { text: new TextDecoder("utf-8", { fatal: true }).decode(body), bytes: body };
  } catch {
    return null;
  }
}

function parseJson(text: string): JsonObject | null {
  try {
    const value: unknown = JSON.parse(text);
    return isJsonObject(value) ? value : null;
  } catch {
    return null;
  }
}

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function parseRegistration(body: JsonObject): ReceiverRegistration | null {
  const receiverId = body.receiver_id;
  const ipAddress = body.ip_address;
  const port = body.webtransport_port;
  const certHash = body.cert_hash_hex;
  const pairingCode = body.pairing_code;

  if (!isString(receiverId) || !/^[A-Za-z0-9_-]{1,128}$/.test(receiverId)) {
    return null;
  }
  if (!isString(ipAddress) || !isPrivateIpv4(ipAddress)) {
    return null;
  }
  if (
    typeof port !== "number" ||
    !Number.isInteger(port) ||
    port < 1 ||
    port > 65535
  ) {
    return null;
  }
  if (!isString(certHash) || !/^[0-9a-fA-F]{64}$/.test(certHash)) {
    return null;
  }
  if (!isString(pairingCode) || !/^\d{4}$/.test(pairingCode)) {
    return null;
  }

  return {
    receiverId,
    ipAddress,
    webtransportPort: port,
    certHashHex: certHash.toLowerCase(),
    pairingCode,
  };
}

function isPrivateIpv4(value: string): boolean {
  const octets = value.split(".");
  if (octets.length !== 4 || octets.some((octet) => !/^\d{1,3}$/.test(octet))) {
    return false;
  }

  const numbers = octets.map(Number);
  if (numbers.some((octet) => octet > 255)) {
    return false;
  }

  return (
    numbers[0] === 10 ||
    (numbers[0] === 172 && numbers[1] >= 16 && numbers[1] <= 31) ||
    (numbers[0] === 192 && numbers[1] === 168)
  );
}

function parseTimestamp(value: string | null): number | null {
  if (value === null || !/^\d{1,12}$/.test(value)) {
    return null;
  }
  const timestamp = Number(value);
  return Number.isSafeInteger(timestamp) ? timestamp : null;
}

function isValidNonce(value: string | null): value is string {
  return value !== null && /^[A-Za-z0-9._~-]{8,128}$/.test(value);
}

function hexToBytes(value: string): Uint8Array | null {
  if (!/^[0-9a-fA-F]+$/.test(value) || value.length % 2 !== 0) {
    return null;
  }
  const bytes = new Uint8Array(value.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
  }
  return bytes;
}

function base64UrlToBytes(value: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*$/.test(value)) {
    throw new Error("invalid base64url value");
  }
  const padded = value.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  return base64ToBytes(padded);
}

function base64ToBytes(value: string): Uint8Array {
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(value)) {
    throw new Error("invalid base64 value");
  }
  const decoded = atob(value);
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

function bytesToBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

function concatBytes(first: Uint8Array, second: Uint8Array): Uint8Array {
  const result = new Uint8Array(first.length + second.length);
  result.set(first);
  result.set(second, first.length);
  return result;
}

function utf8(value: string): Uint8Array {
  return new TextEncoder().encode(value);
}

function ownedArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  const buffer = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(buffer).set(bytes);
  return buffer;
}

function constantTimeEqual(first: Uint8Array, second: Uint8Array): boolean {
  if (first.length !== second.length) {
    return false;
  }
  let difference = 0;
  for (let index = 0; index < first.length; index += 1) {
    difference |= first[index] ^ second[index];
  }
  return difference === 0;
}

async function deriveRegistrationKey(
  rootSecret: string,
  receiverId: string,
): Promise<CryptoKey> {
  const root = base64UrlToBytes(rootSecret);
  if (root.length < 32) {
    throw new Error("registration secret is too short");
  }
  const rootKey = await crypto.subtle.importKey(
    "raw",
    ownedArrayBuffer(root),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const derived = new Uint8Array(
    await crypto.subtle.sign("HMAC", rootKey, ownedArrayBuffer(utf8(`cast-registration-v1:${receiverId}`))),
  );
  return crypto.subtle.importKey(
    "raw",
    ownedArrayBuffer(derived),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
}

async function authenticateRegistration(
  bodyBytes: Uint8Array,
  receiverId: string,
  request: Request,
  env: Env,
  now: number,
): Promise<boolean> {
  const timestampText = request.headers.get("x-receiver-timestamp");
  const nonce = request.headers.get("x-receiver-nonce");
  const signatureText = request.headers.get("x-receiver-signature");
  const timestamp = parseTimestamp(timestampText);
  const signature = signatureText === null ? null : hexToBytes(signatureText);

  if (
    timestamp === null ||
    !isValidNonce(nonce) ||
    signature === null ||
    signature.length !== 32 ||
    Math.abs(now - timestamp) > REGISTRATION_TIMESTAMP_SKEW_SECONDS
  ) {
    return false;
  }

  const key = await deriveRegistrationKey(env.RECEIVER_REGISTRATION_SECRET, receiverId);
  const expected = new Uint8Array(
    await crypto.subtle.sign(
      "HMAC",
      key,
      ownedArrayBuffer(concatBytes(utf8(`${timestampText}\n${nonce}\n`), bodyBytes)),
    ),
  );
  return constantTimeEqual(expected, signature);
}

async function consumeRateLimit(
  db: D1Database,
  bucketKey: string,
  maxAttempts: number,
  windowSeconds: number,
  now: number,
): Promise<boolean> {
  const row = await db
    .prepare(
      `INSERT INTO rate_limits (bucket_key, window_started_at, attempt_count)
       VALUES (?, ?, 1)
       ON CONFLICT(bucket_key) DO UPDATE SET
         window_started_at = CASE
           WHEN rate_limits.window_started_at + ? <= ? THEN ?
           ELSE rate_limits.window_started_at
         END,
         attempt_count = CASE
           WHEN rate_limits.window_started_at + ? <= ? THEN 1
           ELSE rate_limits.attempt_count + 1
         END
       RETURNING attempt_count`,
    )
    .bind(bucketKey, now, windowSeconds, now, now, windowSeconds, now)
    .first<RateLimitRow>();

  return row !== null && row.attempt_count <= maxAttempts;
}

function scheduleCleanup(ctx: ExecutionContext, env: Env, now: number): void {
  ctx.waitUntil(
    env.DB.batch([
      env.DB.prepare("DELETE FROM registration_replays WHERE expires_at <= ?").bind(now),
      env.DB.prepare("DELETE FROM rate_limits WHERE window_started_at + ? <= ?").bind(3600, now),
    ]).catch(() => undefined),
  );
}

async function handleRegistration(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
): Promise<Response> {
  const parsedBody = await readJsonBody(request);
  if (parsedBody === null) {
    return jsonResponse({ error: "invalid request body" }, 400);
  }
  const body = parseJson(parsedBody.text);
  const registration = body === null ? null : parseRegistration(body);
  if (registration === null) {
    return jsonResponse({ error: "invalid registration" }, 400);
  }

  const now = Math.floor(Date.now() / 1000);
  scheduleCleanup(ctx, env, now);
  if (
    !(await consumeRateLimit(
      env.DB,
      `registration:${registration.receiverId}`,
      RATE_LIMITS.registration.maxAttempts,
      RATE_LIMITS.registration.windowSeconds,
      now,
    ))
  ) {
    return jsonResponse({ error: "rate limit exceeded" }, 429, { "retry-after": "60" });
  }

  if (!(await authenticateRegistration(parsedBody.bytes, registration.receiverId, request, env, now))) {
    return jsonResponse({ error: "authentication failed" }, 401);
  }

  const nonce = request.headers.get("x-receiver-nonce");
  if (!isValidNonce(nonce)) {
    return jsonResponse({ error: "authentication failed" }, 401);
  }
  try {
    await env.DB.prepare(
      "INSERT INTO registration_replays (receiver_id, nonce, expires_at) VALUES (?, ?, ?)",
    )
      .bind(registration.receiverId, nonce, now + REGISTRATION_REPLAY_TTL_SECONDS)
      .run();
  } catch (error) {
    if (error instanceof Error && /unique|constraint/i.test(error.message)) {
      return jsonResponse({ error: "authentication failed" }, 401);
    }
    throw error;
  }

  const codeExpiresAt = now + CODE_TTL_SECONDS;
  const registrationExpiresAt = now + REGISTRATION_TTL_SECONDS;
  try {
    await env.DB.batch([
      env.DB.prepare(
        "DELETE FROM active_receivers WHERE code_expires_at <= ? OR registration_expires_at <= ?",
      ).bind(now, now),
      env.DB.prepare(
        `INSERT INTO active_receivers
          (receiver_id, pairing_code, ip_address, webtransport_port, cert_hash_hex,
           code_expires_at, registration_expires_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(receiver_id) DO UPDATE SET
           pairing_code = excluded.pairing_code,
           ip_address = excluded.ip_address,
           webtransport_port = excluded.webtransport_port,
           cert_hash_hex = excluded.cert_hash_hex,
           code_expires_at = excluded.code_expires_at,
           registration_expires_at = excluded.registration_expires_at,
           updated_at = excluded.updated_at`,
      ).bind(
        registration.receiverId,
        registration.pairingCode,
        registration.ipAddress,
        registration.webtransportPort,
        registration.certHashHex,
        codeExpiresAt,
        registrationExpiresAt,
        now,
      ),
    ]);
  } catch (error) {
    if (error instanceof Error && /unique|constraint/i.test(error.message)) {
      return jsonResponse({ error: "pairing code unavailable", retryable: true }, 409);
    }
    throw error;
  }

  return jsonResponse({ ok: true, code_expires_at: codeExpiresAt, registration_expires_at: registrationExpiresAt });
}

function clientAddress(request: Request): string {
  return request.headers.get("cf-connecting-ip") ?? "unknown";
}

async function importSigningKey(privateKeyPem: string): Promise<CryptoKey> {
  const der = pemToDer(privateKeyPem, "PRIVATE KEY");
  return crypto.subtle.importKey(
    "pkcs8",
    ownedArrayBuffer(der),
    { name: "RSA-PSS", hash: "SHA-256" },
    false,
    ["sign"],
  );
}

function pemToDer(pem: string, label: string): Uint8Array {
  const expression = new RegExp(`^-----BEGIN ${label}-----([\\s\\S]+)-----END ${label}-----$`);
  const match = expression.exec(pem.trim());
  if (match === null) {
    throw new Error(`expected PEM ${label}`);
  }
  return base64ToBytes(match[1].replace(/\s/g, ""));
}

function jsonBase64Url(value: JsonObject): string {
  return bytesToBase64Url(utf8(JSON.stringify(value)));
}

function randomHex(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function createConnectionToken(
  privateKey: CryptoKey,
  receiverId: string,
  now: number,
): Promise<{ token: string; expiresAt: number }> {
  const header = jsonBase64Url({ alg: "PS256", typ: "CAST-CONNECTION", v: 1 });
  const expiresAt = now + CONNECTION_TOKEN_TTL_SECONDS;
  const payload = jsonBase64Url({
    receiver_id: receiverId,
    purpose: "webtransport-connect",
    iat: now,
    exp: expiresAt,
    jti: randomHex(16),
  });
  const signingInput = `v1.${header}.${payload}`;
  const signature = new Uint8Array(
    await crypto.subtle.sign(
      { name: "RSA-PSS", saltLength: 32 },
      privateKey,
      ownedArrayBuffer(utf8(signingInput)),
    ),
  );
  return { token: `${signingInput}.${bytesToBase64Url(signature)}`, expiresAt };
}

async function handlePair(
  request: Request,
  env: Env,
  ctx: ExecutionContext,
): Promise<Response> {
  const parsedBody = await readJsonBody(request);
  if (parsedBody === null) {
    return jsonResponse({ error: "invalid request body" }, 400);
  }
  const body = parseJson(parsedBody.text);
  const code = body?.code;
  if (!isString(code) || !/^\d{4}$/.test(code)) {
    return jsonResponse({ error: "invalid pairing code" }, 400);
  }

  const now = Math.floor(Date.now() / 1000);
  scheduleCleanup(ctx, env, now);
  const address = clientAddress(request);
  const ipAllowed = await consumeRateLimit(
    env.DB,
    `pair:ip:${address}`,
    RATE_LIMITS.pairIp.maxAttempts,
    RATE_LIMITS.pairIp.windowSeconds,
    now,
  );
  const codeAllowed = await consumeRateLimit(
    env.DB,
    `pair:code:${code}`,
    RATE_LIMITS.pairCode.maxAttempts,
    RATE_LIMITS.pairCode.windowSeconds,
    now,
  );
  if (!ipAllowed || !codeAllowed) {
    return jsonResponse({ error: "pairing temporarily unavailable" }, 429, { "retry-after": "60" });
  }

  // Import before consuming the code so a broken signing secret does not consume a valid code.
  const signingKey = await importSigningKey(env.PAIRING_TOKEN_PRIVATE_KEY);
  const result = await env.DB.prepare(
    `UPDATE active_receivers
       SET pairing_code = NULL, code_expires_at = 0, updated_at = ?
     WHERE pairing_code = ?
       AND code_expires_at > ?
       AND registration_expires_at > ?
     RETURNING receiver_id, ip_address, webtransport_port, cert_hash_hex`,
  )
    .bind(now, code, now, now)
    .first<PairResult>();

  if (result === null) {
    return jsonResponse({ error: "invalid or expired pairing code" }, 400);
  }

  const signedToken = await createConnectionToken(signingKey, result.receiver_id, now);
  return jsonResponse({
    receiver_id: result.receiver_id,
    ip_address: result.ip_address,
    webtransport_port: result.webtransport_port,
    cert_hash_hex: result.cert_hash_hex,
    connection_token: signedToken.token,
    token_expires_at: signedToken.expiresAt,
  });
}

async function route(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
  const path = new URL(request.url).pathname;
  if (request.method === "POST" && path === "/api/receiver/register") {
    return handleRegistration(request, env, ctx);
  }
  if (request.method === "POST" && path === "/api/pair") {
    return handlePair(request, env, ctx);
  }
  if (path.startsWith("/api/")) {
    return jsonResponse({ error: "not found" }, 404);
  }
  if (request.method === "GET" || request.method === "HEAD") {
    return env.ASSETS.fetch(request);
  }
  return jsonResponse({ error: "method not allowed" }, 405, { allow: "GET, HEAD, POST" });
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    try {
      return await route(request, env, ctx);
    } catch {
      return jsonResponse({ error: "service unavailable" }, 503);
    }
  },
};
