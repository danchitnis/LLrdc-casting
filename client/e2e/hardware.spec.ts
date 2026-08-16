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
  await expect(page.locator('#statCodec')).toHaveText(
    testCase.senderCodec === 'H265' ? 'HEVC / H.265' : (testCase.senderCodec === 'H264_SW' ? 'H.264 (Software)' : 'H.264'),
  );
  await expect(page.locator('#statEncoderHW')).toContainText(testCase.senderCodec === 'H264_SW' ? 'SW Emulated' : 'HW Preferred');
  await waitFor(
    () => page.locator('#statFrameCount').textContent(),
    value => Number.parseInt(value || '0', 10) >= initialFrameCount + 30,
    `${testCase.name} cycle ${cycle} to increase the frame counter by 30`,
    45_000,
  );

  const afterFrames = await receiver.waitFor(
    log => receiverDelta(before, log).includes(`[PLAYBACK] submitted_${testCase.wireCodec}_access_units=1`),
    `${testCase.name} cycle ${cycle} receiver playback`,
    45_000,
  );
  assertReceiverDelta(receiverDelta(before, afterFrames), testCase.wireCodec);

  await page.locator('#toggleBtn').click();
  await expect(page.locator('#statusBadge')).toHaveText('CONNECTED', { timeout: 30_000 });
  await expect(page.locator('#toggleText')).toHaveText('Start Casting');
  await expect(page.locator('#settingsLockNotice')).toBeHidden();
  await expect(page.locator('#log')).toContainText('[STOPPED] Casting session closed.');
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
    await expect(page.locator('#statProtocol')).toHaveText('WebTransport / QUIC');
    await expect.poll(async () => page.locator('#statSignal').textContent()).not.toBe('--');
    await expect.poll(async () => page.locator('#statDevicePing').textContent()).toMatch(/^\d+ ms$/);
    await expect(page.locator('#log')).toContainText('[WEBTRANSPORT] Connected directly to receiver over the LAN.');

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
