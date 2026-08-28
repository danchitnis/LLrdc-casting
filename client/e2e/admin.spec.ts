import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { expect, test, type Page } from '@playwright/test';
import { assertDiagnosticsClean, trackDiagnostics, writeDiagnostics } from './support';

const boardIp = process.env.E2E_BOARD_IP;
const visualDelayMs = Math.max(0, Number(process.env.E2E_VISUAL_DELAY_MS || 1_000));
type Settings = Record<string, unknown>;

interface Snapshot {
  management: { active_stream: unknown };
  pairing: { cloud_status: string };
  settings: Settings & {
    cloud_configuration_ready: boolean;
    cloud_configuration_missing: string[];
    cloud_state: string;
  };
  watchdog: {
    receiver_state: string;
    receiver_generation: number;
    receiver_pid: number | null;
    restart_count: number;
    consecutive_failures: number;
    last_failure: string | null;
  };
}

const editableKeys = [
  'port', 'webtransport_port', 'http_port', 'drm_connector_id', 'drm_plane_id',
  'idle_dashboard', 'idle_dashboard_mode', 'idle_timeout_sec',
  'sender_liveness_timeout_sec', 'udp_buffer_size_mb', 'pairing_code_ttl_sec',
  'local_pairing_code_required', 'cloud_discovery_enabled',
] as const;

const deploymentKeys = [
  'admin_bind_address', 'admin_port', 'cert_dir', 'pairing_worker_url',
  'receiver_id', 'pairing_token_public_key_file',
] as const;

function editableSettings(settings: Settings): Settings {
  return Object.fromEntries(editableKeys.map(key => [key, settings[key]]));
}

function deploymentSettings(settings: Settings): Settings {
  return Object.fromEntries(deploymentKeys.map(key => [key, settings[key]]));
}

async function getSnapshot(page: Page): Promise<Snapshot> {
  const response = await page.request.get(`https://${boardIp}:9090/api/snapshot`);
  expect(response.ok()).toBe(true);
  return await response.json() as Snapshot;
}

function persistedConfig(): string {
  return execFileSync('ssh', [
    '-o', 'BatchMode=yes', boardIp as string,
    'docker exec llrdc-casting cat /config/config.yaml',
  ], { encoding: 'utf8', timeout: 10_000 });
}

function boardCommand(command: string): string {
  return execFileSync('ssh', ['-o', 'BatchMode=yes', boardIp as string, command], { encoding: 'utf8', timeout: 20_000 });
}

function killManagerFromHost(): void {
  const hostPid = boardCommand("docker inspect --format '{{.State.Pid}}' llrdc-casting").trim();
  if (!/^\d+$/.test(hostPid) || Number(hostPid) < 2) throw new Error(`Invalid manager host PID: ${hostPid}`);
  // A process inside a PID namespace cannot reliably SIGKILL that namespace's
  // init. Enter the host PID namespace from a disposable privileged container
  // so Docker observes a genuine unexpected PID 1 exit and applies its policy.
  boardCommand(`docker run --rm --pid=host --privileged --entrypoint /bin/sh llrdc-casting -c 'kill -KILL ${hostPid}'`);
}

function yamlValue(value: unknown): string {
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'string') return JSON.stringify(value);
  return String(value);
}

async function visualPause(page: Page): Promise<void> {
  if (visualDelayMs > 0) await page.waitForTimeout(visualDelayMs);
}

function expectPersistedSettings(text: string, settings: Settings): void {
  for (const key of editableKeys) expect(text).toContain(`  ${key}: ${yamlValue(settings[key])}`);
}

function unusedPorts(): [number, number, number] {
  const listening = execFileSync('ssh', [
    '-o', 'BatchMode=yes', boardIp as string,
    'ss -H -lntu 2>/dev/null || true',
  ], { encoding: 'utf8', timeout: 10_000 });
  const used = new Set([...listening.matchAll(/:(\d+)\s/g)].map(match => Number(match[1])));
  const available: number[] = [];
  for (let port = 55000; port < 55100 && available.length < 3; port += 1) {
    if (!used.has(port)) available.push(port);
  }
  if (available.length !== 3) throw new Error('Could not find three unused receiver test ports');
  return available as [number, number, number];
}

