import { expect, test } from '@playwright/test';
import {
  assertCodecSupport,
  assertDiagnosticsClean,
  assertReceiverDelta,
  chooseCodec,
  diagnosticsFor,
  newTracePath,
  pairThroughUi,
  receiverLogs,
  saveFailureScreenshot,
  startPostPairTrace,
  stopPostPairTrace,
  trackDiagnostics,
  waitFor,
  writeDiagnostics,
  type CodecCase,
  type SenderCodec,
} from './support';

const cases: readonly CodecCase[] = [
  { name: 'hevc-1080p', senderCodec: 'H265', wireCodec: 'hevc', resolution: '1920x1080', cycles: 3 },
  { name: 'h264-hardware-1080p', senderCodec: 'H264', wireCodec: 'h264', resolution: '1920x1080', cycles: 3 },
  { name: 'h264-software-1080p', senderCodec: 'H264_SW', wireCodec: 'h264', resolution: '1920x1080', cycles: 3 },
  { name: 'hevc-4k-boundary', senderCodec: 'H265', wireCodec: 'hevc', resolution: '3840x2160', cycles: 1 },
];

interface EstimatedLatencySnapshot {
  total_ms: number;
  encode_ms: number;
  transport_queue_ms: number;
  decode_display_ms: number;
}

interface ManagementLatencySnapshot {
  management: {
    active_stream: null | {
      frames: number;
      bytes: number;
      measured_bitrate_mbps: number;
      measured_fps: number;
      average_bitrate_mbps: number;
      peak_bitrate_mbps: number;
      estimated_latency: EstimatedLatencySnapshot | null;
      estimated_latency_age_ms: number | null;
      latency_samples: Array<EstimatedLatencySnapshot & { elapsed_sec: number }>;
    };
    health: {
      playback_state: string;
      reassembly_in_flight: number;
      dropped_access_units: number;
      ignored_media_packets: number;
      load_1m: number | null;
      memory_available_mib: number | null;
      memory_total_mib: number | null;
    };
  };
}

function receiverDelta(before: string, after: string): string {
  return after.startsWith(before) ? after.slice(before.length) : after;
}

