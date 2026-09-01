import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { BrowserContext, Page, TestInfo } from '@playwright/test';

const POLL_INTERVAL_MS = 250;
const DEFAULT_WAIT_MS = 30_000;

export type WireCodec = 'hevc' | 'h264';
export type SenderCodec = 'H265' | 'H264' | 'H264_SW';

export interface CodecCase {
  readonly name: string;
  readonly senderCodec: SenderCodec;
  readonly wireCodec: WireCodec;
  readonly resolution: '1920x1080' | '3840x2160';
  readonly cycles: number;
}

export interface BrowserDiagnostics {
  readonly consoleMessages: string[];
  readonly consoleErrors: string[];
  readonly pageErrors: string[];
  readonly requestFailures: string[];
  readonly optionalThirdPartyFailures: string[];
}

export interface ReceiverLogReader {
  read(): string;
  waitFor(predicate: (log: string) => boolean, description: string, timeoutMs?: number): Promise<string>;
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing ${name}; invoke this suite through ./test_browser.sh`);
  return value;
}

export const boardIp = (): string => requiredEnv('E2E_BOARD_IP');
export const adminIp = (): string => process.env.E2E_ADMIN_IP || boardIp();
export const pairingCode = (): string => requiredEnv('E2E_PAIRING_CODE');
export const artifactDir = (): string => process.env.E2E_ARTIFACT_DIR || '../.artefact/manual';

export function redact(value: string): string {
  const code = process.env.E2E_PAIRING_CODE;
  let safe = value;
  if (code) safe = safe.split(code).join('[REDACTED-CODE]');
  return safe
    .replace(/([?&]token=)[^&\s]+/gi, '$1[REDACTED]')
    .replace(/(connection_token["']?\s*[:=]\s*["']?)[A-Za-z0-9._-]+/gi, '$1[REDACTED]')
    .replace(/(authorization\s*[:=]\s*["']?bearer\s+)[^"'\s]+/gi, '$1[REDACTED]');
}

export function attachDiagnostics(page: Page): BrowserDiagnostics {
  const diagnostics: BrowserDiagnostics = {
    consoleMessages: [],
    consoleErrors: [],
    pageErrors: [],
    requestFailures: [],
    optionalThirdPartyFailures: [],
  };
  const removeOptionalBeaconConsoleError = (): void => {
    const generic = '[error] Failed to load resource: net::ERR_CONNECTION_REFUSED';
    const index = diagnostics.consoleErrors.indexOf(generic);
    if (index >= 0) diagnostics.consoleErrors.splice(index, 1);
    const messageIndex = diagnostics.consoleMessages.indexOf(generic);
    if (messageIndex >= 0) diagnostics.consoleMessages.splice(messageIndex, 1);
  };
  page.on('console', message => {
    const text = redact(`[${message.type()}] ${message.text()}`);
    diagnostics.consoleMessages.push(text);
    if (message.type() === 'error') diagnostics.consoleErrors.push(text);
  });
  page.on('pageerror', error => diagnostics.pageErrors.push(redact(error.message)));
  page.on('requestfailed', request => {
    const url = request.url();
    if (url.startsWith('https://static.cloudflareinsights.com/')) {
      // Cloudflare injects this optional analytics script on the public page.
      // Restricted/offline test networks may refuse it; it is unrelated to
      // pairing, LAN handoff, or receiver streaming.
      diagnostics.optionalThirdPartyFailures.push(redact(`${request.method()} ${url}: ${request.failure()?.errorText || 'unknown failure'}`));
      removeOptionalBeaconConsoleError();
      return;
    }
    diagnostics.requestFailures.push(redact(`${request.method()} ${url}: ${request.failure()?.errorText || 'unknown failure'}`));
  });
  return diagnostics;
}

const diagnosticsByPage = new WeakMap<Page, BrowserDiagnostics>();

export function trackDiagnostics(page: Page): BrowserDiagnostics {
  const diagnostics = attachDiagnostics(page);
  diagnosticsByPage.set(page, diagnostics);
  return diagnostics;
}

export function diagnosticsFor(page: Page): BrowserDiagnostics | undefined {
  return diagnosticsByPage.get(page);
}

export function writeDiagnostics(diagnostics: BrowserDiagnostics, name: string): void {
  const dir = artifactDir();
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, `${name}.json`), JSON.stringify(diagnostics, null, 2));
}

export function writeJsonArtifact(name: string, value: unknown): void {
  const dir = artifactDir();
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, name), `${JSON.stringify(value, null, 2)}\n`);
}

export function receiverSystemInfo(): { kernel: string; architecture: string } {
  const output = execFileSync(
    'ssh',
    ['-o', 'BatchMode=yes', boardIp(), 'printf "%s\\n%s\\n" "$(uname -sr)" "$(uname -m)"'],
    { encoding: 'utf8', timeout: 10_000 },
  ).trim().split('\n');
  return {
    kernel: output[0] || 'unknown',
    architecture: output[1] || 'unknown',
  };
}

export async function saveUiScreenshot(page: Page, name: string): Promise<void> {
  const dir = artifactDir();
  mkdirSync(dir, { recursive: true });
  await page.screenshot({
    path: join(dir, name),
    fullPage: true,
    mask: [page.locator('#pairCode, #pair-code')],
  });
}

export function assertDiagnosticsClean(diagnostics: BrowserDiagnostics): void {
  const failures = [
    ...diagnostics.consoleErrors,
    ...diagnostics.pageErrors.map(value => `[pageerror] ${value}`),
    ...diagnostics.requestFailures.map(value => `[requestfailed] ${value}`),
  ].filter((failure, index, all) => {
    if (failure !== '[error] Failed to load resource: net::ERR_CONNECTION_REFUSED') return true;
    const optionalAllowance = diagnostics.optionalThirdPartyFailures.length;
    return index >= all.findIndex(candidate => candidate === failure) + optionalAllowance;
  });
  if (failures.length > 0) throw new Error(`Browser diagnostics reported failures:\n${failures.join('\n')}`);
}

export function receiverLogs(): ReceiverLogReader {
  const read = (): string => {
    try {
      return redact(execFileSync(
        'ssh',
        ['-o', 'BatchMode=yes', boardIp(), 'docker logs --timestamps llrdc-casting 2>&1'],
        { encoding: 'utf8', timeout: 10_000 },
      ));
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      throw new Error(`Unable to read receiver logs: ${message}`);
    }
  };
  return {
    read,
    async waitFor(predicate, description, timeoutMs = DEFAULT_WAIT_MS): Promise<string> {
      const deadline = Date.now() + timeoutMs;
      let latest = '';
      while (Date.now() < deadline) {
        latest = read();
        if (predicate(latest)) return latest;
        await new Promise(resolve => setTimeout(resolve, POLL_INTERVAL_MS));
      }
      throw new Error(`Timed out waiting for receiver log: ${description}\n${latest.slice(-4000)}`);
    },
  };
}

export async function waitFor<T>(read: () => T | Promise<T>, predicate: (value: T) => boolean, description: string, timeoutMs = DEFAULT_WAIT_MS): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  let latest = await read();
  while (Date.now() < deadline) {
    latest = await read();
    if (predicate(latest)) return latest;
    await new Promise(resolve => setTimeout(resolve, POLL_INTERVAL_MS));
  }
  throw new Error(`Timed out waiting for ${description}`);
}

export async function pairThroughUi(page: Page, code: string): Promise<void> {
  const codeInput = page.locator('#pairCode, #pair-code').first();
  const pairButton = page.locator('#pairBtn, #pair-button').first();
  await codeInput.waitFor({ state: 'visible' });
  await codeInput.fill(code);
  await pairButton.click();
  await page.locator('#toggleBtn').waitFor({ state: 'visible', timeout: 60_000 });
  await waitFor(
    () => page.locator('#statusBadge').textContent(),
    value => value?.trim() === 'CONNECTED',
    'CONNECTED status after pairing',
    60_000,
  );
  await waitFor(
    () => page.locator('#pairStatus').textContent(),
    value => value?.trim() === 'PAIRED',
    'PAIRED status after pairing',
    30_000,
  );
  // Do not retain the live pairing code in post-pair traces or DOM snapshots.
  await codeInput.fill('');
}

export async function assertCodecSupport(page: Page, senderCodec: SenderCodec, resolution = '1920x1080'): Promise<void> {
  const [requestedWidth, requestedHeight] = resolution.split('x').map(value => Number.parseInt(value, 10));
  const encodedHeight = Math.ceil(requestedHeight / 16) * 16;
  const supported = await page.evaluate(async ({ codec, width, height, bitrate }) => {
    if (typeof VideoEncoder === 'undefined' || typeof VideoEncoder.isConfigSupported !== 'function') return false;
    const config: VideoEncoderConfig = codec === 'H265'
      ? { codec: 'hev1.1.6.L150.B0', width, height, bitrate, framerate: 30, hardwareAcceleration: 'prefer-hardware' }
      : { codec: 'avc1.42e028', width, height, bitrate: 8_000_000, framerate: 30, hardwareAcceleration: codec === 'H264_SW' ? 'prefer-software' : 'prefer-hardware' };
    try {
      return !!(await VideoEncoder.isConfigSupported(config)).supported;
    } catch {
      return false;
    }
  }, {
    codec: senderCodec,
    width: requestedWidth,
    height: encodedHeight,
    bitrate: requestedWidth >= 3840 ? 15_000_000 : 6_000_000,
  });
  if (!supported) throw new Error(`${senderCodec} WebCodecs configuration is unsupported by the installed Chrome`);
}

export async function chooseCodec(page: Page, senderCodec: SenderCodec, resolution: string): Promise<void> {
  const codec = page.locator('#codec');
  const resolutionSelect = page.locator('#resolution');
  await assertCodecSupport(page, senderCodec, resolution);
  await expectEnabled(page, `#codec option[value="${senderCodec}"]`, `${senderCodec} option`);
  await codec.selectOption(senderCodec);
  await resolutionSelect.selectOption(resolution);
  await page.locator('#videoSource').selectOption('synthetic');
  await page.locator('#fps').selectOption('30');
  await page.locator('#aspectMode').selectOption('preserve');
}

