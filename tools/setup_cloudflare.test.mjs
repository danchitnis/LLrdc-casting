import test from "node:test";
import assert from "node:assert/strict";
import { generateKeyPairSync } from "node:crypto";
import { deriveRegistrationSecret, main, parseArgs, terminalCapabilities, validateKeyPair } from "./setup_cloudflare.mjs";

test("parses status and presentation flags", () => {
  assert.deepEqual(parseArgs(["--status", "--json", "--plain"]), { mode: "status", json: true, plain: true, noColor: false, rotate: false });
});

test("rejects JSON reconciliation mode", () => {
  assert.throws(() => parseArgs(["--json"]), /--json is supported/);
});

test("uses safe defaults for redirected output", () => {
  const capabilities = terminalCapabilities({ stdin: { isTTY: false }, stdout: { isTTY: false, columns: 10 }, env: {} });
  assert.equal(capabilities.tty, false); assert.equal(capabilities.color, false); assert.equal(capabilities.columns, 80);
});

test("respects NO_COLOR and dumb terminals", () => {
  const capabilities = terminalCapabilities({ stdin: { isTTY: true }, stdout: { isTTY: true, columns: 120 }, env: { NO_COLOR: "1", TERM: "dumb", LANG: "C.UTF-8" } });
  assert.equal(capabilities.color, false); assert.equal(capabilities.unicode, true); assert.equal(capabilities.cursor, false);
});

test("derives a stable receiver-specific registration credential", () => {
  const root = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
  const first = deriveRegistrationSecret(root, "receiver-01");
  assert.equal(first, deriveRegistrationSecret(root, "receiver-01"));
  assert.notEqual(first, deriveRegistrationSecret(root, "receiver-02"));
  assert.match(first, /^[A-Za-z0-9_-]+$/);
});

test("validates matching RSA credentials and rejects mismatches", () => {
  const first = generateKeyPairSync("rsa", { modulusLength: 1024, publicKeyEncoding: { type: "spki", format: "pem" }, privateKeyEncoding: { type: "pkcs8", format: "pem" } });
  const second = generateKeyPairSync("rsa", { modulusLength: 1024, publicKeyEncoding: { type: "spki", format: "pem" }, privateKeyEncoding: { type: "pkcs8", format: "pem" } });
  assert.match(validateKeyPair(first.privateKey, first.publicKey), /^[a-f0-9]{64}$/);
  assert.throws(() => validateKeyPair(first.privateKey, second.publicKey), /do not match/);
});

test("status JSON stays machine-readable with a fake remote runner", async () => {
  let output = "";
  const fakeRun = async (_command, args) => {
    if (args.includes("secret") && args.includes("list")) return { code: 0, stdout: JSON.stringify([{ name: "RECEIVER_REGISTRATION_SECRET" }, { name: "PAIRING_TOKEN_PRIVATE_KEY" }]), stderr: "" };
    if (args.includes("d1") && args.includes("list")) return { code: 0, stdout: JSON.stringify([{ name: "cast-pairing", uuid: "fixture-id" }]), stderr: "" };
    if (args.includes("ssh")) return { code: 0, stdout: "", stderr: "" };
    return { code: 0, stdout: "", stderr: "" };
  };
  const code = await main(["--status", "--json", "--plain"], {
    run: fakeRun,
    stdin: { isTTY: false },
    stdout: { isTTY: false, columns: 80, write: chunk => { output += chunk; } },
  });
  assert.equal(code, 1); // Existing workspace state is intentionally incomplete.
  const report = JSON.parse(output);
  assert.equal(report.version, 1);
  assert.ok(Array.isArray(report.results));
});
