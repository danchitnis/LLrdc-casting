/** Typed frontend defaults, WebCodecs limits, and transport constants. */

const STANDARD_FPS = 30;
const HIGH_FPS = 60;
const CODEC_ALIGNMENT = 16;

function alignUp(value: number, alignment: number): number {
  return Math.ceil(value / alignment) * alignment;
}

export const VIDEO_SOURCE_OPTIONS = [
  { value: 'screen', label: 'Screen, window, or tab' },
  { value: 'synthetic', label: 'Built-in test pattern' },
] as const satisfies readonly { value: string; label: string }[];

export const ASPECT_OPTIONS = [
  { value: 'preserve', label: 'Preserve source proportions' },
  { value: 'stretch', label: 'Fill HDMI display' },
] as const satisfies readonly { value: string; label: string }[];

export const RESOLUTION_PRESETS = {
  HD: { value: '1280x720', label: '720p (1280x720)', width: 1280, height: 720 },
  FULL_HD: { value: '1920x1080', label: '1080p (1920x1080)', width: 1920, height: 1080 },
  QHD: { value: '2560x1440', label: '1440p (2560x1440)', width: 2560, height: 1440 },
  UHD: { value: '3840x2160', label: '2160p / 4K UHD (3840x2160)', width: 3840, height: 2160 },
} as const satisfies Record<string, { value: string; label: string; width: number; height: number }>;

export const RESOLUTION_OPTIONS = [
  RESOLUTION_PRESETS.HD,
  RESOLUTION_PRESETS.FULL_HD,
  RESOLUTION_PRESETS.QHD,
  RESOLUTION_PRESETS.UHD,
] as const;

export const FPS_OPTIONS = [STANDARD_FPS, HIGH_FPS] as const satisfies readonly number[];

export const CODEC_OPTIONS = [
  { value: 'H265', label: 'HEVC / H.265' },
  { value: 'H264', label: 'H.264' },
  { value: 'H264_SW', label: 'H.264 (prefer software)' },
] as const satisfies readonly { value: string; label: string }[];

export const BITRATE_OPTIONS = [
  { value: 'auto', label: 'Auto' },
  { value: '3', label: '3 Mbps' },
  { value: '8', label: '8 Mbps' },
  { value: '16', label: '16 Mbps' },
  { value: '30', label: '30 Mbps' },
] as const satisfies readonly { value: string; label: string }[];

export const LATENCY_OPTIONS = [
  { value: 'ULL', label: 'Lowest latency' },
  { value: 'balanced', label: 'Balanced' },
  { value: 'quality', label: 'Best quality' },
] as const satisfies readonly { value: string; label: string }[];

export type VideoSource = typeof VIDEO_SOURCE_OPTIONS[number]['value'];
export type ResolutionValue = typeof RESOLUTION_OPTIONS[number]['value'];
export type AspectMode = typeof ASPECT_OPTIONS[number]['value'];
export type CodecOption = typeof CODEC_OPTIONS[number]['value'];
export type BitrateOption = typeof BITRATE_OPTIONS[number]['value'];
export type LatencyMode = typeof LATENCY_OPTIONS[number]['value'];
export type ResolutionOption = typeof RESOLUTION_OPTIONS[number];

export interface StreamDefaults {
  readonly videoSource: VideoSource;
  readonly resolution: ResolutionValue;
  readonly aspectMode: AspectMode;
  readonly fps: typeof FPS_OPTIONS[number];
  readonly codec: CodecOption;
  readonly bitrate: BitrateOption;
  readonly latency: LatencyMode;
}

export const STREAM_DEFAULTS = {
  videoSource: 'screen',
  resolution: RESOLUTION_PRESETS.FULL_HD.value,
  aspectMode: 'preserve',
  fps: STANDARD_FPS,
  codec: 'H265',
  bitrate: 'auto',
  latency: 'ULL',
} as const satisfies StreamDefaults;

const FULL_HD_CODED_HEIGHT = alignUp(RESOLUTION_PRESETS.FULL_HD.height, CODEC_ALIGNMENT);