async function applySettings(page: Page, settings: Settings): Promise<Snapshot> {
  const selectors: Record<string, string> = {
    port: '#settingPort', webtransport_port: '#settingWebtransportPort', http_port: '#settingHttpPort',
    drm_connector_id: '#settingDrmConnector', drm_plane_id: '#settingDrmPlane',
    idle_dashboard: '#settingIdleDashboard', idle_dashboard_mode: '#settingDashboardMode',
    idle_timeout_sec: '#settingIdleTimeout', sender_liveness_timeout_sec: '#settingLivenessTimeout',
    udp_buffer_size_mb: '#settingUdpBuffer', pairing_code_ttl_sec: '#settingPairingTtl',
  };
  const selectKeys = new Set(['idle_dashboard', 'idle_dashboard_mode']);
  for (const [key, value] of Object.entries(settings)) {
    if (key === 'cloud_discovery_enabled') {
      const checkbox = page.locator('#cloudEnabled');
      if (await checkbox.isChecked() !== value) {
        if (value) await checkbox.check(); else await checkbox.uncheck();
      }
    } else if (key === 'local_pairing_code_required') {
      const checkbox = page.locator('#localPairingRequired');
      if (await checkbox.isChecked() !== value) {
        if (value) await checkbox.check(); else await checkbox.uncheck();
      }
    } else if (selectors[key] && selectKeys.has(key)) {
      await page.locator(selectors[key]).selectOption(String(value));
    } else if (selectors[key]) {
      await page.locator(selectors[key]).fill(String(value));
    }
  }
  await visualPause(page);
  page.once('dialog', dialog => dialog.accept());
  // Dispatch synchronously so the assertion observes the UI's disabled state
  // before a very fast receiver restart can complete and reconnect the socket.
  await page.evaluate(() => (document.querySelector('#saveSettings') as HTMLButtonElement).click());
  await expect(page.locator('#settingsStatus')).toContainText(/receiver restarting/i);
  await expect(page.locator('#settingPort')).toBeDisabled({ timeout: 1_000 });
  await expect.poll(async () => {
    try {
      const current = await getSnapshot(page);
      return JSON.stringify(editableSettings(current.settings)) === JSON.stringify(settings) ? current : null;
    } catch {
      return null;
    }
  }, { timeout: 120_000, intervals: [500, 1_000, 2_000] }).not.toBeNull();
  await expect(page.locator('#settingsStatus')).toContainText(/settings are active/i, { timeout: 120_000 });
  const current = await getSnapshot(page);
  expect(editableSettings(current.settings)).toEqual(settings);
  expectPersistedSettings(persistedConfig(), settings);
  await visualPause(page);
  return current;
}

