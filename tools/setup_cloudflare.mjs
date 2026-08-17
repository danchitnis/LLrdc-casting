#!/usr/bin/env node

/* Resumable Cloudflare pairing setup.  The module uses only Node's standard
 * library; Wrangler and the receiver tools remain subprocesses. */
import {
  createHash,
  createHmac,
  createPrivateKey,
  createPublicKey,
  generateKeyPairSync,
  randomBytes,
} from "node:crypto";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline/promises";
import { stdin as defaultInput, stdout as defaultOutput, stderr as defaultError } from "node:process";
import { constants as fsConstants, existsSync } from "node:fs";
import {
  access,
  chmod,
  mkdir,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const WORKER_DIR = join(SCRIPT_DIR, "cloudflare", "worker");
const STATE_DIR = join(SCRIPT_DIR, ".cloudflare");
const STATE_FILE = join(STATE_DIR, "setup-state.json");
const WORKER_CONFIG = join(WORKER_DIR, "wrangler.toml");
const WORKER_CONFIG_EXAMPLE = join(WORKER_DIR, "wrangler.toml.example");
const PUBLIC_KEY_LOCAL = join(STATE_DIR, "receiver-public.pem");
const PRIVATE_KEY_LOCAL = join(STATE_DIR, "worker-private.pem");
const ROOT_SECRET_LOCAL = join(STATE_DIR, "registration-root.secret");
const RECEIVER_ENV = join(STATE_DIR, "receiver.env");
const CREDENTIAL_BACKUPS = [[PRIVATE_KEY_LOCAL, `${PRIVATE_KEY_LOCAL}.previous`], [PUBLIC_KEY_LOCAL, `${PUBLIC_KEY_LOCAL}.previous`], [ROOT_SECRET_LOCAL, `${ROOT_SECRET_LOCAL}.previous`]];
const DEFAULTS = { domain: "cast.llrdc.com", workerName: "cast-pairing-worker", dbName: "cast-pairing", receiverId: "receiver-01", boardIp: "192.168.1.72" };
const PHASES = ["preflight", "configuration", "database", "credentials", "receiver-key", "worker", "receiver", "verification"];

export class SetupError extends Error {
  constructor(message, options = {}) {
    super(message);
    this.name = "SetupError";
    this.phase = options.phase;
    this.recovery = options.recovery;
    this.cause = options.cause;
  }
}

export function parseArgs(argv) {
  const args = { mode: "reconcile", json: false, plain: false, noColor: false, rotate: false };
  for (const arg of argv) {
    if (arg === "--status") args.mode = "status";
    else if (arg === "--verify") args.mode = "verify";
    else if (arg === "--json") args.json = true;
    else if (arg === "--plain") args.plain = true;
    else if (arg === "--no-color") args.noColor = true;
    else if (arg === "--rotate-credentials") args.rotate = true;
    else if (arg === "--help" || arg === "-h") args.mode = "help";
    else throw new SetupError(`Unknown option: ${arg}`);
  }
  if (args.json && args.mode === "reconcile") throw new SetupError("--json is supported with --status or --verify only");
  return args;
}

export function terminalCapabilities({ stdin = defaultInput, stdout = defaultOutput, env = process.env } = {}) {
  const tty = Boolean(stdin.isTTY && stdout.isTTY);
  const color = !env.NO_COLOR && env.TERM !== "dumb" && !env.CI && (tty || env.FORCE_COLOR);
  const locale = env.LC_ALL || env.LC_CTYPE || env.LANG || "";
  const unicode = tty && !env.LLRDC_ASCII && !/^(C|POSIX)$/i.test(locale);
  const cursor = tty && env.TERM !== "dumb";
  const columns = Number(stdout.columns) > 30 ? Number(stdout.columns) : 80;
  return { tty, color: Boolean(color), unicode, cursor, columns };
}

function ansi(enabled, code, value) { return enabled ? `\u001b[${code}m${value}\u001b[0m` : value; }
function duration(ms) { return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`; }
function redact(value) {
  return String(value)
    .replace(/\u001b\[[0-9;]*m/g, "")
    .replace(/(RECEIVER_REGISTRATION_SECRET|PAIRING_TOKEN_PRIVATE_KEY|registration-root\.secret|worker-private\.pem|connection_token|pairing_code)([^\n]*?)(?=[\s,}]|$)/gi, "$1=<redacted>")
    .replace(/v1\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+/g, "<token-redacted>")
    .replace(/\b[A-Z0-9]{4}\b/g, "<code-redacted>");
}

export class UI {
  constructor(options = {}) {
    this.options = options;
    this.capabilities = terminalCapabilities(options);
    if (options.plain || options.noColor) this.capabilities.color = false;
    if (options.plain) { this.capabilities.unicode = false; this.capabilities.cursor = false; }
    this.phaseNumber = 0;
  }
  icon(kind) {
    if (this.capabilities.unicode) return { ok: "✓", fail: "✗", warn: "!", dot: "•" }[kind] || "•";
    return { ok: "PASS", fail: "FAIL", warn: "WARN", dot: "-" }[kind] || "-";
  }
  line(message = "") { (this.options.stdout || defaultOutput).write(`${message}\n`); }
  title(message) { this.line(ansi(this.capabilities.color, "1;36", message)); }
  info(message) { this.line(`${ansi(this.capabilities.color, "36", this.icon("dot"))} ${message}`); }
  success(message) { this.line(`${ansi(this.capabilities.color, "32", this.icon("ok"))} ${message}`); }
  warn(message) { this.line(`${ansi(this.capabilities.color, "33", this.icon("warn"))} ${message}`); }
  fail(message) { this.line(`${ansi(this.capabilities.color, "31", this.icon("fail"))} ${message}`); }
  phase(name, index = ++this.phaseNumber) {
    this.phaseNumber = index;
    this.line(`\n${ansi(this.capabilities.color, "1;36", `[${index}/${PHASES.length}] ${name}`)}`);
  }
  async prompt(question, defaultValue = "") {
    if (!this.capabilities.tty) throw new SetupError("Interactive setup requires a terminal; use --status or --verify for non-interactive checks");
    const rl = createInterface({ input: this.options.stdin || defaultInput, output: this.options.stdout || defaultOutput });
    try {
      const answer = await rl.question(`${question}${defaultValue ? ` [${defaultValue}]` : ""}: `);
      return answer.trim() || defaultValue;
    } finally { rl.close(); }
  }
  async confirm(question, defaultValue = false) {
    const answer = await this.prompt(`${question} [${defaultValue ? "Y/n" : "y/N"}]`);
    return answer ? /^(yes|y)$/i.test(answer) : defaultValue;
  }
  async run(label, operation) {
    const started = Date.now();
    if (!this.capabilities.tty || !this.capabilities.cursor) {
      this.info(`${label}...`);
      try { const result = await operation(); this.success(`${label} (${duration(Date.now() - started)})`); return result; }
      catch (error) { this.fail(`${label}: ${redact(error.message)}`); throw error; }
    }
    const frames = this.capabilities.unicode ? ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"] : [".", "..", "..."];
    let frame = 0;
    const draw = () => (this.options.stdout || defaultOutput).write(`\r${frames[frame++ % frames.length]} ${label}`.padEnd(Math.min(this.capabilities.columns, 100)));
    draw();
    const timer = setInterval(draw, 120);
    try {
      const result = await operation();
      clearInterval(timer);
      (this.options.stdout || defaultOutput).write("\r\x1b[2K");
      this.success(`${label} (${duration(Date.now() - started)})`);
      return result;
    } catch (error) {
      clearInterval(timer);
      (this.options.stdout || defaultOutput).write("\r\x1b[2K");
      this.fail(`${label}: ${redact(error.message)}`);
      throw error;
    }
  }
}

export function createRunner({ env = process.env } = {}) {
  return (command, args = [], options = {}) => new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { cwd: options.cwd, env: { ...env, ...options.env }, stdio: [options.input === undefined ? "ignore" : "pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", chunk => { stdout += chunk.toString(); });
    child.stderr.on("data", chunk => { stderr += chunk.toString(); });
    let timeout;
    if (options.timeoutMs) timeout = setTimeout(() => child.kill("SIGTERM"), options.timeoutMs);
    child.on("error", reject);
    child.on("close", (code, signal) => { if (timeout) clearTimeout(timeout); resolvePromise({ code: code ?? 1, signal, stdout, stderr }); });
    if (options.input !== undefined) child.stdin.end(options.input);
  });
}

function assertSuccess(result, label, options = {}) {
  if (result.code === 0) return result;
  const detail = redact((result.stderr || result.stdout || "").trim().split("\n").slice(-8).join("\n"));
  throw new SetupError(`${label} failed${detail ? `: ${detail}` : ""}`, options);
}
async function exists(file) { try { await access(file, fsConstants.F_OK); return true; } catch { return false; } }
async function sha256File(file) { return createHash("sha256").update(await readFile(file)).digest("hex"); }

async function hashTree(root) {
  const files = [];
  async function walk(dir) {
    for (const entry of (await readdir(dir, { withFileTypes: true })).sort((a, b) => a.name.localeCompare(b.name))) {
      if (entry.name === "node_modules" || entry.name === ".wrangler") continue;
      const full = join(dir, entry.name);
      if (entry.isDirectory()) await walk(full);
      else if (entry.isFile()) files.push([relative(root, full), await readFile(full)]);
    }
  }
  await walk(root);
  const hash = createHash("sha256");
  for (const [name, data] of files) hash.update(name).update("\0").update(data).update("\0");
  return hash.digest("hex");
}

async function loadState() {
  if (!await exists(STATE_FILE)) return { version: 1, checkpoints: {} };
  try {
    const parsed = JSON.parse(await readFile(STATE_FILE, "utf8"));
    if (parsed?.version !== 1 || typeof parsed !== "object") throw new Error("unsupported state version");
    return parsed;
  } catch (error) {
    throw new SetupError(`Cannot read ${STATE_FILE}: ${error.message}`, { recovery: "Inspect or move the corrupt state file aside, then rerun setup." });
  }
}
async function saveState(state) {
  await mkdir(STATE_DIR, { recursive: true, mode: 0o700 });
  await chmod(STATE_DIR, 0o700);
  const temp = `${STATE_FILE}.tmp-${process.pid}-${randomBytes(4).toString("hex")}`;
  await writeFile(temp, `${JSON.stringify({ ...state, updatedAt: new Date().toISOString() }, null, 2)}\n`, { mode: 0o600 });
  await rename(temp, STATE_FILE);
}
function checkpoint(state, phase, status, detail = {}) {
  state.activePhase = phase;
  state.checkpoints = { ...(state.checkpoints || {}), [phase]: { status, at: new Date().toISOString(), ...detail } };
}

function parseToml(text) {
  const value = key => text.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"`, "m"))?.[1] || "";
  return { name: value("name"), databaseName: value("database_name"), databaseId: value("database_id") };
}
function updateToml(text, values) {
  let result = text;
  for (const [key, value] of Object.entries(values)) {
    const line = `${key} = "${value}"`;
    const expression = new RegExp(`^${key}\\s*=.*$`, "m");
    result = expression.test(result) ? result.replace(expression, line) : `${result}\n${line}`;
  }
  return result.endsWith("\n") ? result : `${result}\n`;
}
async function writeToml(values) {
  const base = await exists(WORKER_CONFIG) ? await readFile(WORKER_CONFIG, "utf8") : await readFile(WORKER_CONFIG_EXAMPLE, "utf8");
  const temp = `${WORKER_CONFIG}.tmp-${process.pid}`;
  await writeFile(temp, updateToml(base, values), { mode: 0o600 });
  await rename(temp, WORKER_CONFIG);
}
function parseJson(text) {
  const trimmed = text.trim();
  if (!trimmed) return null;
  try { return JSON.parse(trimmed); } catch {
    const indexes = [trimmed.indexOf("{"), trimmed.indexOf("[")].filter(index => index >= 0);
    if (!indexes.length) return null;
    try { return JSON.parse(trimmed.slice(Math.min(...indexes))); } catch { return null; }
  }
}
function rows(value) {
  if (Array.isArray(value)) return value.flatMap(item => item?.results || item?.result || item);
  return value?.results || value?.result || [];
}

