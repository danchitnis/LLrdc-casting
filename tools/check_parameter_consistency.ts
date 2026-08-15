import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import {
  CERTIFICATE_CONFIG,
  CODEC_RESOLUTION_LIMITS,
  DECODER_LIMITS,
  ENCODER_GUARDRAILS,
  PAIRING_CONFIG,
  TRANSPORT_CONFIG,
} from '../client/src/lib/config.ts';
import {
  BOOTSTRAP_LIMITS,
  CRYPTO_CONFIG,
  PAIRING_CODE_LENGTH,
  PAIRING_CODE_TTL_SECONDS,
  REQUEST_LIMITS,
  TOKEN_CONFIG,
} from '../cloudflare/worker/src/config.ts';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const rustConfig = readFileSync(resolve(root, 'src/config.rs'), 'utf8');

type Operator = '+' | '-' | '*' | '/' | '(' | ')';
type Token = number | Operator | string;

function tokenize(expression: string): Token[] {
  const tokens: Token[] = [];
  const pattern = /\s*(\d[\d_]*|[A-Z][A-Z0-9_]*|[()+\-*/])/gy;
  let position = 0;
  while (position < expression.length) {
    pattern.lastIndex = position;
    const match = pattern.exec(expression);
    if (!match) throw new Error(`Unsupported integer expression: ${expression}`);
    const token = match[1];
    tokens.push(/^\d[\d_]*$/.test(token) ? Number(token.replaceAll('_', '')) : token);
    position = pattern.lastIndex;
  }
  return tokens;
}

function evaluateInteger(expression: string, resolveIdentifier: (name: string) => number): number {
  const tokens = tokenize(expression);
  let position = 0;

  function primary(): number {
    const token = tokens[position++];
    if (typeof token === 'number') return token;
    if (token === '(') {
      const value = additive();
      if (tokens[position++] !== ')') throw new Error(`Unbalanced expression: ${expression}`);
      return value;
    }
    if (token === '-') return -primary();
    if (typeof token === 'string' && /^[A-Z][A-Z0-9_]*$/.test(token)) {
      return resolveIdentifier(token);
    }
    throw new Error(`Expected integer in expression: ${expression}`);
  }

  function multiplicative(): number {
    let value = primary();
    while (tokens[position] === '*' || tokens[position] === '/') {
      const operator = tokens[position++];
      const operand = primary();
      value = operator === '*' ? value * operand : Math.floor(value / operand);
    }
    return value;
  }

  function additive(): number {
    let value = multiplicative();
    while (tokens[position] === '+' || tokens[position] === '-') {
      const operator = tokens[position++];
      const operand = multiplicative();
      value = operator === '+' ? value + operand : value - operand;
    }
    return value;
  }

  const value = additive();
  if (position !== tokens.length) throw new Error(`Unexpected token in expression: ${expression}`);
  return value;
}

