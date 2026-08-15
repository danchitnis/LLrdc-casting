/** Typed frontend defaults, WebCodecs limits, and transport constants. */

export type VideoSource = 'screen' | 'synthetic';
export type ResolutionValue = '1280x720' | '1920x1080' | '2560x1440' | '3840x2160';
export type AspectMode = 'preserve' | 'stretch';
export type CodecOption = 'H265' | 'H264' | 'H264_SW';
export type BitrateOption = 'auto' | '3' | '8' | '16' | '30';
export type LatencyMode = 'ULL' | 'balanced' | 'quality';

export interface StreamDefaults {
  readonly videoSource: VideoSource;
  readonly resolution: ResolutionValue;
  readonly aspectMode: AspectMode;
  readonly fps: 30 | 60;
  readonly codec: CodecOption;
  readonly bitrate: BitrateOption;
  readonly latency: LatencyMode;
}

export interface ResolutionOption {
  readonly value: ResolutionValue;
  readonly label: string;
  readonly width: number;
  readonly height: number;
}

export const STREAM_DEFAULTS = {
  videoSource: 'screen',
  resolution: '1920x1080',
  aspectMode: 'preserve',
  fps: 30,
  codec: 'H265',
  bitrate: 'auto',
  latency: 'ULL',
} as const satisfies StreamDefaults;

export const VIDEO_SOURCE_OPTIONS = [
  { value: 'screen', label: 'Screen Capture (getDisplayMedia)' },
  { value: 'synthetic', label: 'Bouncing Orb / Test Pattern (Synthetic)' },
] as const satisfies readonly { value: VideoSource; label: string }[];

export const ASPECT_OPTIONS = [
  { value: 'preserve', label: 'Preserve Laptop Aspect Ratio' },
  { value: 'stretch', label: 'Stretch to HDMI Display' },
] as const satisfies readonly { value: AspectMode; label: string }[];

export const RESOLUTION_OPTIONS = [
  { value: '1280x720', label: '720p (1280x720)', width: 1280, height: 720 },
  { value: '1920x1080', label: '1080p (1920x1080)', width: 1920, height: 1080 },
  { value: '2560x1440', label: '1440p (2560x1440)', width: 2560, height: 1440 },
  { value: '3840x2160', label: '2160p / 4K UHD (3840x2160)', width: 3840, height: 2160 },
] as const satisfies readonly ResolutionOption[];

export const FPS_OPTIONS = [30, 60] as const satisfies readonly (30 | 60)[];

export const CODEC_OPTIONS = [
  { value: 'H265', label: 'H.265 / HEVC' },
  { value: 'H264', label: 'H.264' },
  { value: 'H264_SW', label: 'H.264 (software)' },
] as const satisfies readonly { value: CodecOption; label: string }[];

export const BITRATE_OPTIONS = [
  { value: 'auto', label: 'Auto' },
  { value: '3', label: '3 Mbps' },
  { value: '8', label: '8 Mbps' },
  { value: '16', label: '16 Mbps' },
  { value: '30', label: '30 Mbps' },
] as const satisfies readonly { value: BitrateOption; label: string }[];

export const LATENCY_OPTIONS = [
  { value: 'ULL', label: 'Ultra-low latency' },
  { value: 'balanced', label: 'Balanced' },
  { value: 'quality', label: 'Quality' },
] as const satisfies readonly { value: LatencyMode; label: string }[];

export const CODEC_CONFIG = {
  H265: {
    capabilityCodec: 'hev1.1.6.L150.B0',
    defaultCodec: 'hev1.1.6.L150.B0',
    capabilityWidth: 1920,
    capabilityHeight: 1088,
    capabilityBitrate: 6_000_000,
    capabilityFramerate: 30,
  },
  H264: {
    capabilityCodec: 'avc1.42e028',
    defaultCodec: 'avc1.42e028',
    capabilityWidth: 1920,
    capabilityHeight: 1088,
    capabilityBitrate: 8_000_000,
    capabilityFramerate: 30,
  },
} as const;

export const H264_CODEC_STRINGS = {
  UHD: 'avc1.42e033',
  QHD: 'avc1.42e032',
  HD60: 'avc1.42e02a',
  HD: 'avc1.42e028',
} as const;