async function wrangler(run, args, options = {}) {
  const local = join(WORKER_DIR, "node_modules", ".bin", "wrangler");
  const command = existsSync(local) ? local : "npx";
  const commandArgs = existsSync(local) ? args : ["--no-install", "wrangler", ...args];
  return run(command, commandArgs, { cwd: WORKER_DIR, ...options });
}
function validateName(value, label, max = 63) {
  if (!new RegExp(`^[A-Za-z0-9_-]{1,${max}}$`).test(value)) throw new SetupError(`${label} contains unsupported characters`);
  return value;
}
function validateDomain(value) {
  if (!/^(?=.{1,253}$)[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?$/.test(value) || value.includes("..")) throw new SetupError("Cloudflare hostname is invalid");
  return value.toLowerCase();
}
function validateBoard(value) {
  if (!/^[A-Za-z0-9_.:-]+$/.test(value)) throw new SetupError("Receiver SSH address contains unsupported characters");
  return value;
}
function envValues(text) {
  return Object.fromEntries(text.split(/\r?\n/).map(line => line.match(/^([A-Z0-9_]+)=(.*)$/)).filter(Boolean).map(match => [match[1], match[2]]));
}
export function deriveRegistrationSecret(root, receiverId) {
  const key = Buffer.from(root.replace(/-/g, "+").replace(/_/g, "/"), "base64");
  if (key.length < 32) throw new SetupError("Registration root secret must contain at least 32 bytes");
  return createHmac("sha256", key).update(`cast-registration-v1:${receiverId}`).digest("base64url");
}
function keyFingerprint(pem) { return createHash("sha256").update(createPublicKey(pem).export({ type: "spki", format: "der" })).digest("hex"); }
export function validateKeyPair(privatePem, publicPem) {
  const derived = createPublicKey(createPrivateKey(privatePem)).export({ type: "spki", format: "der" });
  const supplied = createPublicKey(publicPem).export({ type: "spki", format: "der" });
  if (!derived.equals(supplied)) throw new SetupError("Worker private key and receiver public key do not match");
  return createHash("sha256").update(supplied).digest("hex");
}
function generatedCredentials() {
  const pair = generateKeyPairSync("rsa", { modulusLength: 2048, publicKeyEncoding: { type: "spki", format: "pem" }, privateKeyEncoding: { type: "pkcs8", format: "pem" } });
  return { privatePem: pair.privateKey, publicPem: pair.publicKey, root: randomBytes(32).toString("base64url"), fingerprint: keyFingerprint(pair.publicKey) };
}
async function installCredentials(credentials) {
  const files = [[PRIVATE_KEY_LOCAL, credentials.privatePem], [PUBLIC_KEY_LOCAL, credentials.publicPem], [ROOT_SECRET_LOCAL, `${credentials.root}\n`]];
  const temps = [];
  try {
    for (const [target, contents] of files) {
      const temp = `${target}.next-${process.pid}-${randomBytes(3).toString("hex")}`;
      await writeFile(temp, contents, { mode: 0o600 });
      temps.push([temp, target]);
    }
    for (const [temp, target] of temps) await rename(temp, target);
    await chmod(PUBLIC_KEY_LOCAL, 0o600); await chmod(PRIVATE_KEY_LOCAL, 0o600); await chmod(ROOT_SECRET_LOCAL, 0o600);
  } catch (error) {
    for (const [temp] of temps) await rm(temp, { force: true }).catch(() => {});
    throw error;
  }
}
async function hasCredentialBackup() { return (await Promise.all(CREDENTIAL_BACKUPS.map(([, backup]) => exists(backup)))).some(Boolean); }
async function backupCredentials() {
  for (const [target, backup] of CREDENTIAL_BACKUPS) {
    if (await exists(target)) { await rm(backup, { force: true }); await rename(target, backup); }
  }
}
async function restoreCredentialBackup() {
  for (const [target, backup] of CREDENTIAL_BACKUPS) {
    if (await exists(backup)) { await rm(target, { force: true }); await rename(backup, target); }
  }
}
async function clearCredentialBackup() { for (const [, backup] of CREDENTIAL_BACKUPS) await rm(backup, { force: true }); }
async function credentialsFromDisk() {
  const present = await Promise.all([PUBLIC_KEY_LOCAL, PRIVATE_KEY_LOCAL, ROOT_SECRET_LOCAL].map(exists));
  if (!present.every(Boolean)) return null;
  const [publicPem, privatePem, rootText] = await Promise.all([readFile(PUBLIC_KEY_LOCAL, "utf8"), readFile(PRIVATE_KEY_LOCAL, "utf8"), readFile(ROOT_SECRET_LOCAL, "utf8")]);
  const privateModes = await Promise.all([PRIVATE_KEY_LOCAL, ROOT_SECRET_LOCAL].map(async file => (await stat(file)).mode & 0o777));
  if (privateModes.some(mode => (mode & 0o077) !== 0)) throw new SetupError("Local private Cloudflare credential files are too permissive; expected owner-only permissions");
  const root = rootText.trim();
  if (!/^[A-Za-z0-9_-]+$/.test(root) || Buffer.from(root.replace(/-/g, "+").replace(/_/g, "/"), "base64").length < 32) throw new SetupError("Local registration root secret is invalid");
  return { publicPem, privatePem, root, fingerprint: validateKeyPair(privatePem, publicPem) };
}

async function ensureDependencies(run, ui, state) {
  const lockHash = await sha256File(join(WORKER_DIR, "package-lock.json"));
  if (!(await exists(join(WORKER_DIR, "node_modules"))) || state.packageLockHash !== lockHash) {
    await ui.run("Installing Worker dependencies", async () => assertSuccess(await run("npm", ["ci"], { cwd: WORKER_DIR }), "npm ci", { phase: "preflight" }));
  }
  state.packageLockHash = lockHash;
}
async function checkPrerequisites(run, ui, state) {
  for (const command of ["npm", "ssh", "scp", "curl", "openssl"]) {
    const result = await run("sh", ["-c", `command -v ${command}`]);
    if (result.code !== 0) throw new SetupError(`Required command not found: ${command}`, { phase: "preflight" });
  }
  if (!(await exists(WORKER_DIR)) || !(await exists(WORKER_CONFIG_EXAMPLE))) throw new SetupError("Cloudflare Worker files are incomplete", { phase: "preflight" });
  await ensureDependencies(run, ui, state);
  assertSuccess(await wrangler(run, ["whoami"]), "Wrangler authentication", { phase: "preflight", recovery: "Run npx wrangler login, then rerun setup." });
}

async function resolveDatabase({ run, state, config, dbName, ui, createAllowed }) {
  const before = assertSuccess(await wrangler(run, ["d1", "list", "--json", "--config", WORKER_CONFIG]), "D1 list", { phase: "database" });
  let databaseId = config.databaseId && config.databaseId !== "REPLACE_WITH_D1_DATABASE_ID" ? config.databaseId : state.databaseId;
  const databases = rows(parseJson(before.stdout));
  const matches = databases.filter(db => db?.name === dbName || db?.database_name === dbName);
  if (databaseId && databases.length && !databases.some(db => (db?.uuid || db?.database_id || db?.id) === databaseId && (db?.name || db?.database_name) === dbName)) {
    throw new SetupError(`Configured D1 ID does not belong to database ${dbName}`, { phase: "database", recovery: "Inspect wrangler.toml and choose the matching D1 database before rerunning." });
  }
  if (!databaseId && matches.length === 1) databaseId = matches[0].uuid || matches[0].database_id || matches[0].id;
  if (!databaseId && matches.length > 1) throw new SetupError(`Multiple D1 databases are named ${dbName}; set the intended ID in wrangler.toml`, { phase: "database" });
  if (!databaseId && createAllowed) {
    await ui.run(`Creating remote D1 database ${dbName}`, async () => assertSuccess(await wrangler(run, ["d1", "create", dbName, "--binding", "DB", "--config", WORKER_CONFIG]), "D1 create", { phase: "database" }));
    const after = assertSuccess(await wrangler(run, ["d1", "list", "--json", "--config", WORKER_CONFIG]), "D1 list after create", { phase: "database" });
    const created = rows(parseJson(after.stdout)).filter(db => db?.name === dbName || db?.database_name === dbName);
    if (created.length !== 1) throw new SetupError("D1 was created but its ID could not be resolved from Wrangler JSON", { phase: "database", recovery: "Rerun setup; it will discover the database by name." });
    databaseId = created[0].uuid || created[0].database_id || created[0].id;
  }
  if (!databaseId) throw new SetupError(`No D1 database named ${dbName} is configured`, { phase: "database", recovery: "Rerun and approve creation, or set database_id in wrangler.toml." });
  if (!/^[A-Za-z0-9-]+$/.test(databaseId)) throw new SetupError("D1 database ID is invalid", { phase: "database" });
  await writeToml({ name: config.name || state.config.workerName, database_name: dbName, database_id: databaseId });
  state.databaseId = databaseId; state.dbName = dbName;
  return databaseId;
}
async function applyMigrations({ run, ui, dbName }) {
  await ui.run("Applying remote D1 migrations", async () => assertSuccess(await wrangler(run, ["d1", "migrations", "apply", dbName, "--remote", "--config", WORKER_CONFIG]), "D1 migrations", { phase: "database" }));
  const check = assertSuccess(await wrangler(run, ["d1", "execute", dbName, "--remote", "--json", "--config", WORKER_CONFIG, "--command", "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('active_receivers','registration_replays','rate_limits')"]), "D1 schema verification", { phase: "database" });
  if (rows(parseJson(check.stdout)).length < 3) throw new SetupError("D1 migration verification found an incomplete schema", { phase: "database" });
}

async function remoteKeyFingerprint(run, boardIp) {
  const remoteCommands = ["openssl pkey -pubin -in /var/lib/llrdc-pairing/public.pem -outform DER 2>/dev/null | sha256sum", "sudo openssl pkey -pubin -in /var/lib/llrdc-pairing/public.pem -outform DER 2>/dev/null | sha256sum"];
  for (const command of remoteCommands.map(value => ["-o", "BatchMode=yes", boardIp, value])) {
    const result = await run("ssh", command);
    if (result.code === 0) return result.stdout.trim().split(/\s+/)[0];
  }
  return "";
}
async function uploadReceiverKey({ run, ui, boardIp }) {
  const local = keyFingerprint(await readFile(PUBLIC_KEY_LOCAL, "utf8"));
  if (await remoteKeyFingerprint(run, boardIp) === local) {
    ui.success("Receiver public key is already installed and matches");
    return;
  }
  const writable = await run("ssh", ["-o", "BatchMode=yes", boardIp, "test -d /var/lib/llrdc-pairing && test -w /var/lib/llrdc-pairing"]);
  if (writable.code === 0) {
    await ui.run("Installing receiver public key", async () => assertSuccess(await run("scp", ["-o", "BatchMode=yes", PUBLIC_KEY_LOCAL, `${boardIp}:/var/lib/llrdc-pairing/public.pem`]), "receiver key upload", { phase: "receiver-key" }));
  } else {
    if (!ui.capabilities.tty || !(await ui.confirm("Receiver pairing directory requires sudo. Install only public.pem with sudo", true))) throw new SetupError("Receiver public-key installation was not approved", { phase: "receiver-key" });
    const remote = `/tmp/llrdc-receiver-public-${process.pid}-${randomBytes(3).toString("hex")}.pem`;
    try {
      await ui.run("Copying receiver public key", async () => assertSuccess(await run("scp", ["-o", "BatchMode=yes", PUBLIC_KEY_LOCAL, `${boardIp}:${remote}`]), "receiver key staging", { phase: "receiver-key" }));
      await ui.run("Installing receiver public key with sudo", async () => assertSuccess(await run("ssh", ["-tt", "-o", "BatchMode=yes", boardIp, `sudo test -d /var/lib/llrdc-pairing && sudo install -m 644 '${remote}' /var/lib/llrdc-pairing/public.pem && rm -f '${remote}'`]), "receiver sudo key install", { phase: "receiver-key" }));
    } finally { await run("ssh", ["-o", "BatchMode=yes", boardIp, `rm -f '${remote}'`]).catch(() => {}); }
  }
  const actual = await remoteKeyFingerprint(run, boardIp);
  if (actual !== local) throw new SetupError("Receiver public-key fingerprint does not match the local key", { phase: "receiver-key", recovery: "Rerun setup to repair /var/lib/llrdc-pairing/public.pem." });
}
async function writeReceiverEnv({ domain, receiverId, registrationSecret }) {
  const text = ["# Generated by setup_cloudflare.sh. Do not commit this file.", "SERVER_CLOUD_DISCOVERY_ENABLED=1", `SERVER_PAIRING_WORKER_URL=https://${domain}`, `SERVER_RECEIVER_ID=${receiverId}`, `SERVER_RECEIVER_REGISTRATION_SECRET=${registrationSecret}`, "SERVER_PAIRING_TOKEN_PUBLIC_KEY_FILE=/pairing/public.pem", ""].join("\n");
  const temp = `${RECEIVER_ENV}.tmp-${process.pid}`;
  await writeFile(temp, text, { mode: 0o600 }); await rename(temp, RECEIVER_ENV); await chmod(RECEIVER_ENV, 0o600);
}
async function workerSecrets(run) {
  const result = await wrangler(run, ["secret", "list", "--format", "json", "--config", WORKER_CONFIG]);
  if (result.code !== 0) return [];
  return rows(parseJson(result.stdout)).map(item => item?.name || item?.key).filter(Boolean);
}
async function uploadWorkerSecrets({ run, ui, credentials }) {
  await ui.run("Uploading registration secret", async () => assertSuccess(await wrangler(run, ["secret", "put", "RECEIVER_REGISTRATION_SECRET", "--config", WORKER_CONFIG], { input: `${credentials.root}\n` }), "registration secret upload", { phase: "worker" }));
  await ui.run("Uploading token signing key", async () => assertSuccess(await wrangler(run, ["secret", "put", "PAIRING_TOKEN_PRIVATE_KEY", "--config", WORKER_CONFIG], { input: credentials.privatePem }), "token key upload", { phase: "worker" }));
}
async function inspectContainer(run, boardIp, values) {
  const result = await run("ssh", ["-o", "BatchMode=yes", boardIp, "docker inspect -f '{{.State.Running}} {{range .Config.Env}}{{println .}}{{end}}' llrdc-casting"]);
  if (result.code !== 0) return { healthy: false, reason: "container is not available" };
  const lines = result.stdout.trim().split(/\r?\n/); const running = lines.shift() === "true";
  const env = Object.fromEntries(lines.map(line => line.split(/=(.*)/s)).filter(pair => pair.length === 2));
  const expected = { CLOUD_DISCOVERY_ENABLED: "1", PAIRING_WORKER_URL: `https://${values.domain}`, RECEIVER_ID: values.receiverId, PAIRING_TOKEN_PUBLIC_KEY_FILE: "/pairing/public.pem" };
  const healthy = running && Object.entries(expected).every(([key, value]) => env[key] === value) && env.RECEIVER_REGISTRATION_SECRET === values.registrationSecret;
  return { healthy, reason: healthy ? "configured and running" : "configuration differs" };
}

async function publicEndpointHealthy(run, domain) { return (await run("curl", ["-fsSI", "--max-time", "10", `https://${domain}/`])).code === 0; }
async function verifyPublicEndpoint({ run, ui, domain }) {
  const head = await run("curl", ["-fsSI", "--max-time", "20", `https://${domain}/`]);
  if (head.code !== 0) throw new SetupError(`https://${domain}/ is not reachable`, { phase: "verification", recovery: "Check DNS/custom-domain propagation and rerun --verify." });
  const body = await run("curl", ["-fsS", "--max-time", "20", `https://${domain}/`]);
  if (body.code !== 0 || !/LLrdc Pairing|pairing code/i.test(body.stdout)) throw new SetupError("Cloudflare root endpoint did not return the pairing bootstrap", { phase: "verification" });
  const invalid = await run("curl", ["-sS", "-o", "/dev/null", "-w", "%{http_code}", "--max-time", "20", "-X", "POST", `https://${domain}/api/pair`, "-H", "content-type: application/json", "--data", '{"code":"0000"}']);
  if (invalid.code !== 0 || !/^(400|429)$/.test(invalid.stdout.trim())) throw new SetupError(`Invalid pairing smoke test returned HTTP ${invalid.stdout.trim() || "no response"}`, { phase: "verification" });
  ui.success(`Public bootstrap and invalid-code protection verified for ${domain}`);
}
async function verifyRegistration({ run, dbName, receiverId }) {
  const sql = `SELECT receiver_id FROM active_receivers WHERE receiver_id='${receiverId}' AND pairing_code IS NOT NULL AND code_expires_at > unixepoch() AND registration_expires_at > unixepoch() LIMIT 1`;
  const result = assertSuccess(await wrangler(run, ["d1", "execute", dbName, "--remote", "--json", "--config", WORKER_CONFIG, "--command", sql]), "active receiver verification", { phase: "verification" });
  return rows(parseJson(result.stdout)).some(row => row?.receiver_id === receiverId);
}

async function collectConfig(state, ui) {
  const existingConfig = await exists(WORKER_CONFIG) ? parseToml(await readFile(WORKER_CONFIG, "utf8")) : {};
  const existingEnv = await exists(RECEIVER_ENV) ? envValues(await readFile(RECEIVER_ENV, "utf8")) : {};
  const yamlIp = await exists(join(SCRIPT_DIR, "config.yaml")) ? (await readFile(join(SCRIPT_DIR, "config.yaml"), "utf8")).match(/^\s*ip:\s*["']([^"']+)["']/m)?.[1] : "";
  const previous = state.config || {};
  const ask = (question, fallback, validator) => ui.prompt(question, fallback).then(validator);
  return {
    domain: await ask("Cloudflare hostname", previous.domain || existingEnv.SERVER_PAIRING_WORKER_URL?.replace(/^https?:\/\//, "") || DEFAULTS.domain, validateDomain),
    workerName: await ask("Worker name", previous.workerName || existingConfig.name || DEFAULTS.workerName, value => validateName(value, "Worker name")),
    dbName: await ask("D1 database name", previous.dbName || existingConfig.databaseName || DEFAULTS.dbName, value => validateName(value, "D1 database name")),
    receiverId: await ask("Receiver ID", previous.receiverId || existingEnv.SERVER_RECEIVER_ID || DEFAULTS.receiverId, value => validateName(value, "Receiver ID", 128)),
    boardIp: await ask("Receiver SSH address", previous.boardIp || yamlIp || DEFAULTS.boardIp, validateBoard),
  };
}

async function reconcile({ run, ui }) {
  const state = await loadState();
  let currentPhase = "preflight";
  const persist = () => saveState(state);
  const interrupted = async signal => {
    state.activePhase = currentPhase;
    state.interrupted = { signal, at: new Date().toISOString() };
    await persist().catch(() => {});
    ui.warn(`Setup interrupted during ${currentPhase}; state was preserved. Rerun ./setup_cloudflare.sh to resume.`);
    process.exit(130);
  };
  process.once("SIGINT", () => { void interrupted("SIGINT"); }); process.once("SIGTERM", () => { void interrupted("SIGTERM"); });
  let previousCredentials = null;
  let rotatedCredentials = false;
  try {
    ui.title("LLrdc Cloudflare pairing setup");
    ui.info("Cloudflare carries discovery only; media and WebTransport remain on the receiver LAN.");
    for (let index = 0; index < PHASES.length; index += 1) {
      currentPhase = PHASES[index]; ui.phase(currentPhase, index + 1); checkpoint(state, currentPhase, "running"); await persist();
      if (currentPhase === "preflight") await checkPrerequisites(run, ui, state);
      else if (currentPhase === "configuration") {
        state.config = await collectConfig(state, ui);
        ui.info(`Cloudflare hostname: ${state.config.domain}`);
        ui.info(`Worker/database: ${state.config.workerName} / ${state.config.dbName}`);
        ui.info(`Receiver: ${state.config.receiverId} at ${state.config.boardIp}`);
        if (!(await ui.confirm("Apply this configuration and reconcile remote state", true))) throw new SetupError("Setup was cancelled", { phase: "configuration", recovery: "Rerun setup when ready to apply the displayed configuration." });
        const existing = await exists(WORKER_CONFIG) ? parseToml(await readFile(WORKER_CONFIG, "utf8")) : {};
        const values = { name: state.config.workerName, database_name: state.config.dbName };
        if (existing.databaseName && existing.databaseName !== state.config.dbName) { values.database_id = "REPLACE_WITH_D1_DATABASE_ID"; state.databaseId = null; }
        await writeToml(values);
      }
      else if (currentPhase === "database") {
        const config = parseToml(await readFile(WORKER_CONFIG, "utf8"));
        const hasId = Boolean((config.databaseId && config.databaseId !== "REPLACE_WITH_D1_DATABASE_ID") || state.databaseId);
        await resolveDatabase({ run, state, config, dbName: state.config.dbName, ui, createAllowed: hasId || await ui.confirm(`Create or reuse D1 database ${state.config.dbName}`, true) });
        await applyMigrations({ run, ui, dbName: state.config.dbName });
      } else if (currentPhase === "credentials") {
        if (await hasCredentialBackup()) {
          ui.warn("An earlier credential rotation was interrupted; restoring the previous local credential set before inspection.");
          await restoreCredentialBackup();
          state.rotationInProgress = false;
          await persist();
        }
        let credentials = await credentialsFromDisk();
        if (!credentials) { ui.warn("Local credentials are missing or invalid; generating replacements transactionally."); credentials = generatedCredentials(); await installCredentials(credentials); }
        else if (ui.options.rotate) {
          if (!(await ui.confirm("Generate new credentials and invalidate the current Worker credentials", false))) throw new SetupError("Credential rotation was not approved", { phase: "credentials" });
          previousCredentials = credentials;
          await backupCredentials();
          state.rotationInProgress = true;
          await persist();
          credentials = generatedCredentials();
          await installCredentials(credentials);
          rotatedCredentials = true;
        }
        state.keyFingerprint = credentials.fingerprint; state.registrationSecretFingerprint = createHash("sha256").update(credentials.root).digest("hex"); state.credentialsReady = true;
      } else if (currentPhase === "receiver-key") await uploadReceiverKey({ run, ui, boardIp: state.config.boardIp });
      else if (currentPhase === "worker") {
        const credentials = await credentialsFromDisk(); const secrets = await workerSecrets(run);
        if (!secrets.includes("RECEIVER_REGISTRATION_SECRET") || !secrets.includes("PAIRING_TOKEN_PRIVATE_KEY") || state.keyFingerprint !== state.lastUploadedKeyFingerprint) {
          if (state.lastUploadedKeyFingerprint && !ui.options.rotate && !(await ui.confirm("Worker credentials differ from the saved deployment; rotate Worker secrets", false))) throw new SetupError("Worker credential repair was not approved", { phase: "worker" });
          await uploadWorkerSecrets({ run, ui, credentials }); state.lastUploadedKeyFingerprint = state.keyFingerprint;
        } else ui.success("Worker secrets are present and unchanged");
        const registrationSecret = deriveRegistrationSecret(credentials.root, state.config.receiverId);
        await writeReceiverEnv({ domain: state.config.domain, receiverId: state.config.receiverId, registrationSecret });
        const fingerprint = await hashTree(WORKER_DIR);
        if (state.deployFingerprint !== fingerprint || !(await publicEndpointHealthy(run, state.config.domain))) {
          await ui.run(`Checking and deploying Worker at ${state.config.domain}`, async () => { assertSuccess(await run("npm", ["run", "check"], { cwd: WORKER_DIR }), "Worker type-check", { phase: "worker" }); assertSuccess(await wrangler(run, ["deploy", "--config", WORKER_CONFIG, "--domains", state.config.domain]), "Worker deploy", { phase: "worker" }); });
          state.deployFingerprint = fingerprint;
        } else ui.success("Worker source and public endpoint are unchanged");
      } else if (currentPhase === "receiver") {
        const credentials = await credentialsFromDisk(); const registrationSecret = deriveRegistrationSecret(credentials.root, state.config.receiverId);
        const inspected = await inspectContainer(run, state.config.boardIp, { ...state.config, registrationSecret });
        if (!inspected.healthy) await ui.run("Starting receiver with Cloud discovery enabled", async () => assertSuccess(await run(join(SCRIPT_DIR, "server.sh"), ["--start", `--board-ip=${state.config.boardIp}`, "--cloud=true"]), "receiver start", { phase: "receiver" })); else ui.success(`Receiver container is ${inspected.reason}`);
      } else if (currentPhase === "verification") {
        await verifyPublicEndpoint({ run, ui, domain: state.config.domain });
        if (!await verifyRegistration({ run, dbName: state.config.dbName, receiverId: state.config.receiverId })) throw new SetupError("Receiver registration is not active in D1", { phase: "verification", recovery: "Wait for registration retry, then rerun ./setup_cloudflare.sh --verify." });
        const credentials = await credentialsFromDisk(); if (await remoteKeyFingerprint(run, state.config.boardIp) !== credentials.fingerprint) throw new SetupError("Receiver key verification failed", { phase: "verification" });
        const logs = await run("ssh", ["-o", "BatchMode=yes", state.config.boardIp, "docker logs --since 120s llrdc-casting 2>&1"]);
        if (!logs.stdout.includes("[CLOUD DISCOVERY] Receiver registration succeeded")) throw new SetupError("Receiver has not reported successful Cloud discovery registration", { phase: "verification", recovery: "Check receiver logs and rerun --verify after registration retries." });
      }
      checkpoint(state, currentPhase, "complete"); await persist();
    }
    state.activePhase = null; state.interrupted = null; await persist();
    await clearCredentialBackup();
    state.rotationInProgress = false;
    await persist();
    ui.title("Setup verified");
    ui.line("Verification summary:");
    ui.success("D1 database and migrations");
    ui.success("Worker secrets and deployment");
    ui.success("Receiver public-key fingerprint");
    ui.success("Receiver registration in D1");
    ui.success("Public bootstrap and invalid-code response");
    ui.success(`Cloud URL: https://${state.config.domain}/`);
    ui.success(`Receiver: ${state.config.boardIp} (${state.config.receiverId})`);
    ui.info("Full browser-to-LAN HEVC regression: skipped (run ./test_browser.sh cloud when desired).");
  } catch (error) {
    if (rotatedCredentials && previousCredentials) await restoreCredentialBackup().catch(async () => { await installCredentials(previousCredentials).catch(() => {}); });
    state.rotationInProgress = false;
    state.activePhase = currentPhase; state.lastError = { message: redact(error.message), at: new Date().toISOString() }; await persist().catch(() => {});
    throw error instanceof SetupError ? error : new SetupError(error.message, { phase: currentPhase, cause: error });
  }
}

async function statusReport(run) {
  const state = await loadState();
  const diskEnv = await exists(RECEIVER_ENV) ? envValues(await readFile(RECEIVER_ENV, "utf8")) : {};
  const config = state.config || { ...(await exists(WORKER_CONFIG) ? parseToml(await readFile(WORKER_CONFIG, "utf8")) : {}), domain: diskEnv.SERVER_PAIRING_WORKER_URL?.replace(/^https?:\/\//, ""), receiverId: diskEnv.SERVER_RECEIVER_ID };
  const results = [];
  const add = (name, status, detail = "") => results.push({ name, status, detail });
  add("local state", await exists(STATE_FILE) ? "ok" : "missing", STATE_FILE);
  add("Worker config", await exists(WORKER_CONFIG) ? "ok" : "missing", WORKER_CONFIG);
  add("local credentials", await credentialsFromDisk().then(value => value ? "ok" : "missing").catch(() => "invalid"), STATE_DIR);
  if (config.dbName || config.databaseName) {
    const dbName = config.dbName || config.databaseName;
    const list = await wrangler(run, ["d1", "list", "--json", "--config", WORKER_CONFIG]);
    const dbs = list.code === 0 ? rows(parseJson(list.stdout)) : [];
    add("D1 database", list.code !== 0 ? "unavailable" : dbs.some(db => (db.name || db.database_name) === dbName) ? "ok" : "missing", dbName);
    const secretResult = await wrangler(run, ["secret", "list", "--format", "json", "--config", WORKER_CONFIG]);
    const secrets = secretResult.code === 0 ? rows(parseJson(secretResult.stdout)).map(item => item?.name || item?.key).filter(Boolean) : [];
    add("Worker secrets", secretResult.code !== 0 ? "unavailable" : secrets.includes("RECEIVER_REGISTRATION_SECRET") && secrets.includes("PAIRING_TOKEN_PRIVATE_KEY") ? "ok" : "missing");
  }
  if (config.boardIp) { const ssh = await run("ssh", ["-o", "BatchMode=yes", config.boardIp, "true"]); add("receiver SSH", ssh.code === 0 ? "ok" : "unavailable", config.boardIp); }
  return { version: 1, state, config, results };
}
function printReport(report, ui) {
  ui.title("LLrdc Cloudflare setup status");
  for (const item of report.results) {
    const marker = item.status === "ok" ? ui.icon("ok") : item.status === "missing" || item.status === "unavailable" ? ui.icon("warn") : ui.icon("fail");
    ui.line(`${marker} ${item.name.padEnd(22)} ${item.status}${item.detail ? ` — ${item.detail}` : ""}`);
  }
}
async function verifyOnly({ run, ui, args }) {
  const report = await statusReport(run);
  if (args.json) {
    let verified = null;
    if (args.mode === "verify") {
      const diagnosticUi = new UI({ plain: true, stdin: ui.options.stdin || defaultInput, stdout: defaultError });
      const config = report.config; const domain = config.domain || DEFAULTS.domain; const dbName = config.dbName || config.databaseName; const receiverId = config.receiverId;
      await verifyPublicEndpoint({ run, ui: diagnosticUi, domain });
      if (!dbName || !receiverId || !await verifyRegistration({ run, dbName, receiverId })) throw new SetupError("Verification did not find an active receiver registration", { phase: "verification" });
      verified = { publicEndpoint: true, activeRegistration: true };
    }
    (ui.options.stdout || defaultOutput).write(`${JSON.stringify({ ...report, verification: verified })}\n`);
    return report.results.every(item => item.status === "ok") ? 0 : 1;
  }
  printReport(report, ui);
  if (args.mode === "verify") {
    const config = report.config; const domain = config.domain || DEFAULTS.domain; const dbName = config.dbName || config.databaseName; const receiverId = config.receiverId;
    await verifyPublicEndpoint({ run, ui, domain });
    if (!dbName || !receiverId || !await verifyRegistration({ run, dbName, receiverId })) throw new SetupError("Verification did not find an active receiver registration", { phase: "verification" });
  }
  return report.results.every(item => item.status === "ok") ? 0 : 1;
}
function printHelp(ui) {
  ui.title("LLrdc Cloudflare setup");
  ui.line("Usage: ./setup_cloudflare.sh [--status|--verify] [--json] [--plain|--no-color]");
  ui.line(""); ui.line("  (no option)           Inspect, confirm, reconcile, and verify setup"); ui.line("  --status              Read-only local and remote status report"); ui.line("  --verify              Run verification checks and return a meaningful exit code"); ui.line("  --rotate-credentials  Replace credentials after an explicit confirmation"); ui.line("  --json                Machine-readable output for --status/--verify"); ui.line("  --plain               Disable color, cursor control, and Unicode symbols"); ui.line("  --no-color            Disable ANSI colors while retaining terminal layout");
}

export async function main(argv = process.argv.slice(2), dependencies = {}) {
  const args = parseArgs(argv);
  const ui = dependencies.ui || new UI({ ...args, stdin: dependencies.stdin || defaultInput, stdout: dependencies.stdout || defaultOutput, json: args.json });
  if (args.mode === "help") { printHelp(ui); return 0; }
  const run = dependencies.run || createRunner();
  if (args.mode === "status" || args.mode === "verify") return verifyOnly({ run, ui, args });
  if (!ui.capabilities.tty) throw new SetupError("Interactive setup requires a terminal; use --status/--verify for non-interactive checks");
  await reconcile({ run, ui }); return 0;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().then(code => { process.exitCode = code; }).catch(error => {
    process.stderr.write(`ERROR: ${redact(error.message)}\n`);
    if (error.phase) process.stderr.write(`Failed phase: ${error.phase}\n`);
    if (error.recovery) process.stderr.write(`Recovery: ${error.recovery}\n`);
    process.exitCode = 1;
  });
}
