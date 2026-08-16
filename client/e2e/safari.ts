import { execFileSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

// Safari coverage intentionally uses Apple's installed Safari WebDriver.
// Do not replace this with Playwright WebKit: it is a different browser engine.
type JsonObject = Record<string, unknown>;
type ElementReference = { id: string };

const ELEMENT_KEY = 'element-6066-11e4-a52e-4f735466cecf';
const boardIp = process.env.E2E_BOARD_IP;
const pairingCode = process.env.E2E_PAIRING_CODE;
const artifactDir = process.env.E2E_ARTIFACT_DIR || '../.artefact/manual';
const webdriverUrl = process.env.SAFARI_WEBDRIVER_URL || 'http://127.0.0.1:4444';

if (!boardIp || !pairingCode) throw new Error('Missing E2E_BOARD_IP or E2E_PAIRING_CODE; invoke this suite through ./test_browser.sh codec safari');

function redact(value: string): string {
  return value
    .split(pairingCode || '')
    .join('[REDACTED-CODE]')
    .replace(/([?&]token=)[^&\s]+/gi, '$1[REDACTED]')
    .replace(/(connection_token["']?\s*[:=]\s*["']?)[A-Za-z0-9._-]+/gi, '$1[REDACTED]')
    .replace(/(authorization\s*[:=]\s*["']?bearer\s+)[^"'\s]+/gi, '$1[REDACTED]');
}

function sleep(milliseconds: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, milliseconds));
}

class SafariDriver {
  private sessionId: string | null = null;
  private readonly baseUrl: string;

  constructor(baseUrl: string) {
    this.baseUrl = baseUrl;
  }

  private async request<T>(method: string, path: string, body?: JsonObject): Promise<T> {
    const response = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers: body ? { 'content-type': 'application/json' } : undefined,
      body: body ? JSON.stringify(body) : undefined,
    });
    const payload = await response.json() as { value?: unknown };
    const value = payload.value as JsonObject | undefined;
    if (!response.ok || value?.error) {
      const message = value?.message || `HTTP ${response.status}`;
      throw new Error(`Safari WebDriver ${method} ${path}: ${redact(String(message))}`);
    }
    return payload.value as T;
  }

  private sessionPath(path: string): string {
    if (!this.sessionId) throw new Error('Safari WebDriver session is not active');
    return `/session/${this.sessionId}${path}`;
  }

  async start(): Promise<void> {
    const value = await this.request<JsonObject>('POST', '/session', {
      capabilities: { alwaysMatch: { browserName: 'safari', acceptInsecureCerts: true } },
    });
    this.sessionId = String(value.sessionId || '');
    if (!this.sessionId) throw new Error('Safari WebDriver did not return a session id');
  }

  async stop(): Promise<void> {
    if (!this.sessionId) return;
    try { await this.request('DELETE', this.sessionPath('')); } finally { this.sessionId = null; }
  }

  async navigate(url: string): Promise<void> {
    await this.request('POST', this.sessionPath('/url'), { url });
  }

  private async element(selector: string): Promise<ElementReference> {
    const value = await this.request<JsonObject>('POST', this.sessionPath('/element'), {
      using: 'css selector',
      value: selector,
    });
    const id = value[ELEMENT_KEY] || value['ELEMENT'];
    if (!id) throw new Error(`Safari could not locate ${selector}`);
    return { id: String(id) };
  }

  async exists(selector: string): Promise<boolean> {
    try { await this.element(selector); return true; } catch { return false; }
  }

  async click(selector: string): Promise<void> {
    const element = await this.element(selector);
    await this.request('POST', this.sessionPath(`/element/${element.id}/click`));
  }

  async setValue(selector: string, value: string): Promise<void> {
    const element = await this.element(selector);
    await this.request('POST', this.sessionPath(`/element/${element.id}/value`), {
      text: value,
      value: [...value],
    });
  }

  async text(selector: string): Promise<string> {
    const element = await this.element(selector);
    return this.request<string>('GET', this.sessionPath(`/element/${element.id}/text`));
  }

  async execute<T>(script: string, args: unknown[] = []): Promise<T> {
    return this.request<T>('POST', this.sessionPath('/execute/sync'), { script, args });
  }

  async executeAsync<T>(script: string, args: unknown[] = []): Promise<T> {
    return this.request<T>('POST', this.sessionPath('/execute/async'), { script, args });
  }

  async screenshot(): Promise<string> {
    return this.request<string>('GET', this.sessionPath('/screenshot'));
  }

  async source(): Promise<string> {
    return this.request<string>('GET', this.sessionPath('/source'));
  }

  async waitFor(selector: string, predicate: (value: string) => boolean, description: string, timeoutMs = 30_000): Promise<string> {
    const deadline = Date.now() + timeoutMs;
    let last = '';
    while (Date.now() < deadline) {
      if (await this.exists(selector)) {
        last = await this.text(selector);
        if (predicate(last)) return last;
      }
      await sleep(250);
    }
    throw new Error(`Timed out waiting for ${description}; last value: ${redact(last)}`);
  }
}

function receiverRead(): string {
  return redact(execFileSync(
    'ssh',
    ['-o', 'BatchMode=yes', boardIp!, 'docker logs --timestamps llrdc-casting 2>&1'],
    { encoding: 'utf8', timeout: 10_000 },
  ));
}

async function receiverWait(predicate: (log: string) => boolean, description: string, timeoutMs = 45_000): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  let latest = '';
  while (Date.now() < deadline) {
    latest = receiverRead();
    if (predicate(latest)) return latest;
    await sleep(250);
  }
  throw new Error(`Timed out waiting for receiver log: ${description}\n${latest.slice(-4000)}`);
}

function receiverDelta(before: string, after: string): string {
  return after.startsWith(before) ? after.slice(before.length) : after;
}

function assertReceiverDelta(delta: string, wireCodec: 'hevc' | 'h264'): void {
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
  if (!delta.includes(`[PLAYBACK] submitted_${wireCodec}_access_units=1`)) {
    throw new Error(`Receiver did not submit the first ${wireCodec} access unit`);
  }
}

async function installDiagnostics(driver: SafariDriver): Promise<void> {
  await driver.execute(`(() => {
    const state = { consoleMessages: [], consoleErrors: [], pageErrors: [], requestFailures: [] };
    window.__llrdcSafariDiagnostics = state;
    for (const type of ['log', 'info', 'warn', 'error']) {
      const original = console[type];
      console[type] = (...args) => {
        const message = args.map(value => String(value)).join(' ');
        state.consoleMessages.push('[' + type + '] ' + message);
        if (type === 'error') state.consoleErrors.push(message);
        original.apply(console, args);
      };
    }
    window.addEventListener('error', event => state.pageErrors.push(String(event.error?.message || event.message)));
    window.addEventListener('unhandledrejection', event => state.pageErrors.push(String(event.reason?.message || event.reason)));
    const originalFetch = window.fetch;
    window.fetch = (...args) => originalFetch(...args).catch(error => {
      state.requestFailures.push(String(args[0]) + ': ' + String(error));
      throw error;
    });
  })()`);
}

async function capabilitySupported(driver: SafariDriver, codec: 'H265' | 'H264'): Promise<boolean> {
  return driver.executeAsync(`const args = arguments[0];
    const done = arguments[arguments.length - 1];
    if (typeof VideoEncoder === 'undefined' || typeof VideoEncoder.isConfigSupported !== 'function') { done(false); return; }
    const config = args.codec === 'H265'
      ? { codec: 'hev1.1.6.L150.B0', width: 1920, height: 1088, bitrate: 6000000, framerate: 30, hardwareAcceleration: 'prefer-hardware' }
      : { codec: 'avc1.42e028', width: 1920, height: 1088, bitrate: 8000000, framerate: 30, hardwareAcceleration: 'prefer-hardware' };
    VideoEncoder.isConfigSupported(config).then(result => done(!!result.supported)).catch(() => done(false));
  `, [{ codec }]);
}

async function select(driver: SafariDriver, selector: string, value: string): Promise<void> {
  await driver.execute(`const args = arguments[0];
    const select = document.querySelector(args.selector);
    if (!select) throw new Error('Missing select ' + args.selector);
    select.value = args.value;
    select.dispatchEvent(new Event('input', { bubbles: true }));
    select.dispatchEvent(new Event('change', { bubbles: true }));
  `, [{ selector, value }]);
}

async function runCycle(driver: SafariDriver, codec: 'H265' | 'H264', before: string): Promise<string> {
  await select(driver, '#codec', codec);
  await select(driver, '#resolution', '1920x1080');
  await select(driver, '#videoSource', 'synthetic');
  await select(driver, '#fps', '30');
  await select(driver, '#aspectMode', 'preserve');
  await driver.click('#toggleBtn');
  await driver.waitFor('#statusBadge', value => value.trim() === 'STREAMING', `${codec} STREAMING`, 30_000);
  await driver.waitFor('#settingsLockNotice', value => value.trim().length > 0, `${codec} settings lock`, 15_000);
  await driver.waitFor('#statFrameCount', value => Number.parseInt(value, 10) >= 30, `${codec} frame counter`, 45_000);
  const afterFrames = await receiverWait(log => receiverDelta(before, log).includes(`[PLAYBACK] submitted_${codec === 'H265' ? 'hevc' : 'h264'}_access_units=1`), `${codec} receiver playback`);
  assertReceiverDelta(receiverDelta(before, afterFrames), codec === 'H265' ? 'hevc' : 'h264');
  await driver.click('#toggleBtn');
  await driver.waitFor('#statusBadge', value => value.trim() === 'CONNECTED', `${codec} CONNECTED after stop`, 30_000);
  await driver.waitFor('#toggleText', value => value.trim() === 'Start Casting', `${codec} clean stop`, 15_000);
  return afterFrames;
}

async function main(): Promise<void> {
  mkdirSync(artifactDir, { recursive: true });
  const driver = new SafariDriver(webdriverUrl);
  const diagnosticsPath = join(artifactDir, 'safari-browser.json');
  try {
    await driver.start();
    await driver.navigate(`https://${boardIp}:8080/`);
    await driver.waitFor('#pairCode', value => value.length >= 0, 'Safari local pairing page', 45_000);
    await installDiagnostics(driver);
    await driver.setValue('#pairCode', pairingCode!);
    await driver.click('#pairBtn');
    await driver.waitFor('#pairStatus', value => value.trim() === 'PAIRED', 'Safari PAIRED', 60_000);
    await driver.waitFor('#statusBadge', value => value.trim() === 'CONNECTED', 'Safari CONNECTED', 60_000);
    await driver.waitFor('#resolution', value => value.length > 0, 'Safari casting controls', 15_000);
    const pairedLogs = receiverRead();
    if (pairedLogs.includes('[CLOUD DISCOVERY]')) throw new Error('Safari codec run observed cloud discovery while cloud is disabled');
    if (!(await capabilitySupported(driver, 'H265'))) throw new Error('Safari H265 WebCodecs configuration is unsupported');
    if (!(await capabilitySupported(driver, 'H264'))) throw new Error('Safari H264 WebCodecs configuration is unsupported');

    console.log('[E2E][Safari] PAIRED and CONNECTED directly over LAN.');
    for (const codec of ['H265', 'H264'] as const) {
      console.log(`[E2E][Safari] Starting ${codec} 1080p cycle.`);
      const before = receiverRead();
      await runCycle(driver, codec, before);
      console.log(`[E2E][Safari] Completed ${codec} 1080p cycle.`);
    }

    const diagnostics = (await driver.execute<JsonObject>(`return (() => {
      const state = window.__llrdcSafariDiagnostics || { consoleMessages: [], consoleErrors: [], pageErrors: [], requestFailures: [] };
      const log = document.querySelector('#log');
      return { ...state, domLog: log ? log.textContent || '' : '' };
    })()`)) || {};
    writeFileSync(diagnosticsPath, JSON.stringify(diagnostics, null, 2));
    const failures = [
      ...((diagnostics.consoleErrors as string[]) || []),
      ...((diagnostics.pageErrors as string[]) || []),
      ...((diagnostics.requestFailures as string[]) || []),
    ];
    if (failures.length > 0) throw new Error(`Safari browser diagnostics reported failures:\n${failures.join('\n')}`);
    const domLog = String(diagnostics.domLog || '');
    if (domLog.includes('[PLAYBACK ERROR]') || domLog.includes('[BITSTREAM ERROR]')) {
      throw new Error('Safari DOM log reported playback or bitstream errors');
    }
  } catch (error) {
    try {
      const screenshot = await driver.screenshot();
      writeFileSync(join(artifactDir, 'failure.png'), Buffer.from(screenshot, 'base64'));
      writeFileSync(join(artifactDir, 'failure.html'), await driver.source());
    } catch { /* Safari may not have created a session or page */ }
    try {
      const diagnostics = await driver.execute<JsonObject>(`return (() => {
        const state = window.__llrdcSafariDiagnostics || { consoleMessages: [], consoleErrors: [], pageErrors: [], requestFailures: [] };
        const log = document.querySelector('#log');
        return { ...state, domLog: log ? log.textContent || '' : '' };
      })()`);
      writeFileSync(diagnosticsPath, redact(JSON.stringify(diagnostics, null, 2)));
    } catch { /* preserve the original failure if the page is gone */ }
    throw error;
  } finally {
    try { await driver.stop(); } catch { /* preserve the original failure */ }
  }
}

main().catch(error => {
  console.error(`[E2E][Safari] ${redact(error instanceof Error ? error.message : String(error))}`);
  process.exitCode = 1;
});
