//! Typed configuration defaults and protocol limits.
//!
//! Values supplied through the environment, CLI, or `config.yaml` remain
//! runtime overrides. This module is the single home for compiled defaults
//! and limits used by the Rust server.

use std::str::FromStr;

/// Parse a typed environment override, retaining the supplied fallback when
/// the variable is absent or cannot be parsed.
pub fn env_or<T>(name: &str, fallback: T) -> T
where
    T: FromStr,
{
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

/// Read a string environment override while retaining the compiled default
/// when the variable is absent.
pub fn env_string_or(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_owned())
}

pub fn env_bool_or(name: &str, fallback: bool) -> bool {
    match std::env::var(name).ok().as_deref() {
        Some("1") | Some("true") | Some("TRUE") | Some("yes") | Some("YES") => true,
        Some("0") | Some("false") | Some("FALSE") | Some("no") | Some("NO") => false,
        _ => fallback,
    }
}

pub mod server {
    pub const DEFAULT_HTTP_PORT: u16 = 8080;
    pub const DEFAULT_ADMIN_PORT: u16 = 9090;
    pub const DEFAULT_WEBTRANSPORT_PORT: u16 = 4433;
    pub const DEFAULT_BOARD_PORT: u16 = 4434;
    pub const DEFAULT_UDP_BUFFER_SIZE_MB: usize = 8;
    pub const DEFAULT_IDLE_TIMEOUT_SEC: u64 = 30;
    pub const DEFAULT_SENDER_LIVENESS_TIMEOUT_SEC: u64 = 90;
    pub const DEFAULT_CERTS_DIR: &str = "/certs";
    pub const DEFAULT_PAIRING_PUBLIC_KEY_FILE: &str = "/pairing/public.pem";
    pub const DEFAULT_DRM_PLANE_ID: &str = "33";
    pub const HTTP_REQUEST_BUFFER_BYTES: usize = 4 * 1024;
    pub const HTTP_TLS_BUFFER_BYTES: usize = 1024;
}

pub mod pairing {
    pub const PAIRING_CODE_LENGTH: usize = 4;
    pub const PAIRING_CODE_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    pub const PAIRING_CODE_TTL_SEC: u64 = 60 * 60;
    pub const PAIRING_ATTEMPT_WINDOW_SEC: u64 = 60;
    pub const PAIRING_ATTEMPT_LIMIT: u32 = 5;
    pub const PAIRING_TOKEN_ISSUED_AT_SKEW_SEC: i64 = 30;
    pub const PAIRING_TOKEN_MAX_LIFETIME_SEC: i64 = 60;
    pub const PAIRING_REPLAY_CACHE_LIMIT: usize = 1024;
}

pub mod packet {
    use std::time::Duration;

    pub const CODEC_ALIGNMENT: usize = 16;
    pub const H264_VISIBLE_MAX_HEIGHT: usize = 1080;
    pub const CODEC_TAG_BYTES: usize = 4;
    pub const SEQUENCE_BYTES: usize = 4;
    pub const CHUNK_INDEX_BYTES: usize = 2;
    pub const CHUNK_COUNT_BYTES: usize = 2;
    pub const DIMENSION_BYTES: usize = 2;
    pub const TAG_OFFSET: usize = 0;
    pub const SEQUENCE_OFFSET: usize = TAG_OFFSET + CODEC_TAG_BYTES;
    pub const CHUNK_INDEX_OFFSET: usize = SEQUENCE_OFFSET + SEQUENCE_BYTES;
    pub const CHUNK_COUNT_OFFSET: usize = CHUNK_INDEX_OFFSET + CHUNK_INDEX_BYTES;
    pub const WIDTH_OFFSET: usize = CHUNK_COUNT_OFFSET + CHUNK_COUNT_BYTES;
    pub const HEIGHT_OFFSET: usize = WIDTH_OFFSET + DIMENSION_BYTES;
    /// Codec tag, sequence, chunk index/count, width, and height.
    pub const PACKET_HEADER_BYTES: usize = HEIGHT_OFFSET + DIMENSION_BYTES;
    pub const H264_TAG: &[u8; CODEC_TAG_BYTES] = b"H264";
    pub const H265_TAG: &[u8; CODEC_TAG_BYTES] = b"H265";
    pub const LEGACY_H264_TAG: &[u8; CODEC_TAG_BYTES] = b"VIDC";
    pub const LEGACY_H265_TAG: &[u8; CODEC_TAG_BYTES] = b"HEVC";
    pub const STOP_TAG: &[u8; CODEC_TAG_BYTES] = b"STOP";
    pub const CHUNK_BYTES: usize = 1350;
    pub const MAX_ACCESS_UNIT_BYTES: usize = 8 * 1024 * 1024;
    pub const MAX_IN_FLIGHT_ACCESS_UNITS: usize = 32;
    pub const ACCESS_UNIT_ASSEMBLY_TTL_MS: u64 = 50;
    pub const ACCESS_UNIT_ASSEMBLY_TTL: Duration =
        Duration::from_millis(ACCESS_UNIT_ASSEMBLY_TTL_MS);
    pub const MAX_UNI_STREAM_MESSAGE_BYTES: usize = MAX_ACCESS_UNIT_BYTES + PACKET_HEADER_BYTES;
    pub const MAX_CONTROL_MESSAGE_BYTES: usize = 64 * 1024;
    pub const H264_MAX_WIDTH: usize = 1920;
    pub const H264_MAX_HEIGHT: usize =
        (H264_VISIBLE_MAX_HEIGHT + CODEC_ALIGNMENT - 1) / CODEC_ALIGNMENT * CODEC_ALIGNMENT;
    pub const H265_MAX_WIDTH: usize = 3840;
    pub const H265_MAX_HEIGHT: usize = 2160;
}