export const CODEC_CONFIG = {
  H265: {
    capabilityCodec: 'hev1.1.6.L150.B0',
    defaultCodec: 'hev1.1.6.L150.B0',
    capabilityWidth: RESOLUTION_PRESETS.FULL_HD.width,
    capabilityHeight: FULL_HD_CODED_HEIGHT,
    capabilityBitrate: 6_000_000,
    capabilityFramerate: STREAM_DEFAULTS.fps,
  },
  H264: {
    capabilityCodec: 'avc1.42e028',
    capabilityWidth: RESOLUTION_PRESETS.FULL_HD.width,
    capabilityHeight: FULL_HD_CODED_HEIGHT,
    capabilityBitrate: 8_000_000,
    capabilityFramerate: STREAM_DEFAULTS.fps,
  },
} as const;

export const H264_CODEC_STRINGS = {
  HD60: 'avc1.42e02a',
  HD: 'avc1.42e028',
} as const;

export const CODEC_RESOLUTION_LIMITS = {
  H264_MAX_WIDTH: RESOLUTION_PRESETS.FULL_HD.width,
  H264_MAX_HEIGHT: FULL_HD_CODED_HEIGHT,
  H265_MAX_WIDTH: RESOLUTION_PRESETS.UHD.width,
  H265_MAX_HEIGHT: RESOLUTION_PRESETS.UHD.height,
  H265_AUTO_MIN_WIDTH: RESOLUTION_PRESETS.QHD.width,
} as const;

export const FPS_THRESHOLDS = {
  HIGH: HIGH_FPS,
} as const;

export const DECODER_LIMITS = {
  MAX_ACCESS_UNIT_BYTES: 8 * 1024 * 1024,
} as const;

export const ENCODER_GUARDRAILS = {
  ALIGNMENT: CODEC_ALIGNMENT,
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
    { minWidth: RESOLUTION_PRESETS.UHD.width, at60: 25_000_000, below60: 15_000_000 },
    { minWidth: RESOLUTION_PRESETS.QHD.width, at60: 14_000_000, below60: 9_000_000 },
    { minWidth: RESOLUTION_PRESETS.FULL_HD.width, at60: 10_000_000, below60: 6_000_000 },
  ],
  H264: [
    { minWidth: RESOLUTION_PRESETS.FULL_HD.width, at60: 16_000_000, below60: 10_000_000 },
  ],
  fallback: { at60: 5_000_000, below60: 3_000_000 },
  H264Fallback: { at60: 8_000_000, below60: 5_000_000 },
} as const;

