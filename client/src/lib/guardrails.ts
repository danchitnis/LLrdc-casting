import {
  BITRATE_THRESHOLDS,
  CODEC_CONFIG,
  CODEC_RESOLUTION_LIMITS,
  ENCODER_GUARDRAILS,
  FPS_THRESHOLDS,
  H264_CODEC_STRINGS,
  RESOLUTION_OPTIONS,
  STREAM_DEFAULTS,
} from './config.ts';

export interface CodecCapabilityStatus {
  h265Supported: boolean;
  h264HardwarePreferenceSupported: boolean;
  h264Supported: boolean;
}

export interface EncodedDimensions {
  width: number;
  height: number;
}

/**
 * Check a requested stream rate against the currently negotiated HDMI mode.
 * The driver/EDID may advertise faster modes, but they are not usable until
 * the receiver has actually negotiated one of those modes.
 */
export function isFrameRateWithinDisplayMode(
  requestedFps: number,
  displayRefreshFps?: number,
): boolean {
  return !displayRefreshFps || displayRefreshFps <= 0 || requestedFps <= displayRefreshFps;
}

/** Update the FPS selector from the receiver's active HDMI refresh rate. */
export function updateDisplayFpsGuardrails(displayRefreshFps?: number): void {
  const fpsSelect = document.getElementById('fps') as HTMLSelectElement | null;
  if (!fpsSelect || !displayRefreshFps || displayRefreshFps <= 0) return;

  Array.from(fpsSelect.options).forEach(option => {
    const optionFps = Number.parseInt(option.value, 10);
    if (!Number.isFinite(optionFps)) return;
    const allowed = isFrameRateWithinDisplayMode(optionFps, displayRefreshFps);
    option.disabled = !allowed;
    if (allowed && option.textContent?.includes('(Unsupported by display)')) {
      option.textContent = option.textContent.replace(' (Unsupported by display)', '');
    } else if (!allowed && !option.textContent?.includes('(Unsupported by display)')) {
      option.textContent = `${option.textContent || `${optionFps} FPS`} (Unsupported by display)`;
    }
  });

  const selected = Number.parseInt(fpsSelect.value, 10);
  if (!isFrameRateWithinDisplayMode(selected, displayRefreshFps)) {
    const fallback = Array.from(fpsSelect.options).find(option => !option.disabled);
    if (fallback) fpsSelect.value = fallback.value;
  }
}

export function isCodecResolutionAllowed(codec: string, dimensions: EncodedDimensions): boolean {
  return codec === 'H265'
    ? dimensions.width <= CODEC_RESOLUTION_LIMITS.H265_MAX_WIDTH
      && dimensions.height <= CODEC_RESOLUTION_LIMITS.H265_MAX_HEIGHT
    : dimensions.width <= CODEC_RESOLUTION_LIMITS.H264_MAX_WIDTH
      && dimensions.height <= CODEC_RESOLUTION_LIMITS.H264_MAX_HEIGHT;
}