export async function expectEnabled(page: Page, selector: string, description: string): Promise<void> {
  const enabled = await page.locator(selector).isEnabled();
  if (!enabled) throw new Error(`${description} is disabled`);
}

export function newTracePath(testInfo: TestInfo, name: string): string {
  return testInfo.outputPath(`${name}.trace.zip`);
}

const activeTraces = new WeakMap<BrowserContext, string>();

export async function startPostPairTrace(context: BrowserContext, path: string): Promise<void> {
  mkdirSync(dirname(path), { recursive: true });
  await context.tracing.start({ screenshots: true, snapshots: true, sources: true });
  activeTraces.set(context, path);
}

export async function stopPostPairTrace(context: BrowserContext): Promise<void> {
  const path = activeTraces.get(context);
  if (!path) return;
  activeTraces.delete(context);
  await context.tracing.stop({ path });
}

export async function saveFailureScreenshot(page: Page, testInfo: TestInfo): Promise<void> {
  await page.screenshot({
    path: testInfo.outputPath('failure.png'),
    fullPage: true,
    mask: [page.locator('#pairCode, #pair-code')],
  });
}

export function assertReceiverDelta(delta: string, wireCodec: WireCodec): void {
  const unexpectedLayerAlerts = delta
    .split('\n')
    .filter(line => line.includes('[LAYER 2 ALERT]'))
    .filter(line => !line.includes("Can't set fd... driver-name already set."));
  const errors = ['[PLAYBACK ERROR]', '[BITSTREAM ERROR]'].filter(marker => delta.includes(marker));
  if (unexpectedLayerAlerts.length > 0) errors.push('[LAYER 2 ALERT]');
  if (errors.length > 0) throw new Error(`Receiver reported streaming errors: ${errors.join(', ')}\n${delta.slice(-5000)}`);
  if (!delta.includes('[PROBE RECV] seq=1')) throw new Error('Receiver did not receive a fresh seq=1 frame');
  if (!new RegExp(`\\[BITSTREAM VALIDATOR\\] seq=1 \\(.*${wireCodec === 'hevc' ? 'VPS' : 'SPS'}`).test(delta)) {
    throw new Error(`Receiver did not validate a ${wireCodec} keyframe`);
  }
  if (!new RegExp(`\\[PLAYBACK\\] submitted_${wireCodec}_access_units=[1-9]\\d*`).test(delta)) {
    throw new Error(`Receiver did not submit a ${wireCodec} access unit`);
  }
}