function escaped(name: string): string {
  return name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function extractRustInteger(name: string, ancestors: readonly string[] = []): number {
  if (ancestors.includes(name)) {
    throw new Error(`Circular Rust constant expression: ${[...ancestors, name].join(' -> ')}`);
  }
  const match = rustConfig.match(new RegExp(`pub const ${escaped(name)}: [^=]+\\s*=\\s*(?<value>[^;]+);`));
  if (!match?.groups?.value) throw new Error(`Missing Rust constant ${name}`);
  return evaluateInteger(
    match.groups.value,
    identifier => extractRustInteger(identifier, [...ancestors, name]),
  );
}

function extractRustString(name: string): string {
  const match = rustConfig.match(new RegExp(`pub const ${escaped(name)}: [^=]+\\s*=\\s*"(?<value>[^"]+)";`));
  if (!match?.groups?.value) throw new Error(`Missing Rust string constant ${name}`);
  return match.groups.value;
}

function extractRustByteString(name: string): string {
  const match = rustConfig.match(new RegExp(`pub const ${escaped(name)}: [^=]+\\s*=\\s*b"(?<value>[^"]+)";`));
  if (!match?.groups?.value) throw new Error(`Missing Rust byte string constant ${name}`);
  return match.groups.value;
}

function assertEqual(label: string, left: string | number, right: string | number): void {
  if (left !== right) throw new Error(`Parameter drift for ${label}: ${left} !== ${right}`);
}

const rustClientNumbers: ReadonlyArray<readonly [string, string, number]> = [
  ['DEFAULT_WEBTRANSPORT_PORT', 'direct WebTransport port', PAIRING_CONFIG.DIRECT_WEBTRANSPORT_PORT],
  ['CODEC_ALIGNMENT', 'codec alignment', ENCODER_GUARDRAILS.ALIGNMENT],
  ['CODEC_TAG_BYTES', 'codec tag bytes', TRANSPORT_CONFIG.PACKET_FIELD_BYTES.TAG],
  ['SEQUENCE_BYTES', 'sequence field bytes', TRANSPORT_CONFIG.PACKET_FIELD_BYTES.SEQUENCE],
  ['CHUNK_INDEX_BYTES', 'chunk-index field bytes', TRANSPORT_CONFIG.PACKET_FIELD_BYTES.CHUNK_INDEX],
  ['CHUNK_COUNT_BYTES', 'chunk-count field bytes', TRANSPORT_CONFIG.PACKET_FIELD_BYTES.CHUNK_COUNT],
  ['DIMENSION_BYTES', 'dimension field bytes', TRANSPORT_CONFIG.PACKET_FIELD_BYTES.WIDTH],
  ['TAG_OFFSET', 'codec tag offset', TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.TAG],
  ['SEQUENCE_OFFSET', 'sequence offset', TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.SEQUENCE],
  ['CHUNK_INDEX_OFFSET', 'chunk-index offset', TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.CHUNK_INDEX],
  ['CHUNK_COUNT_OFFSET', 'chunk-count offset', TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.CHUNK_COUNT],
  ['WIDTH_OFFSET', 'width offset', TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.WIDTH],
  ['HEIGHT_OFFSET', 'height offset', TRANSPORT_CONFIG.PACKET_FIELD_OFFSETS.HEIGHT],
  ['PACKET_HEADER_BYTES', 'packet header bytes', TRANSPORT_CONFIG.PACKET_HEADER_BYTES],
  ['LENGTH_PREFIX_BYTES', 'length prefix bytes', TRANSPORT_CONFIG.LENGTH_PREFIX_BYTES],
  ['PAIRING_CODE_LENGTH', 'pairing code length', PAIRING_CONFIG.CODE_LENGTH],
  ['MAX_ACCESS_UNIT_BYTES', 'maximum access-unit bytes', DECODER_LIMITS.MAX_ACCESS_UNIT_BYTES],
  ['MAX_CONTROL_MESSAGE_BYTES', 'maximum control-message bytes', TRANSPORT_CONFIG.MAX_CONTROL_MESSAGE_BYTES],
  ['H264_MAX_WIDTH', 'H.264 maximum width', CODEC_RESOLUTION_LIMITS.H264_MAX_WIDTH],
  ['H264_MAX_HEIGHT', 'H.264 maximum height', CODEC_RESOLUTION_LIMITS.H264_MAX_HEIGHT],
  ['H265_MAX_WIDTH', 'H.265 maximum width', CODEC_RESOLUTION_LIMITS.H265_MAX_WIDTH],
  ['H265_MAX_HEIGHT', 'H.265 maximum height', CODEC_RESOLUTION_LIMITS.H265_MAX_HEIGHT],
];

for (const [rustName, label, typescriptValue] of rustClientNumbers) {
  assertEqual(`Rust/client ${label}`, extractRustInteger(rustName), typescriptValue);
}

const rustWorkerNumbers: ReadonlyArray<readonly [string, string, number]> = [
  ['TOKEN_VERSION', 'token version', TOKEN_CONFIG.VERSION],
  ['PAIRING_CODE_LENGTH', 'pairing code length', PAIRING_CODE_LENGTH],
  ['PAIRING_CODE_TTL_SEC', 'pairing code TTL seconds', PAIRING_CODE_TTL_SECONDS],
  ['MAX_UI_BYTES', 'maximum UI bytes', BOOTSTRAP_LIMITS.MAX_UI_BYTES],
  ['LENGTH_PREFIX_BYTES', 'length prefix bytes', BOOTSTRAP_LIMITS.LENGTH_PREFIX_BYTES],
  ['PAIRING_TOKEN_MAX_LIFETIME_SEC', 'connection token lifetime seconds', TOKEN_CONFIG.CONNECTION_TTL_SECONDS],
  ['TOKEN_RSA_SALT_BYTES', 'RSA-PSS salt bytes', TOKEN_CONFIG.RSA_PSS_SALT_LENGTH],
  ['SHA256_DIGEST_BYTES', 'SHA-256 digest bytes', CRYPTO_CONFIG.SHA256_DIGEST_BYTES],
];

for (const [rustName, label, workerValue] of rustWorkerNumbers) {
  assertEqual(`Rust/Worker ${label}`, extractRustInteger(rustName), workerValue);
}

const rustWorkerStrings: ReadonlyArray<readonly [string, string, string]> = [
  ['TOKEN_PREFIX', 'token version prefix', TOKEN_CONFIG.PREFIX],
  ['TOKEN_ALGORITHM', 'token algorithm', TOKEN_CONFIG.ALGORITHM],
  ['TOKEN_TYPE', 'token type', TOKEN_CONFIG.TYPE],
  ['TOKEN_PURPOSE', 'token purpose', TOKEN_CONFIG.PURPOSE],
];
for (const [rustName, label, workerValue] of rustWorkerStrings) {
  assertEqual(`Rust/Worker ${label}`, extractRustString(rustName), workerValue);
}

const rustClientTags: ReadonlyArray<readonly [string, string, string]> = [
  ['H264_TAG', 'H.264 packet tag', TRANSPORT_CONFIG.CODEC_TAGS.H264],
  ['H265_TAG', 'H.265 packet tag', TRANSPORT_CONFIG.CODEC_TAGS.H265],
  ['STOP_TAG', 'stop packet tag', TRANSPORT_CONFIG.STOP_TAG],
];
for (const [rustName, label, clientValue] of rustClientTags) {
  assertEqual(`Rust/client ${label}`, extractRustByteString(rustName), clientValue);
  assertEqual(`client ${label} length`, clientValue.length, TRANSPORT_CONFIG.PACKET_FIELD_BYTES.TAG);
}

assertEqual(
  'client width/height field sizes',
  TRANSPORT_CONFIG.PACKET_FIELD_BYTES.WIDTH,
  TRANSPORT_CONFIG.PACKET_FIELD_BYTES.HEIGHT,
);

assertEqual(
  'client/Worker SHA-256 digest bytes',
  CERTIFICATE_CONFIG.SHA256_DIGEST_BYTES,
  CRYPTO_CONFIG.SHA256_DIGEST_BYTES,
);
assertEqual(
  'client/Worker SHA-256 fingerprint hex length',
  CERTIFICATE_CONFIG.SHA256_HEX_LENGTH,
  REQUEST_LIMITS.CERTIFICATE_HASH_HEX_LENGTH,
);
assertEqual('Worker token prefix/version', TOKEN_CONFIG.PREFIX, `v${TOKEN_CONFIG.VERSION}`);

console.log('shared Rust/TypeScript/Worker parameter values are consistent');