export const CODEC_RESOLUTION_LIMITS = {
  H264_MAX_WIDTH: 1920,
  H264_MAX_HEIGHT: 1088,
  H265_MAX_WIDTH: 3840,
  H265_MAX_HEIGHT: 2160,
  H265_AUTO_MIN_WIDTH: 2560,
} as const;

export const FPS_THRESHOLDS = {
  HIGH: 60,
} as const;

export const DECODER_LIMITS = {
  MAX_ACCESS_UNIT_BYTES: 8 * 1024 * 1024,
} as const;

export const ENCODER_GUARDRAILS = {
  ALIGNMENT: 16,
  MIN_PERIODIC_KEYFRAME_INTERVAL: 10,
  INITIAL_KEYFRAME_COUNT: 5,
  MAX_ENCODER_QUEUE: 8,
  FRAME_TIMING_SLACK_MS: 2,
  // WebCodecs exposes the portable preference values; hardware use is
  // enforced for HEVC by the capability check below.
  HARDWARE_ACCELERATION: 'prefer-hardware',
  SOFTWARE_ACCELERATION: 'prefer-software',
} as const;

export const BITRATE_THRESHOLDS = {
  H265: [
    { minWidth: 3840, at60: 25_000_000, below60: 15_000_000 },
    { minWidth: 2560, at60: 14_000_000, below60: 9_000_000 },
    { minWidth: 1920, at60: 10_000_000, below60: 6_000_000 },
  ],
  H264: [
    { minWidth: 1920, at60: 16_000_000, below60: 10_000_000 },
  ],
  fallback: { at60: 5_000_000, below60: 3_000_000 },
  H264Fallback: { at60: 8_000_000, below60: 5_000_000 },
} as const;

export const TRANSPORT_CONFIG = {
  PACKET_HEADER_BYTES: 16,
  CHUNK_BYTES: 1350,
  TAG_BYTES: 4,
  CONTROL_LENGTH_PREFIX_BYTES: 4,
  MAX_CONTROL_MESSAGE_BYTES: 64 * 1024,
  STOP_PACKET_BYTES: 20,
  STOP_TAG: [83, 84, 79, 80],
  SINGLE_PACKET_CHUNK_COUNT: 1,
  KEEPALIVE_INTERVAL_MS: 1000,
  GEOMETRY_TIMEOUT_MS: 5000,
  GEOMETRY_POLL_INTERVAL_MS: 100,
} as const;

export const QUEUE_CONFIG = {
  MAX_ENCODER_QUEUE: ENCODER_GUARDRAILS.MAX_ENCODER_QUEUE,
  FRAME_TIMING_SLACK_MS: ENCODER_GUARDRAILS.FRAME_TIMING_SLACK_MS,
} as const;

export const SYNTHETIC_PATTERN_CONFIG = {
  GRID_STEP: 100,
  GRID_LINE_WIDTH: 2,
  ORB_X_SPEED: 2,
  ORB_Y_SPEED: 3,
  ORB_RADIUS: 120,
  GRADIENT_RADIUS: 10,
  LABEL_MARGIN_MAX_PX: 24,
  LABEL_MARGIN_RATIO: 0.05,
  TITLE_MAX_PX: 24,
  TITLE_RATIO: 0.045,
  DETAIL_MIN_PX: 18,
  DETAIL_RATIO: 0.62,
  LINE_HEIGHT: 1.5,
} as const;

export const ANNEXB_CONFIG = {
  DECODER_DESCRIPTION_MIN_BYTES: 7,
  HEVC_NUM_ARRAYS_OFFSET: 22,
  HEVC_ARRAY_HEADER_BYTES: 3,
  AVC_NUM_SPS_OFFSET: 5,
  NAL_LENGTH_BYTES: 2,
  NAL_LENGTH_PREFIX_BYTES: 4,
  START_CODE: [0x00, 0x00, 0x00, 0x01],
  START_CODE_3: 3,
  START_CODE_4: 4,
  H264_SPS_TYPE: 7,
  H264_PPS_TYPE: 8,
  H265_VPS_TYPE: 32,
  H265_SPS_TYPE: 33,
  H265_PPS_TYPE: 34,
  H264_AUD: 0x09,
  H265_AUD: 35,
} as const;

export const PAIRING_CONFIG = {
  CODE_LENGTH: 4,
  CODE_PATTERN: /^[A-Z0-9]{4}$/,
  DIRECT_WEBTRANSPORT_PORT: 4433,
} as const;