test.describe('receiver management portal', () => {
  test.skip(!boardIp, 'E2E_BOARD_IP is required');

  test('loads the typed Astro dashboard and receives a live snapshot', async ({ page }, testInfo) => {
    const diagnostics = trackDiagnostics(page);
    try {
      const response = await page.goto(`https://${boardIp}:9090/`, { waitUntil: 'domcontentloaded' });
      expect(response?.ok()).toBe(true);
      await expect(page).toHaveTitle('LLRDC Management');
      await expect(page.locator('h1')).toHaveText('LLRDC Receiver Management');
      await expect(page.locator('#state')).toHaveText(/^(IDLE|STREAMING)$/);
      const receiverState = (await page.locator('#state').textContent())?.trim();
      if (receiverState === 'STREAMING') {
        await expect(page.locator('#metrics .metric')).toHaveCount(10);
        await expect(page.locator('#stop')).toBeEnabled();
      } else {
        await expect(page.locator('#metrics .metric')).toHaveCount(1);
        await expect(page.locator('#metrics')).toContainText('Stream');
        await expect(page.locator('#metrics')).toContainText('Idle');
      }
      await expect(page.locator('#health .metric')).toHaveCount(12);
      await expect(page.locator('#events')).not.toBeEmpty();
      assertDiagnosticsClean(diagnostics);
    } finally {
      writeDiagnostics(diagnostics, `${testInfo.project.name}-browser`);
    }
  });

  test('keeps operational logs in a dedicated, scrollable, color-coded tab', async ({ page }, testInfo) => {
    const diagnostics = trackDiagnostics(page);
    try {
      await page.goto(`https://${boardIp}:9090/`, { waitUntil: 'domcontentloaded' });
      await expect(page.locator('#overviewTab')).toBeVisible();
      await expect(page.locator('#logsTab')).toBeHidden();
      await expect(page.locator('#settingsTab')).toBeHidden();

      await page.locator('#logsTabButton').click();
      await expect(page.locator('#logsTab')).toBeVisible();
      await expect(page.locator('#overviewTab')).toBeHidden();
      await expect(page.locator('#logsTabButton')).toHaveAttribute('aria-selected', 'true');
      await expect(page.locator('#operationalLogs .log-entry').first()).toBeVisible({ timeout: 15_000 });
      await expect(page.locator('#logCount')).toContainText(/Showing \d+ of \d+ retained events/);

      const presentation = await page.locator('#operationalLogs').evaluate((node) => ({
        overflowY: getComputedStyle(node).overflowY,
        maxHeight: getComputedStyle(node).maxHeight,
      }));
      expect(presentation.overflowY).toBe('auto');
      expect(presentation.maxHeight).not.toBe('none');

      await page.locator('#logSeverity').selectOption('info');
      await expect(page.locator('#operationalLogs .log-entry').first()).toHaveClass(/log-info/);
      const markerColor = await page.locator('#operationalLogs .log-entry').first().evaluate((node) => getComputedStyle(node, '::before').backgroundColor);
      expect(markerColor).toBe('rgb(53, 212, 154)');

      await page.locator('#logSeverity').selectOption('all');
      const categoryOption = page.locator('#logCategory option').nth(1);
      if (await categoryOption.count()) {
        await page.locator('#logCategory').selectOption(await categoryOption.getAttribute('value') || 'all');
        await expect(page.locator('#operationalLogs .log-entry').first()).toBeVisible();
      }
      await page.locator('#jumpLatest').click();
      assertDiagnosticsClean(diagnostics);
    } finally {
      writeDiagnostics(diagnostics, `${testInfo.project.name}-logs-browser`);
    }
  });

  test('watchdog recovers crashes and hangs while the portal stays available', async ({ page }, testInfo) => {
    const diagnostics = trackDiagnostics(page);
    try {
      await page.goto(`https://${boardIp}:9090/`, { waitUntil: 'domcontentloaded' });
      const initial = await getSnapshot(page);
      expect(initial.watchdog.receiver_state).toBe('ready');
      expect(initial.watchdog.receiver_pid).not.toBeNull();
      const disconnects = Number(await page.locator('html').getAttribute('data-portal-disconnects') || '0');

      boardCommand(`docker exec llrdc-casting kill -KILL ${initial.watchdog.receiver_pid}`);
      await expect.poll(
        async () => (await getSnapshot(page)).watchdog.receiver_generation,
        { timeout: 60_000, intervals: [500, 1_000, 2_000] },
      ).toBeGreaterThan(initial.watchdog.receiver_generation);
      await expect.poll(async () => (await getSnapshot(page)).watchdog.receiver_state, { timeout: 60_000 }).toBe('ready');
      const afterCrash = await getSnapshot(page);
      expect(afterCrash.watchdog.receiver_generation).toBeGreaterThan(initial.watchdog.receiver_generation);
      expect(afterCrash.watchdog.last_failure).toMatch(/signal|code/);
      expect(Number(await page.locator('html').getAttribute('data-portal-disconnects') || '0')).toBe(disconnects);

      expect(afterCrash.watchdog.receiver_pid).not.toBeNull();
      boardCommand(`docker exec llrdc-casting kill -STOP ${afterCrash.watchdog.receiver_pid}`);
      await expect.poll(async () => (await getSnapshot(page)).watchdog.receiver_generation, { timeout: 90_000, intervals: [1_000, 2_000] }).toBeGreaterThan(afterCrash.watchdog.receiver_generation);
      await expect.poll(async () => (await getSnapshot(page)).watchdog.receiver_state, { timeout: 60_000 }).toBe('ready');
      const afterHang = await getSnapshot(page);
      expect(afterHang.watchdog.last_failure).toMatch(/signal|heartbeat|code/);
      expect(Number(await page.locator('html').getAttribute('data-portal-disconnects') || '0')).toBe(disconnects);

      await page.locator('#logsTabButton').click();
      const downloadPromise = page.waitForEvent('download');
      await page.locator('#downloadLogs').click();
      const download = await downloadPromise;
      expect(download.suggestedFilename()).toBe('llrdc-diagnostics.zip');
      assertDiagnosticsClean(diagnostics);

      killManagerFromHost();
      await expect.poll(async () => {
        try { return (await getSnapshot(page)).watchdog.receiver_state; } catch { return 'unavailable'; }
      }, { timeout: 90_000, intervals: [1_000, 2_000] }).toBe('ready');
      await expect(page.locator('#state')).toHaveText(/^(IDLE|STREAMING)$/, { timeout: 30_000 });
      await expect.poll(
        async () => Number(await page.locator('html').getAttribute('data-portal-disconnects') || '0'),
        { timeout: 30_000, intervals: [250, 500, 1_000] },
      ).toBeGreaterThan(disconnects);
    } finally {
      writeDiagnostics(diagnostics, `${testInfo.project.name}-watchdog-browser`);
    }
  });

  test('changes every editable setting, verifies receiver state, and restores it', async ({ page }, testInfo) => {
    const diagnostics = trackDiagnostics(page);
    let initial: Snapshot | undefined;
    let initialConfig = '';
    try {
      await page.goto(`https://${boardIp}:9090/`, { waitUntil: 'domcontentloaded' });
      await page.locator('#settingsTabButton').click();
      await expect(page.locator('#settingsTab')).toBeVisible();
      await expect(page.locator('#settingsTab .card').last()).toContainText('Cloud discovery');
      await visualPause(page);
      initial = await getSnapshot(page);
      initialConfig = process.env.E2E_MANAGEMENT_INITIAL_CONFIG
        ? readFileSync(process.env.E2E_MANAGEMENT_INITIAL_CONFIG, 'utf8')
        : persistedConfig();
      expect(initial.settings.cloud_configuration_ready, `Cloud provisioning is incomplete: ${initial.settings.cloud_configuration_missing.join(', ')}`).toBe(true);

      if (initial.management.active_stream) {
        page.once('dialog', dialog => dialog.accept());
        await page.locator('#stop').click();
        await expect(page.locator('#state')).toHaveText('IDLE', { timeout: 30_000 });
      }

      const [port, webtransportPort, httpPort] = unusedPorts();
      const first = {
        ...editableSettings(initial.settings),
        port, webtransport_port: webtransportPort, http_port: httpPort,
        idle_dashboard: !initial.settings.idle_dashboard,
        idle_dashboard_mode: initial.settings.idle_dashboard_mode === 'raw' ? 'hevc' : 'raw',
        idle_timeout_sec: Number(initial.settings.idle_timeout_sec) + 7,
        sender_liveness_timeout_sec: Number(initial.settings.sender_liveness_timeout_sec) + 11,
        udp_buffer_size_mb: Number(initial.settings.udp_buffer_size_mb) + 1,
        pairing_code_ttl_sec: Number(initial.settings.pairing_code_ttl_sec) + 60,
        cloud_discovery_enabled: process.env.E2E_MANAGEMENT_FIXED_CODE ? initial.settings.cloud_discovery_enabled : !initial.settings.cloud_discovery_enabled,
        local_pairing_code_required: false,
      };
      const afterFirst = await applySettings(page, first);
      expect(deploymentSettings(afterFirst.settings)).toEqual(deploymentSettings(initial.settings));
      if (first.cloud_discovery_enabled && !process.env.E2E_MANAGEMENT_FIXED_CODE) {
        await expect.poll(() => page.locator('#health .metric').allTextContents(), { timeout: 120_000 }).toContain('CloudREADY');
      }

      const directClient = await page.context().newPage();
      await directClient.goto(`https://${boardIp}:${first.http_port}/`, { waitUntil: 'domcontentloaded' });
      await expect(directClient.locator('#pairForm')).toBeHidden({ timeout: 30_000 });
      await expect(directClient.locator('#pairStatus')).toHaveText(/PAIRED \(CODE DISABLED\)/, { timeout: 30_000 });
      await directClient.close();

      // Keep the dashboard disabled while exercising alternate DRM identifiers.
      const second = {
        ...first,
        idle_dashboard: false,
        local_pairing_code_required: true,
        drm_connector_id: initial.settings.drm_connector_id === 'auto' ? '54' : 'auto',
        drm_plane_id: initial.settings.drm_plane_id === '33' ? '31' : '33',
      };
      const afterSecond = await applySettings(page, second);
      expect(deploymentSettings(afterSecond.settings)).toEqual(deploymentSettings(initial.settings));

      const fixedCode = process.env.E2E_MANAGEMENT_FIXED_CODE;
      if (fixedCode) {
        const pairedClient = await page.context().newPage();
        await pairedClient.goto(`https://${boardIp}:${second.http_port}/`, { waitUntil: 'domcontentloaded' });
        await expect(pairedClient.locator('#pairForm')).toBeVisible({ timeout: 30_000 });
        await pairedClient.locator('#pairCode').fill(fixedCode);
        await pairedClient.locator('#pairBtn').click();
        await expect(pairedClient.locator('#pairStatus')).toHaveText('PAIRED', { timeout: 30_000 });
        await pairedClient.close();
      }
    } finally {
      if (initial) {
        const original = editableSettings(initial.settings);
        await visualPause(page);
        const response = await page.request.put(`https://${boardIp}:9090/api/settings`, {
          headers: { 'Content-Type': 'application/json' },
          data: { settings: original, confirm_restart: true },
        });
        expect([200, 202]).toContain(response.status());
        await expect.poll(async () => {
          try {
            const current = await getSnapshot(page);
            return JSON.stringify(editableSettings(current.settings)) === JSON.stringify(original) ? current : null;
          } catch {
            return null;
          }
        }, { timeout: 120_000, intervals: [500, 1_000, 2_000] }).not.toBeNull();
        expect(createHash('sha256').update(persistedConfig()).digest('hex')).toBe(createHash('sha256').update(initialConfig).digest('hex'));
      }
      writeDiagnostics(diagnostics, `${testInfo.project.name}-settings-browser`);
    }
  });

  test('shows repository-managed and editable runtime settings', async ({ page }, testInfo) => {
    const diagnostics = trackDiagnostics(page);
    try {
      await page.goto(`https://${boardIp}:9090/`, { waitUntil: 'domcontentloaded' });
      await page.locator('#settingsTabButton').click();
      const current = await getSnapshot(page);
      await expect(page.locator('#settingPort')).toHaveValue(String(current.settings.port));
      await expect(page.locator('#settingHttpPort')).toHaveValue(String(current.settings.http_port));
      await expect(page.locator('#deploymentSettings')).toContainText(/Management bind/);
      await expect(page.locator('#settingsTab .card').last()).toContainText('Cloud discovery');
      await expect(page.locator('#saveSettings')).toBeDisabled();
      await expect(page.locator('#saveCloud')).toHaveCount(0);

      const cloudEnabled = page.locator('#cloudEnabled');
      await expect(cloudEnabled).toHaveJSProperty('checked', Boolean(current.settings.cloud_discovery_enabled));
      await cloudEnabled.click();
      await expect(page.locator('#saveSettings')).toBeEnabled();

      await page.reload({ waitUntil: 'domcontentloaded' });
      await page.locator('#settingsTabButton').click();
      const reloaded = await getSnapshot(page);
      await expect(page.locator('#saveSettings')).toBeDisabled();
      const localPairingRequired = page.locator('#localPairingRequired');
      await expect(localPairingRequired).toHaveJSProperty('checked', Boolean(reloaded.settings.local_pairing_code_required));
      await localPairingRequired.click();
      await expect(page.locator('#saveSettings')).toBeEnabled();
      assertDiagnosticsClean(diagnostics);
    } finally {
      writeDiagnostics(diagnostics, `${testInfo.project.name}-settings-browser`);
    }
  });
});
