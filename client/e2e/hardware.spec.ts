import { expect, test } from '@playwright/test';
import {
  adminIp,
  assertCodecSupport,
  assertDiagnosticsClean,
  assertReceiverDelta,
  chooseCodec,
  diagnosticsFor,
  newTracePath,
  pairThroughUi,
  receiverSystemInfo,
  receiverLogs,
  saveFailureScreenshot,
  saveUiScreenshot,
  startPostPairTrace,
  stopPostPairTrace,
  trackDiagnostics,
  waitFor,
  writeDiagnostics,
  writeJsonArtifact,
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
  sender_queue_ms: number;
  delivery_ms: number;
  receiver_queue_ms: number;
  transport_queue_ms: number;
  decode_display_ms: number;
  media_write_blocked_ms: number;
  clock_uncertainty_ms: number;
  adaptive_bitrate_mbps: number;
  configured_bitrate_mbps: number;
  dropped_input_frames: number;
  effective_fps: number;
}

interface ManagementLatencySnapshot {
  management: {
    active_stream: null | {
      sender: null | {
        user_agent: string;
        platform: string;
      };
      config: {
        codec: string;
        resolution: string;
        fps: number;
        bitrate_mbps: number;
        latency_mode: string;
        aspect_mode: string;
        capture_resolution: string;
        encoded_resolution: string;
      };
      frames: number;
      bytes: number;
      measured_bitrate_mbps: number;
      measured_fps: number;
      average_bitrate_mbps: number;
      peak_bitrate_mbps: number;
      sequence_gaps: number;
      estimated_latency: EstimatedLatencySnapshot | null;
      estimated_latency_age_ms: number | null;
      latency_samples: Array<EstimatedLatencySnapshot & { elapsed_sec: number }>;
    };
    health: {
      display_resolution: string;
      display_fps: number;
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

async function writeReferenceBenchmark(page: Parameters<typeof pairThroughUi>[0], snapshot: ManagementLatencySnapshot): Promise<void> {
  const stream = snapshot.management.active_stream;
  if (!stream) throw new Error('Active stream disappeared before the reference benchmark was written');
  const warmupSeconds = 5;
  const measurementSeconds = 10;
  const totals = stream.latency_samples
    .filter(sample => sample.elapsed_sec >= warmupSeconds && sample.elapsed_sec < warmupSeconds + measurementSeconds)
    .map(sample => sample.total_ms)
    .filter(value => Number.isFinite(value) && value > 0);
  expect(totals.length).toBeGreaterThanOrEqual(8);
  expect(stream.sequence_gaps).toBe(0);
  expect(stream.config.codec).toBe('H265');
  expect(stream.config.fps).toBe(30);
  expect(stream.config.latency_mode).toBe('ULL');
  const browser = await page.evaluate(() => ({
    userAgent: navigator.userAgent,
    platform: navigator.platform,
  }));
  const receiver = receiverSystemInfo();
  const round = (value: number): number => Math.round(value * 10) / 10;
  const averageMs = totals.reduce((sum, value) => sum + value, 0) / totals.length;
  writeJsonArtifact('performance-summary.json', {
    schema_version: 2,
    metric: 'average_estimated_encoder_input_to_display_ms',
    measured_at: new Date().toISOString(),
    environment: {
      browser_user_agent: browser.userAgent,
      sender_platform: browser.platform,
      receiver: 'Radxa ROCK 4C+ / RK3399',
      receiver_kernel: receiver.kernel,
      receiver_architecture: receiver.architecture,
      network_type: process.env.E2E_NETWORK_TYPE || 'unknown',
      receiver_interface: process.env.E2E_RECEIVER_INTERFACE || 'unknown',
      display_resolution: snapshot.management.health.display_resolution,
      display_fps: snapshot.management.health.display_fps,
    },
    configuration: {
      source: 'synthetic',
      codec: stream.config.codec,
      selected_resolution: '1920x1080',
      encoded_resolution: stream.config.encoded_resolution,
      fps: stream.config.fps,
      bitrate_selection: 'auto',
      configured_bitrate_mbps: stream.config.bitrate_mbps,
      latency_mode: stream.config.latency_mode,
      aspect_mode: stream.config.aspect_mode,
    },
    sample_count: totals.length,
    warmup_seconds: warmupSeconds,
    measurement_seconds: measurementSeconds,
    sample_kind: 'unsmoothed_phase_estimate',
    average_ms: round(averageMs),
    sequence_gaps: stream.sequence_gaps,
  });
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
  await expect(page.locator('#statLatencyDetail')).toHaveText('Synchronized encoder-input-to-display estimate');
  await expect.poll(async () => {
    const response = await page.request.get(`https://${adminIp()}:9090/api/snapshot`);
    if (!response.ok()) return null;
    const snapshot = await response.json() as ManagementLatencySnapshot;
    return snapshot.management.active_stream?.estimated_latency?.total_ms ?? null;
  }, { timeout: 10_000 }).not.toBeNull();
  const managementResponse = await page.request.get(`https://${adminIp()}:9090/api/snapshot`);
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
    managementLatency.encode_ms + managementLatency.sender_queue_ms + managementLatency.delivery_ms
      + managementLatency.receiver_queue_ms + managementLatency.decode_display_ms,
    5,
  );
  expect(managementLatencySamples.length).toBeGreaterThan(0);
  expect(managementLatency.transport_queue_ms).toBeCloseTo(
    managementLatency.sender_queue_ms + managementLatency.delivery_ms + managementLatency.receiver_queue_ms, 5,
  );
  expect(managementLatency.clock_uncertainty_ms).toBeGreaterThanOrEqual(0);
  expect(managementLatency.adaptive_bitrate_mbps).toBeLessThanOrEqual(managementLatency.configured_bitrate_mbps);
  expect(managementLatency.effective_fps).toBeGreaterThan(0);
  expect(activeStream.estimated_latency_age_ms).not.toBeNull();
  expect(activeStream.estimated_latency_age_ms ?? Number.POSITIVE_INFINITY).toBeLessThan(3_000);
  const graphedLatency = managementLatencySamples.at(-1);
  expect(graphedLatency?.total_ms).toBeGreaterThan(0);
  expect(graphedLatency?.total_ms).toBeLessThan(5_000);
  if (testCase.name === 'hevc-1080p' && cycle === 1) {
    await expect.poll(async () => {
      const response = await page.request.get(`https://${adminIp()}:9090/api/snapshot`);
      if (!response.ok()) return 0;
      const snapshot = await response.json() as ManagementLatencySnapshot;
      return snapshot.management.active_stream?.latency_samples.at(-1)?.elapsed_sec ?? 0;
    }, { timeout: 30_000 }).toBeGreaterThanOrEqual(15);
    const benchmarkResponse = await page.request.get(`https://${adminIp()}:9090/api/snapshot`);
    expect(benchmarkResponse.ok()).toBe(true);
    const benchmarkSnapshot = await benchmarkResponse.json() as ManagementLatencySnapshot;
    await writeReferenceBenchmark(page, benchmarkSnapshot);

    const initialLatencySampleCount = benchmarkSnapshot.management.active_stream?.latency_samples.length ?? 0;
    const portal = await page.context().newPage();
    await portal.goto(`https://${adminIp()}:9090/`, { waitUntil: 'domcontentloaded' });
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
    await expect(portal.locator('#latencyMetrics')).not.toContainText('Measuring…');
    await expect(portal.locator('#congestionMetrics')).not.toContainText('Measuring…');
    await portal.waitForTimeout(5_000);
    const sustainedResponse = await portal.request.get(`https://${adminIp()}:9090/api/snapshot`);
    expect(sustainedResponse.ok()).toBe(true);
    const sustainedSnapshot = await sustainedResponse.json() as ManagementLatencySnapshot;
    const sustainedStream = sustainedSnapshot.management.active_stream;
    if (!sustainedStream) throw new Error('Active stream disappeared during sustained latency sampling');
    expect(sustainedStream.latency_samples.length).toBeGreaterThanOrEqual(initialLatencySampleCount + 3);
    expect(sustainedStream.estimated_latency_age_ms).not.toBeNull();
    expect(sustainedStream.estimated_latency_age_ms ?? Number.POSITIVE_INFINITY).toBeLessThan(3_000);
    await expect(portal.locator('#latencyMetrics')).not.toContainText('Measuring…');
    await expect(portal.locator('#congestionMetrics')).not.toContainText('Measuring…');
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
    const response = await page.request.get(`https://${adminIp()}:9090/api/snapshot`);
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
  await expect(page.locator('#toggleBtn')).toBeEnabled();
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
    await expect(page.locator('link[href^="http"], script[src^="http"], img[src^="http"]')).toHaveCount(0);
    await expect(page.locator('header .subtitle')).toHaveText('Private, low-latency casting to HDMI');

    await page.setViewportSize({ width: 390, height: 844 });
    await expect(page.locator('#pairingTitle')).toBeVisible();
    await expect(page.locator('#toggleBtn')).toBeVisible();
    await saveUiScreenshot(page, 'casting-mobile.png');
    await page.setViewportSize({ width: 1280, height: 900 });
    await saveUiScreenshot(page, 'casting-desktop.png');

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