pub mod transport {
    pub const FRAME_CHANNEL_CAPACITY: usize = 64;
    pub const CONTROL_CHANNEL_CAPACITY: usize = 32;
    pub const LENGTH_PREFIX_BYTES: usize = 4;
    pub const DATAGRAM_BUFFER_BYTES: usize = 64 * 1024;
    pub const DATAGRAM_ERROR_RETRY_SEC: u64 = 60 * 60;
}

pub mod playback {
    pub const KMS_DEVICE_PIXEL_ASPECT_RATIO: &str = "15/16";
    pub const DEFAULT_DISPLAY_CONNECTOR_ID: &str = "54";
    pub const RAW_PIPELINE_BLOCK_SIZE: usize = 64 * 1024;
    pub const RAW_PIPELINE_FRAMERATE: &str = "1/1";
    pub const RAW_QUEUE_CAPACITY: usize = 2;
    pub const ENCODED_QUEUE_CAPACITY: usize = 16;
    pub const ENCODED_CONFIG_INTERVAL: i32 = -1;
}

pub mod certificate {
    pub const VALIDITY_REFRESH_BUFFER_SEC: u64 = 24 * 60 * 60;
    pub const NOT_BEFORE_OFFSET_DAYS: i64 = 1;
    pub const NOT_AFTER_OFFSET_DAYS: i64 = 13;
}

pub mod discovery {
    pub const SHA256_DIGEST_BYTES: usize = 256 / 8;
    pub const TOKEN_VERSION: u8 = 1;
    // Keep synchronized with TOKEN_VERSION; the cross-language consistency
    // check also verifies that this is the textual `v{VERSION}` prefix.
    pub const TOKEN_PREFIX: &str = "v1";
    pub const TOKEN_ALGORITHM: &str = "PS256";
    pub const TOKEN_TYPE: &str = "CAST-CONNECTION";
    pub const TOKEN_PURPOSE: &str = "webtransport-connect";
    pub const REGISTRATION_INITIAL_RETRY_SEC: u64 = 2;
    pub const REGISTRATION_NO_IP_RETRY_SEC: u64 = 5;
    pub const REGISTRATION_NO_CODE_RETRY_SEC: u64 = 2;
    pub const REGISTRATION_SUCCESS_RETRY_SEC: u64 = 45;
    pub const REGISTRATION_MAX_RETRY_SEC: u64 = 60;
    pub const REGISTRATION_NONCE_BYTES: usize = 16;
    pub const TOKEN_RSA_SALT_BYTES: usize = SHA256_DIGEST_BYTES;
}

pub mod telemetry {
    pub const DEFAULT_CODEC: &str = "hevc";
    pub const DEFAULT_ASPECT_MODE: &str = "preserve";
    pub const DEFAULT_ACTIVE_FPS: u32 = 30;
    pub const DEFAULT_ACTIVE_RESOLUTION: &str = "1920x1080";
    pub const DEFAULT_ACTIVE_BITRATE_MBPS: f32 = 10.0;
    pub const DEFAULT_ACTIVE_LATENCY_MODE: &str = "ULL";
    pub const DEFAULT_IDLE_RESOLUTION: &str = "0x0";
    pub const DEFAULT_IDLE_FPS: u32 = 0;
    pub const DEFAULT_IDLE_BITRATE_MBPS: f32 = 0.0;
    pub const DEFAULT_IDLE_LATENCY_MS: f32 = 0.0;
    pub const DEFAULT_DELIVERY_RATE_PERCENT: f32 = 100.0;
    pub const PERCENT_SCALE: f64 = 100.0;
    pub const DEFAULT_TELEMETRY_CHANNEL_CAPACITY: usize = 100;
}

pub mod ui {
    pub const MAX_UI_BYTES: usize = 2 * 1024 * 1024;
}

pub mod display {
    pub const DEFAULT_MAX_WIDTH: u32 = 1920;
    pub const DEFAULT_MAX_HEIGHT: u32 = 1080;
    pub const DEFAULT_MAX_FPS: u32 = 60;
    pub const DRM_CARD_SCAN_LIMIT: u32 = 4;
    pub const DRM_CONNECT_ATTEMPTS: u32 = 10;
    pub const DRM_CONNECT_RETRY_MS: u64 = 100;
}

pub mod dashboard {
    pub const DEFAULT_MODE: &str = "raw";
    pub const RAW_FRAME_RATE: &str = "1";
    pub const ENCODED_KEYFRAME_INTERVAL: u32 = 3600;
    pub const STDOUT_BUFFER_BYTES: usize = 128 * 1024;
    pub const FEED_INTERVAL_MS: u64 = 100;
}
