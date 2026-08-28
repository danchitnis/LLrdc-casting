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
} from './support';

function receiverDelta(before: string, after: string): string {
  return after.startsWith(before) ? after.slice(before.length) : after;
}

test.describe('cloud pairing handoff', () => {
  test.beforeEach(async ({ page }) => {
    trackDiagnostics(page);
    await page.context().grantPermissions(
      ['local-network-access'],
      { origin: 'https://cast.llrdc.com' },
    );
    await page.goto('https://cast.llrdc.com/', { waitUntil: 'domcontentloaded' });
    await page.locator('#pair-code').waitFor({ state: 'visible', timeout: 45_000 });
  });

  test.afterEach(async ({ page }, testInfo) => {
    await stopPostPairTrace(page.context());
    const diagnostics = diagnosticsFor(page);
    if (diagnostics) writeDiagnostics(diagnostics, 'cloud-browser');
    if (testInfo.status !== testInfo.expectedStatus) {
      try { await saveFailureScreenshot(page, testInfo); } catch { /* page may already be closed */ }
    } else if (diagnostics) {
      assertDiagnosticsClean(diagnostics);
    }
  });

  test('pairs through cast.llrdc.com and hands off one HEVC stream to LAN', async ({ page }, testInfo) => {
    const receiver = receiverLogs();
    const pairRequests: string[] = [];
    page.on('request', request => {
      if (request.url().includes('/api/pair')) pairRequests.push(request.url());
    });

    await pairThroughUi(page, process.env.E2E_PAIRING_CODE || '');
    expect(page.url()).toMatch(/^https:\/\/cast\.llrdc\.com/);
    expect(pairRequests).toHaveLength(1);
    await receiver.waitFor(
      log => log.includes('[WEBTRANSPORT] Client connected successfully via QUIC/UDP!'),
      'direct LAN WebTransport handoff',
      30_000,
    );
    await expect(page.locator('#statusBadge')).toHaveText('CONNECTED');
    await expect.poll(async () => page.locator('#statSignal').textContent()).not.toBe('--');

    await assertCodecSupport(page, 'H265', '1920x1080');
    await chooseCodec(page, 'H265', '1920x1080');
    await startPostPairTrace(page.context(), newTracePath(testInfo, 'cloud'));

    const before = receiver.read();
    const initialFrameCount = Number.parseInt((await page.locator('#statFrameCount').textContent()) || '0', 10);
    await page.locator('#toggleBtn').click();
    await expect(page.locator('#statusBadge')).toHaveText('STREAMING', { timeout: 30_000 });
    await expect(page.locator('#settingsLockNotice')).toBeVisible();
    await expect(page.locator('#statCodec')).toHaveText('HEVC / H.265');
    await expect(page.locator('#statEncoderHW')).toContainText('HW Preferred');
    await waitFor(
      () => page.locator('#statFrameCount').textContent(),
      value => Number.parseInt(value || '0', 10) >= initialFrameCount + 30,
      'cloud HEVC cycle to increase the frame counter by 30',
      45_000,
    );
    const afterFrames = await receiver.waitFor(
      log => receiverDelta(before, log).includes('[PLAYBACK] submitted_hevc_access_units=1'),
      'cloud HEVC receiver playback',
      45_000,
    );
    assertReceiverDelta(receiverDelta(before, afterFrames), 'hevc');

    await page.locator('#toggleBtn').click();
    await expect(page.locator('#statusBadge')).toHaveText('CONNECTED', { timeout: 30_000 });
    await expect(page.locator('#toggleText')).toHaveText('Start Casting');
    await expect(page.locator('#settingsLockNotice')).toBeHidden();
  });
});