async function runCycle(page: Parameters<typeof pairThroughUi>[0], receiver: ReturnType<typeof receiverLogs>, testCase: CodecCase, cycle: number): Promise<void> {
  console.log(`[E2E] Starting ${testCase.name} cycle ${cycle}/${testCase.cycles} (${testCase.senderCodec}, ${testCase.resolution}).`);
  await chooseCodec(page, testCase.senderCodec, testCase.resolution);
  const before = receiver.read();
  const initialFrameCount = Number.parseInt((await page.locator('#statFrameCount').textContent()) || '0', 10);

  await page.locator('#toggleBtn').click();
  await expect(page.locator('#statusBadge')).toHaveText('STREAMING', { timeout: 30_000 });
  await expect(page.locator('#settingsLockNotice')).toBeVisible();
  await expect(page.locator('#toggleText')).toHaveText('Stop Casting');
  await expect(page.locator('.stat-label', { hasText: 'Target output' })).toHaveCount(1);
  await expect(page.locator('.stat-label', { hasText: 'Frames written to transport' })).toHaveCount(1);
  await expect(page.locator('#statCodec')).toHaveText(
    testCase.senderCodec === 'H265' ? 'HEVC / H.265' : (testCase.senderCodec === 'H264_SW' ? 'H.264 (Software Preferred)' : 'H.264'),
  );
  await expect(page.locator('#statEncoderHW')).toContainText(testCase.senderCodec === 'H264_SW' ? 'Software Preferred' : 'HW Preferred');
  await waitFor(
    () => page.locator('#statFrameCount').textContent(),
    value => Number.parseInt(value || '0', 10) >= initialFrameCount + 30,
    `${testCase.name} cycle ${cycle} to increase the frame counter by 30`,
    45_000,
  );
  await expect.poll(async () => page.locator('#statDevicePing').textContent(), { timeout: 10_000 }).toMatch(/^~\d+ ms$/);
  const estimatedLatency = Number.parseInt((await page.locator('#statDevicePing').textContent())?.replace(/\D/g, '') || '0', 10);
  expect(estimatedLatency).toBeGreaterThan(0);
  expect(estimatedLatency).toBeLessThan(5_000);
  await expect(page.locator('#statLatencyDetail')).toHaveText(/^Encode \d+ · Transport\/queue \d+ · Decode\/display ~\d+ ms$/);
  await expect.poll(async () => {
    const response = await page.request.get(`https://${process.env.E2E_BOARD_IP}:9090/api/snapshot`);
    if (!response.ok()) return null;
    const snapshot = await response.json() as ManagementLatencySnapshot;
    return snapshot.management.active_stream?.estimated_latency?.total_ms ?? null;
  }, { timeout: 10_000 }).not.toBeNull();
  const managementResponse = await page.request.get(`https://${process.env.E2E_BOARD_IP}:9090/api/snapshot`);
  expect(managementResponse.ok()).toBe(true);
  const managementSnapshot = await managementResponse.json() as ManagementLatencySnapshot;
  const activeStream = managementSnapshot.management.active_stream;
  if (!activeStream) throw new Error('Management active stream disappeared before metric validation');
  expect(activeStream.frames).toBeGreaterThan(0);
  expect(activeStream.bytes).toBeGreaterThan(0);
  expect(activeStream.measured_bitrate_mbps).toBeGreaterThan(0);
  expect(activeStream.measured_fps).toBeGreaterThan(0);
  expect(activeStream.average_bitrate_mbps).toBeGreaterThan(0);
  expect(activeStream.peak_bitrate_mbps).toBeGreaterThan(0);
  const managementLatency = managementSnapshot.management.active_stream?.estimated_latency;
  const managementLatencySamples = managementSnapshot.management.active_stream?.latency_samples ?? [];
  expect(managementLatency).not.toBeNull();
  if (!managementLatency) throw new Error('Management latency estimate disappeared before validation');
  expect(managementLatency.total_ms).toBeGreaterThan(0);
  expect(managementLatency.total_ms).toBeLessThan(5_000);
  expect(managementLatency.total_ms).toBeCloseTo(
    managementLatency.encode_ms + managementLatency.transport_queue_ms + managementLatency.decode_display_ms,
    5,
  );
  expect(managementLatencySamples.length).toBeGreaterThan(0);
  expect(activeStream.estimated_latency_age_ms).not.toBeNull();
  expect(activeStream.estimated_latency_age_ms ?? Number.POSITIVE_INFINITY).toBeLessThan(3_000);
  const graphedLatency = managementLatencySamples.at(-1);
  expect(graphedLatency?.total_ms).toBeGreaterThan(0);
  expect(graphedLatency?.total_ms).toBeLessThan(5_000);
  if (testCase.name === 'hevc-1080p' && cycle === 1) {
    const initialLatencySampleCount = managementLatencySamples.length;
    const portal = await page.context().newPage();
    await portal.goto(`https://${process.env.E2E_BOARD_IP}:9090/`, { waitUntil: 'domcontentloaded' });
    await portal.bringToFront();
    // Playwright disables normal background-page lifecycle behavior, so both
    // tabs otherwise report `visible`. Drive the same visibility transition a
    // real Chrome tab switch emits and assert the sender remains measurable.
    await page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await expect.poll(() => page.evaluate(() => document.visibilityState), { timeout: 5_000 }).toBe('hidden');
    await expect.poll(() => portal.evaluate(() => document.visibilityState), { timeout: 5_000 }).toBe('visible');
    await expect(portal.locator('#metrics')).not.toContainText('Measuring…');
    await portal.waitForTimeout(35_000);
    const sustainedResponse = await portal.request.get(`https://${process.env.E2E_BOARD_IP}:9090/api/snapshot`);
    expect(sustainedResponse.ok()).toBe(true);
    const sustainedSnapshot = await sustainedResponse.json() as ManagementLatencySnapshot;
    const sustainedStream = sustainedSnapshot.management.active_stream;
    if (!sustainedStream) throw new Error('Active stream disappeared during sustained latency sampling');
    expect(sustainedStream.latency_samples.length).toBeGreaterThanOrEqual(initialLatencySampleCount + 20);
    expect(sustainedStream.estimated_latency_age_ms).not.toBeNull();
    expect(sustainedStream.estimated_latency_age_ms ?? Number.POSITIVE_INFINITY).toBeLessThan(3_000);
    await expect(portal.locator('#metrics')).not.toContainText('Measuring…');
    await expect(portal.locator('#latencyFreshness')).toHaveText('Latency samples are current');
    await portal.close();
    await page.bringToFront();
    await page.evaluate(() => {
      Reflect.deleteProperty(document, 'visibilityState');
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await expect.poll(() => page.evaluate(() => document.visibilityState), { timeout: 5_000 }).toBe('visible');
  }
  await expect.poll(async () => {
    const response = await page.request.get(`https://${process.env.E2E_BOARD_IP}:9090/api/snapshot`);
    if (!response.ok()) return null;
    const snapshot = await response.json() as ManagementLatencySnapshot;
    return snapshot.management.health.playback_state;
  }, { timeout: 10_000 }).toBe(testCase.wireCodec === 'hevc' ? 'h265' : 'h264');
  const health = managementSnapshot.management.health;
  expect(health.reassembly_in_flight).toBeGreaterThanOrEqual(0);
  expect(health.dropped_access_units).toBeGreaterThanOrEqual(0);
  expect(health.ignored_media_packets).toBeGreaterThanOrEqual(0);

  const afterFrames = await receiver.waitFor(
    log => new RegExp(`\\[PLAYBACK\\] submitted_${testCase.wireCodec}_access_units=[1-9]\\d*`).test(receiverDelta(before, log)),
    `${testCase.name} cycle ${cycle} receiver playback`,
    45_000,
  );
  assertReceiverDelta(receiverDelta(before, afterFrames), testCase.wireCodec);

  await page.locator('#toggleBtn').click();
  await expect(page.locator('#statusBadge')).toHaveText('CONNECTED', { timeout: 30_000 });
  await expect(page.locator('#toggleText')).toHaveText('Start Casting');
  await expect(page.locator('#settingsLockNotice')).toBeHidden();
  await expect(page.locator('#userNotice')).toBeHidden();
  await expect(page.locator('#statDevicePing')).toHaveText('--');
  await expect(page.locator('#statLatencyDetail')).toHaveText('Available while streaming');
  console.log(`[E2E] Completed ${testCase.name} cycle ${cycle}/${testCase.cycles}.`);
}

test.describe('local codec matrix', () => {
  test.beforeEach(async ({ page }) => {
    trackDiagnostics(page);
    const receiverUrl = `https://${process.env.E2E_BOARD_IP}:8080/`;
    let lastError: unknown;
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      try {
        console.log(`[E2E] Opening local receiver UI (navigation attempt ${attempt}/3).`);
        await page.goto(receiverUrl, { waitUntil: 'commit', timeout: 45_000 });
        await page.locator('#pairCode').waitFor({ state: 'visible', timeout: 15_000 });
        lastError = undefined;
        break;
      } catch (error) {
        lastError = error;
        if (attempt < 3) await page.waitForTimeout(1_000);
      }
    }
    if (lastError) throw lastError;
  });

  test.afterEach(async ({ page }, testInfo) => {
    await stopPostPairTrace(page.context());
    const diagnostics = diagnosticsFor(page);
    if (diagnostics) writeDiagnostics(diagnostics, 'codec-browser');
    if (testInfo.status !== testInfo.expectedStatus) {
      try { await saveFailureScreenshot(page, testInfo); } catch { /* page may already be closed */ }
    } else if (diagnostics) {
      assertDiagnosticsClean(diagnostics);
    }
  });

  test('pairs locally and exercises HEVC/H.264 decoder boundaries', async ({ page }, testInfo) => {
    console.log('[E2E] Opening local receiver UI and pairing directly over LAN.');
    const receiver = receiverLogs();
    await pairThroughUi(page, process.env.E2E_PAIRING_CODE || '');
    const pairedLogs = receiver.read();
    expect(pairedLogs).not.toContain('[CLOUD DISCOVERY]');
    await expect(page.locator('#statusBadge')).toHaveText('CONNECTED');
    await expect.poll(async () => page.locator('#statSignal').textContent()).not.toBe('--');
    await expect(page.locator('#statDevicePing')).toHaveText('--');
    await expect(page.locator('#statLatencyDetail')).toHaveText('Available while streaming');
    await expect(page.locator('.stat-label', { hasText: 'Target output' })).toHaveCount(1);
    await expect(page.locator('.stat-label', { hasText: 'Frames written to transport' })).toHaveCount(1);
    await expect(page.locator('#userNotice')).toBeHidden();

    await assertCodecSupport(page, 'H265', '1920x1080');
    await assertCodecSupport(page, 'H264', '1920x1080');
    await assertCodecSupport(page, 'H264_SW', '1920x1080');

    for (const senderCodec of ['H264', 'H264_SW'] as const satisfies readonly SenderCodec[]) {
      await chooseCodec(page, senderCodec, '1920x1080');
      await expect(page.locator('#resolution option[value="1280x720"]')).toBeEnabled();
      await expect(page.locator('#resolution option[value="1920x1080"]')).toBeEnabled();
      await expect(page.locator('#resolution option[value="2560x1440"]')).toBeDisabled();
      await expect(page.locator('#resolution option[value="3840x2160"]')).toBeDisabled();
    }

    await startPostPairTrace(page.context(), newTracePath(testInfo, 'codec'));
    console.log(`[E2E] Starting ${cases.reduce((total, testCase) => total + testCase.cycles, 0)} local codec cycles.`);
    for (const testCase of cases) {
      for (let cycle = 1; cycle <= testCase.cycles; cycle += 1) {
        await runCycle(page, receiver, testCase, cycle);
      }
    }
    expect(receiver.read()).not.toContain('[CLOUD DISCOVERY]');
  });
});
