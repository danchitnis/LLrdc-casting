export interface CodecCapabilityStatus {
  h265HW: boolean;
  h265Supported: boolean;
  h264HW: boolean;
  h264Supported: boolean;
}

let lastCapabilityStatus: CodecCapabilityStatus | null = null;

export function updateEncoderHWStatus(caps?: CodecCapabilityStatus | null): void {
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  const statEncoderHW = document.getElementById('statEncoderHW');
  if (!codecSelect || !statEncoderHW) return;

  const status = caps || lastCapabilityStatus;
  const val = codecSelect.value;

  if (val === 'H264_SW') {
    statEncoderHW.textContent = 'SW Emulated (CPU)';
    statEncoderHW.style.color = '#f59e0b';
  } else if (val === 'H265') {
    const isHW = status ? status.h265HW : true;
    if (isHW) {
      statEncoderHW.textContent = 'HW Accelerated (GPU)';
      statEncoderHW.style.color = '#10b981';
    } else {
      statEncoderHW.textContent = 'HW Unavailable (CPU Blocked)';
      statEncoderHW.style.color = '#ef4444';
    }
  } else if (val === 'H264') {
    const isHW = status ? status.h264HW : true;
    if (isHW) {
      statEncoderHW.textContent = 'HW Accelerated (GPU)';
      statEncoderHW.style.color = '#10b981';
    } else {
      statEncoderHW.textContent = 'SW Emulated (CPU)';
      statEncoderHW.style.color = '#f59e0b';
    }
  }
}

export async function checkBrowserCodecCapabilities(
  logFn?: (msg: string, isError?: boolean) => void
): Promise<CodecCapabilityStatus> {
  const result: CodecCapabilityStatus = {
    h265HW: false,
    h265Supported: false,
    h264HW: false,
    h264Supported: false
  };

  if (typeof VideoEncoder === 'undefined' || typeof VideoEncoder.isConfigSupported !== 'function') {
    logFn?.('[CAPABILITY] WebCodecs VideoEncoder API not available in this browser', true);
    updateEncoderHWStatus(result);
    return result;
  }

  // Probe H.265 (HEVC)
  try {
    const h265Config: VideoEncoderConfig = {
      codec: 'hev1.1.6.L150.B0',
      width: 1920,
      height: 1080,
      bitrate: 6_000_000,
      framerate: 30,
      hardwareAcceleration: 'prefer-hardware'
    };
    const resH265 = await VideoEncoder.isConfigSupported(h265Config);
    if (resH265.supported) {
      result.h265Supported = true;
      result.h265HW = resH265.config?.hardwareAcceleration === 'prefer-hardware' || resH265.supported;
    }
  } catch (e) {}

  // Probe H.264 (AVC)
  try {
    const h264Config: VideoEncoderConfig = {
      codec: 'avc1.42e028',
      width: 1920,
      height: 1080,
      bitrate: 8_000_000,
      framerate: 30,
      hardwareAcceleration: 'prefer-hardware'
    };
    const resH264 = await VideoEncoder.isConfigSupported(h264Config);
    if (resH264.supported) {
      result.h264Supported = true;
      result.h264HW = resH264.config?.hardwareAcceleration === 'prefer-hardware' || resH264.supported;
    }
  } catch (e) {}

  lastCapabilityStatus = result;

  // Update dropdown option for H.265 & H.264
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  if (codecSelect) {
    const h265Option = codecSelect.querySelector('option[value="H265"]') as HTMLOptionElement | null;
    const h264Option = codecSelect.querySelector('option[value="H264"]') as HTMLOptionElement | null;
    const h264SwOption = codecSelect.querySelector('option[value="H264_SW"]') as HTMLOptionElement | null;

    if (h265Option) {
      if (!result.h265Supported) {
        h265Option.disabled = true;
        h265Option.textContent = 'HEVC / H.265 (Unsupported)';
        if (codecSelect.value === 'H265') {
          codecSelect.value = 'H264';
          logFn?.('[GUARDRAIL] Browser lacks H.265 encoding support. Auto-switched to H.264');
        }
      } else if (!result.h265HW) {
        h265Option.disabled = true;
        h265Option.textContent = 'HEVC / H.265 (HW Unavailable - CPU Blocked)';
        if (codecSelect.value === 'H265') {
          codecSelect.value = 'H264';
          logFn?.('[GUARDRAIL] Browser lacks H.265 hardware acceleration. H.265 software encoding disabled (high CPU load); auto-switched to H.264');
        }
      } else {
        h265Option.disabled = false;
        h265Option.textContent = 'HEVC / H.265 (Hardware Accelerated)';
      }
    }

    if (h264Option) {
      if (result.h264HW) {
        h264Option.textContent = 'H.264 (Hardware Accelerated)';
      } else if (result.h264Supported) {
        h264Option.textContent = 'H.264 (Software Emulated)';
      } else {
        h264Option.disabled = true;
        h264Option.textContent = 'H.264 (Unsupported)';
      }
    }

    if (h264SwOption) {
      if (result.h264Supported) {
        h264SwOption.disabled = false;
        h264SwOption.textContent = 'H.264 (Software Emulated)';
      } else {
        h264SwOption.disabled = true;
        h264SwOption.textContent = 'H.264 Software (Unsupported)';
      }
    }
  }

  updateCodecAndResolutionGuardrails(logFn);
  updateEncoderHWStatus(result);

  logFn?.(`[CAPABILITIES] H.265 HW: ${result.h265HW ? 'AVAILABLE' : 'UNAVAILABLE'} | H.264 HW: ${result.h264HW ? 'AVAILABLE' : 'UNAVAILABLE'} | H.264 SW: ${result.h264Supported ? 'AVAILABLE' : 'UNAVAILABLE'}`);

  return result;
}

export function updateCodecAndResolutionGuardrails(
  logFn?: (msg: string, isError?: boolean) => void
): void {
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  const resSelect = document.getElementById('resolution') as HTMLSelectElement | null;
  if (!codecSelect || !resSelect) return;

  const selectedCodec = codecSelect.value;
  const res2kOption = resSelect.querySelector('option[value="2560x1440"]') as HTMLOptionElement | null;
  const res4kOption = resSelect.querySelector('option[value="3840x2160"]') as HTMLOptionElement | null;
  if (selectedCodec === 'H264' || selectedCodec === 'H264_SW') {
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
  const [w = 1920] = resStr.split('x').map(n => parseInt(n, 10));
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  if (codecSelect) {
    if (w >= 2560) {
      const h265Option = codecSelect.querySelector('option[value="H265"]') as HTMLOptionElement | null;
      if (h265Option && !h265Option.disabled) {
        codecSelect.value = 'H265';
      }
    }
  }
  updateCodecAndResolutionGuardrails(logFn);
  updateEncoderHWStatus();
}

export function onCodecChange(logFn?: (msg: string, isError?: boolean) => void): void {
  updateCodecAndResolutionGuardrails(logFn);
  updateEncoderHWStatus();
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
