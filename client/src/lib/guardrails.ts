export function updateCodecAndResolutionGuardrails(
  logFn?: (msg: string, isError?: boolean) => void
): void {
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  const resSelect = document.getElementById('resolution') as HTMLSelectElement | null;
  if (!codecSelect || !resSelect) return;

  const selectedCodec = codecSelect.value;
  const res2kOption = resSelect.querySelector('option[value="2560x1440"]') as HTMLOptionElement | null;
  const res4kOption = resSelect.querySelector('option[value="3840x2160"]') as HTMLOptionElement | null;

  if (selectedCodec === 'H264') {
    if (res2kOption) res2kOption.disabled = true;
    if (res4kOption) res4kOption.disabled = true;

    if (resSelect.value === '2560x1440' || resSelect.value === '3840x2160') {
      resSelect.value = '1920x1080';
      logFn?.('[GUARDRAIL] H.264 supports up to 1080p on RK3399 hardware decoder; adjusted to 1080p');
    }
  } else {
    if (res2kOption) res2kOption.disabled = false;
    if (res4kOption) res4kOption.disabled = false;
  }
}

export function onResolutionChange(logFn?: (msg: string, isError?: boolean) => void): void {
  const resSelect = document.getElementById('resolution') as HTMLSelectElement | null;
  if (!resSelect) return;
  const resStr = resSelect.value;
  const [w] = resStr.split('x').map(n => parseInt(n, 10));
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  if (codecSelect) {
    if (w >= 2560) {
      codecSelect.value = 'H265';
    }
  }
  updateCodecAndResolutionGuardrails(logFn);
}

export function onCodecChange(logFn?: (msg: string, isError?: boolean) => void): void {
  updateCodecAndResolutionGuardrails(logFn);
}

export function calculateTargetBitrate(
  bitrateSetting: string,
  codec: string,
  width: number,
  fps: number
): number {
  if (bitrateSetting && bitrateSetting !== 'auto') {
    const mbps = parseFloat(bitrateSetting);
    if (!isNaN(mbps) && mbps > 0) {
      return Math.round(mbps * 1_000_000);
    }
  }

  if (codec === 'H265') {
    if (width >= 3840) return fps >= 60 ? 25_000_000 : 15_000_000;
    if (width >= 2560) return fps >= 60 ? 14_000_000 : 9_000_000;
    if (width >= 1920) return fps >= 60 ? 10_000_000 : 6_000_000;
    return fps >= 60 ? 5_000_000 : 3_000_000;
  } else {
    if (width >= 1920) return fps >= 60 ? 16_000_000 : 10_000_000;
    return fps >= 60 ? 8_000_000 : 5_000_000;
  }
}

export function getCodecString(codec: string, width: number, _fps?: number): string {
  if (codec === 'H265') {
    return 'hev1.1.6.L150.B0';
  } else {
    if (width >= 3840) {
      return 'avc1.42e033';
    } else if (width >= 2560) {
      return 'avc1.42e032';
    }
    return 'avc1.42e028';
  }
}