export function alignEncoderDimensions(
  codec: string,
  width: number,
  height: number,
): EncodedDimensions {
  if (codec === 'H264' || codec === 'H264_SW') {
    return { width, height: Math.ceil(height / ENCODER_GUARDRAILS.ALIGNMENT) * ENCODER_GUARDRAILS.ALIGNMENT };
  }
  return {
    width: Math.ceil(width / ENCODER_GUARDRAILS.ALIGNMENT) * ENCODER_GUARDRAILS.ALIGNMENT,
    height: Math.ceil(height / ENCODER_GUARDRAILS.ALIGNMENT) * ENCODER_GUARDRAILS.ALIGNMENT,
  };
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
    const isSupported = status ? status.h265Supported : true;
    if (isSupported) {
      statEncoderHW.textContent = 'HW Preferred (Browser API)';
      statEncoderHW.style.color = '#10b981';
    } else {
      statEncoderHW.textContent = 'Unsupported';
      statEncoderHW.style.color = '#ef4444';
    }
  } else if (val === 'H264') {
    const isHWPreferred = status ? status.h264HardwarePreferenceSupported : true;
    if (isHWPreferred) {
      statEncoderHW.textContent = 'HW Preferred (Browser API)';
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
    h265Supported: false,
    h264HardwarePreferenceSupported: false,
    h264Supported: false
  };

  if (typeof VideoEncoder === 'undefined' || typeof VideoEncoder.isConfigSupported !== 'function') {
    logFn?.('[CAPABILITY] WebCodecs VideoEncoder API not available in this browser', true);
    updateEncoderHWStatus(result);
    return result;
  }

  // Probe H.265 with the browser's hardware preference. WebCodecs exposes
  // preference, not a portable post-encode hardware confirmation API.
  try {
    const h265Config: VideoEncoderConfig = {
      codec: CODEC_CONFIG.H265.capabilityCodec,
      width: CODEC_CONFIG.H265.capabilityWidth,
      height: CODEC_CONFIG.H265.capabilityHeight,
      bitrate: CODEC_CONFIG.H265.capabilityBitrate,
      framerate: CODEC_CONFIG.H265.capabilityFramerate,
      hardwareAcceleration: ENCODER_GUARDRAILS.HARDWARE_ACCELERATION,
    };
    const resH265 = await VideoEncoder.isConfigSupported(h265Config);
    if (resH265.supported) {
      result.h265Supported = true;
    }
  } catch (e) {}

  // Probe H.264 hardware and software independently.
  try {
    const h264Config: VideoEncoderConfig = {
      codec: CODEC_CONFIG.H264.capabilityCodec,
      width: CODEC_CONFIG.H264.capabilityWidth,
      height: CODEC_CONFIG.H264.capabilityHeight,
      bitrate: CODEC_CONFIG.H264.capabilityBitrate,
      framerate: CODEC_CONFIG.H264.capabilityFramerate,
      hardwareAcceleration: ENCODER_GUARDRAILS.HARDWARE_ACCELERATION,
    };
    const resH264 = await VideoEncoder.isConfigSupported(h264Config);
    if (resH264.supported) {
      result.h264HardwarePreferenceSupported = true;
    }
  } catch (e) {}

  try {
    const h264SoftwareConfig: VideoEncoderConfig = {
      codec: CODEC_CONFIG.H264.capabilityCodec,
      width: CODEC_CONFIG.H264.capabilityWidth,
      height: CODEC_CONFIG.H264.capabilityHeight,
      bitrate: CODEC_CONFIG.H264.capabilityBitrate,
      framerate: CODEC_CONFIG.H264.capabilityFramerate,
      hardwareAcceleration: ENCODER_GUARDRAILS.SOFTWARE_ACCELERATION,
    };
    const resH264Software = await VideoEncoder.isConfigSupported(h264SoftwareConfig);
    result.h264Supported = !!resH264Software.supported;
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
      } else {
        h265Option.disabled = false;
        h265Option.textContent = 'HEVC / H.265 (Hardware Preferred)';
      }
    }

    if (h264Option) {
      if (result.h264HardwarePreferenceSupported) {
        h264Option.textContent = 'H.264 (Hardware Preferred)';
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

  logFn?.(`[CAPABILITIES] H.265: ${result.h265Supported ? 'SUPPORTED' : 'UNAVAILABLE'} | H.264 HW preference: ${result.h264HardwarePreferenceSupported ? 'SUPPORTED' : 'UNAVAILABLE'} | H.264 SW: ${result.h264Supported ? 'AVAILABLE' : 'UNAVAILABLE'}`);

  return result;
}

export function updateCodecAndResolutionGuardrails(
  logFn?: (msg: string, isError?: boolean) => void
): void {
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  const resSelect = document.getElementById('resolution') as HTMLSelectElement | null;
  if (!codecSelect || !resSelect) return;

  const selectedCodec = codecSelect.value;
  const isH264 = selectedCodec === 'H264' || selectedCodec === 'H264_SW';
  Array.from(resSelect.options).forEach(option => {
    const resolution = RESOLUTION_OPTIONS.find(candidate => candidate.value === option.value);
    if (!resolution) return;
    const encodedDimensions = alignEncoderDimensions(selectedCodec, resolution.width, resolution.height);
    option.disabled = isH264 && !isCodecResolutionAllowed(selectedCodec, encodedDimensions);
  });

  if (isH264) {
    const selectedOption = resSelect.selectedOptions[0];
    if (selectedOption?.disabled) {
      resSelect.value = STREAM_DEFAULTS.resolution;
      logFn?.('[GUARDRAIL] H.264 supports up to 1080p on RK3399 hardware decoder; adjusted to 1080p');
    }
  }
}

export function onResolutionChange(logFn?: (msg: string, isError?: boolean) => void): void {
  const resSelect = document.getElementById('resolution') as HTMLSelectElement | null;
  if (!resSelect) return;
  const resStr = resSelect.value;
  const [w = Number.parseInt(STREAM_DEFAULTS.resolution.split('x')[0], 10)] = resStr.split('x').map(n => parseInt(n, 10));
  const codecSelect = document.getElementById('codec') as HTMLSelectElement | null;
  if (codecSelect && w >= CODEC_RESOLUTION_LIMITS.H265_AUTO_MIN_WIDTH) {
    const h265Option = codecSelect.querySelector('option[value="H265"]') as HTMLOptionElement | null;
    if (h265Option && !h265Option.disabled) codecSelect.value = 'H265';
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
    const threshold = BITRATE_THRESHOLDS.H265.find(entry => width >= entry.minWidth);
    if (threshold) return fps >= FPS_THRESHOLDS.HIGH ? threshold.at60 : threshold.below60;
    return fps >= FPS_THRESHOLDS.HIGH ? BITRATE_THRESHOLDS.fallback.at60 : BITRATE_THRESHOLDS.fallback.below60;
  }
  const threshold = BITRATE_THRESHOLDS.H264.find(entry => width >= entry.minWidth);
  if (threshold) return fps >= FPS_THRESHOLDS.HIGH ? threshold.at60 : threshold.below60;
  return fps >= FPS_THRESHOLDS.HIGH ? BITRATE_THRESHOLDS.H264Fallback.at60 : BITRATE_THRESHOLDS.H264Fallback.below60;
}

export function getCodecString(codec: string, fps: number = STREAM_DEFAULTS.fps): string {
  if (codec === 'H265') {
    return CODEC_CONFIG.H265.defaultCodec;
  }
  return fps >= FPS_THRESHOLDS.HIGH ? H264_CODEC_STRINGS.HD60 : H264_CODEC_STRINGS.HD;
}