const UINT16_BYTES = 2;
const UINT32_BYTES = 4;
const PACKET_FIELD_BYTES = {
  TAG: 4,
  SEQUENCE: UINT32_BYTES,
  CHUNK_INDEX: UINT16_BYTES,
  CHUNK_COUNT: UINT16_BYTES,
  WIDTH: UINT16_BYTES,
  HEIGHT: UINT16_BYTES,
  CAPTURE_TIME: 8,
  ENCODE_DURATION: 4,
  SEND_START_TIME: 8,
} as const;
const PACKET_FIELD_OFFSETS = {
  TAG: 0,
  SEQUENCE: PACKET_FIELD_BYTES.TAG,
  CHUNK_INDEX: PACKET_FIELD_BYTES.TAG + PACKET_FIELD_BYTES.SEQUENCE,
  CHUNK_COUNT: PACKET_FIELD_BYTES.TAG + PACKET_FIELD_BYTES.SEQUENCE + PACKET_FIELD_BYTES.CHUNK_INDEX,
  WIDTH: PACKET_FIELD_BYTES.TAG + PACKET_FIELD_BYTES.SEQUENCE + PACKET_FIELD_BYTES.CHUNK_INDEX + PACKET_FIELD_BYTES.CHUNK_COUNT,
  HEIGHT: PACKET_FIELD_BYTES.TAG + PACKET_FIELD_BYTES.SEQUENCE + PACKET_FIELD_BYTES.CHUNK_INDEX + PACKET_FIELD_BYTES.CHUNK_COUNT + PACKET_FIELD_BYTES.WIDTH,
  CAPTURE_TIME: PACKET_FIELD_BYTES.TAG + PACKET_FIELD_BYTES.SEQUENCE + PACKET_FIELD_BYTES.CHUNK_INDEX + PACKET_FIELD_BYTES.CHUNK_COUNT + PACKET_FIELD_BYTES.WIDTH + PACKET_FIELD_BYTES.HEIGHT,
  ENCODE_DURATION: PACKET_FIELD_BYTES.TAG + PACKET_FIELD_BYTES.SEQUENCE + PACKET_FIELD_BYTES.CHUNK_INDEX + PACKET_FIELD_BYTES.CHUNK_COUNT + PACKET_FIELD_BYTES.WIDTH + PACKET_FIELD_BYTES.HEIGHT + PACKET_FIELD_BYTES.CAPTURE_TIME,
  SEND_START_TIME: PACKET_FIELD_BYTES.TAG + PACKET_FIELD_BYTES.SEQUENCE + PACKET_FIELD_BYTES.CHUNK_INDEX + PACKET_FIELD_BYTES.CHUNK_COUNT + PACKET_FIELD_BYTES.WIDTH + PACKET_FIELD_BYTES.HEIGHT + PACKET_FIELD_BYTES.CAPTURE_TIME + PACKET_FIELD_BYTES.ENCODE_DURATION,
} as const;
const LEGACY_PACKET_HEADER_BYTES = PACKET_FIELD_OFFSETS.HEIGHT + PACKET_FIELD_BYTES.HEIGHT;
const TIMED_V1_PACKET_HEADER_BYTES = PACKET_FIELD_OFFSETS.ENCODE_DURATION + PACKET_FIELD_BYTES.ENCODE_DURATION;
const PACKET_HEADER_BYTES = PACKET_FIELD_OFFSETS.SEND_START_TIME + PACKET_FIELD_BYTES.SEND_START_TIME;
const LENGTH_PREFIX_BYTES = UINT32_BYTES;

export const TRANSPORT_CONFIG = {
  PACKET_HEADER_BYTES,
  LEGACY_PACKET_HEADER_BYTES,
  TIMED_V1_PACKET_HEADER_BYTES,
  PACKET_FIELD_BYTES,
  PACKET_FIELD_OFFSETS,
  LENGTH_PREFIX_BYTES,
  PACKET_FRAME_PREFIX_BYTES: PACKET_HEADER_BYTES + LENGTH_PREFIX_BYTES,
  MAX_CONTROL_MESSAGE_BYTES: 64 * 1024,
  CODEC_TAGS: { H264: 'H24S', H265: 'H26S' },
  TIMED_V1_CODEC_TAGS: { H264: 'H24T', H265: 'H26T' },
  LEGACY_CODEC_TAGS: { H264: 'H264', H265: 'H265' },
  STOP_TAG: 'STOP',
  SINGLE_PACKET_CHUNK_COUNT: 1,
  KEEPALIVE_INTERVAL_MS: 1000,
  PING_RESPONSE_TIMEOUT_MS: 2500,
  GEOMETRY_TIMEOUT_MS: 5000,
  GEOMETRY_POLL_INTERVAL_MS: 100,
} as const;

const SHA256_BITS = 256;
const BITS_PER_BYTE = 8;
const HEX_CHARS_PER_BYTE = 2;
const SHA256_DIGEST_BYTES = SHA256_BITS / BITS_PER_BYTE;

export const CERTIFICATE_CONFIG = {
  SHA256_DIGEST_BYTES,
  HEX_CHARS_PER_BYTE,
  SHA256_HEX_LENGTH: SHA256_DIGEST_BYTES * HEX_CHARS_PER_BYTE,
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
  H265_VPS_TYPE: 32,
  H265_SPS_TYPE: 33,
  H265_PPS_TYPE: 34,
  H264_AUD: 0x09,
} as const;

const PAIRING_CODE_LENGTH = 4;
export const PAIRING_CONFIG = {
  CODE_LENGTH: PAIRING_CODE_LENGTH,
  CODE_PATTERN: new RegExp(`^[A-Z0-9]{${PAIRING_CODE_LENGTH}}$`),
  DIRECT_WEBTRANSPORT_PORT: 4433,
} as const;
