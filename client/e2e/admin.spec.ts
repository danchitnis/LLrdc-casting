import { expect, test } from '@playwright/test';
import { assertDiagnosticsClean, trackDiagnostics, writeDiagnostics } from './support';

const boardIp = process.env.E2E_BOARD_IP;

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
});
