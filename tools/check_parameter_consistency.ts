import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const rustConfig = readFileSync(resolve(root, 'src/config.rs'), 'utf8');
const typescriptConfig = readFileSync(resolve(root, 'client/src/lib/config.ts'), 'utf8');

type Token = number | '+' | '-' | '*' | '/' | '(' | ')';

function tokenize(expression: string): Token[] {
  const tokens: Token[] = [];
  const pattern = /\s*(\d+|[()+\-*/])/gy;
  let position = 0;
  while (position < expression.length) {
    pattern.lastIndex = position;
    const match = pattern.exec(expression);
    if (!match) throw new Error(`Unsupported integer expression: ${expression}`);
    const token = match[1];
    tokens.push(/^\d+$/.test(token) ? Number(token) : token as Exclude<Token, number>);
    position = pattern.lastIndex;
  }
  return tokens;
}

function evaluateInteger(expression: string): number {
  const tokens = tokenize(expression.replaceAll('_', ''));
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

function extractRust(name: string): number {
  const match = rustConfig.match(new RegExp(`pub const ${escaped(name)}: usize = (?<value>[^;]+);`));
  if (!match?.groups?.value) throw new Error(`Missing Rust constant ${name}`);
  return evaluateInteger(match.groups.value);
}

function extractTypeScript(name: string): number {
  const match = typescriptConfig.match(new RegExp(`\\b${escaped(name)}: (?<value>[0-9_ *+\\/()\\-]+),`));
  if (!match?.groups?.value) throw new Error(`Missing TypeScript constant ${name}`);
  return evaluateInteger(match.groups.value);
}

const sharedValues: ReadonlyArray<readonly [string, string]> = [
  ['PACKET_HEADER_BYTES', 'PACKET_HEADER_BYTES'],
  ['CHUNK_BYTES', 'CHUNK_BYTES'],
  ['PAIRING_CODE_LENGTH', 'CODE_LENGTH'],
  ['MAX_ACCESS_UNIT_BYTES', 'MAX_ACCESS_UNIT_BYTES'],
  ['MAX_CONTROL_MESSAGE_BYTES', 'MAX_CONTROL_MESSAGE_BYTES'],
  ['H264_MAX_WIDTH', 'H264_MAX_WIDTH'],
  ['H264_MAX_HEIGHT', 'H264_MAX_HEIGHT'],
  ['H265_MAX_WIDTH', 'H265_MAX_WIDTH'],
  ['H265_MAX_HEIGHT', 'H265_MAX_HEIGHT'],
];

for (const [rustName, typescriptName] of sharedValues) {
  const rustValue = extractRust(rustName);
  const typescriptValue = extractTypeScript(typescriptName);
  if (rustValue !== typescriptValue) {
    throw new Error(
      `Parameter drift for ${rustName}: Rust=${rustValue}, TypeScript=${typescriptValue}`,
    );
  }
}

console.log('shared Rust/TypeScript parameter values are consistent');
