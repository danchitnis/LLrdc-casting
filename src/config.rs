//! Typed configuration defaults and protocol limits.
//!
//! Values supplied through the environment, CLI, or `config.yaml` remain
//! runtime overrides. This module is the single home for compiled defaults
//! and limits used by the Rust server.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;

pub const DEVICE_CONFIG_PATH: &str = "/config/config.yaml";
pub const DEVICE_CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceConfigDocument {
    pub version: u32,
    pub server: ReceiverSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiverSettings {
    pub port: u16,
    pub webtransport_port: u16,
    pub http_port: u16,
    pub admin_bind_address: String,
    pub admin_port: u16,
    pub drm_connector_id: String,
    pub drm_plane_id: String,
    pub idle_dashboard: bool,
    pub idle_dashboard_mode: String,
    pub idle_timeout_sec: u64,
    pub sender_liveness_timeout_sec: u64,
    pub udp_buffer_size_mb: usize,
    pub cert_dir: String,
    pub pairing_worker_url: String,
    pub cloud_discovery_enabled: bool,
    pub receiver_id: String,
    pub pairing_code_ttl_sec: u64,
    #[serde(default = "default_local_pairing_code_required")]
    pub local_pairing_code_required: bool,
    pub pairing_token_public_key_file: String,
}

fn default_local_pairing_code_required() -> bool { true }

impl Default for ReceiverSettings {
    fn default() -> Self {
        Self {
            port: server::DEFAULT_BOARD_PORT,
            webtransport_port: server::DEFAULT_WEBTRANSPORT_PORT,
            http_port: server::DEFAULT_HTTP_PORT,
            admin_bind_address: String::new(),
            admin_port: server::DEFAULT_ADMIN_PORT,
            drm_connector_id: "auto".to_string(),
            drm_plane_id: server::DEFAULT_DRM_PLANE_ID.to_string(),
            idle_dashboard: true,
            idle_dashboard_mode: dashboard::DEFAULT_MODE.to_string(),
            idle_timeout_sec: server::DEFAULT_IDLE_TIMEOUT_SEC,
            sender_liveness_timeout_sec: server::DEFAULT_SENDER_LIVENESS_TIMEOUT_SEC,
            udp_buffer_size_mb: server::DEFAULT_UDP_BUFFER_SIZE_MB,
            cert_dir: server::DEFAULT_CERTS_DIR.to_string(),
            pairing_worker_url: "https://cast.llrdc.com".to_string(),
            cloud_discovery_enabled: false,
            receiver_id: String::new(),
            pairing_code_ttl_sec: pairing::PAIRING_CODE_TTL_SEC,
            local_pairing_code_required: true,
            pairing_token_public_key_file: server::DEFAULT_PAIRING_PUBLIC_KEY_FILE.to_string(),
        }
    }
}

impl ReceiverSettings {
    pub fn from_environment() -> Self {
        let defaults = Self::default();
        Self {
            port: env_or("BOARD_PORT", defaults.port),
            webtransport_port: env_or("WEBTRANSPORT_PORT", defaults.webtransport_port),
            http_port: env_or("HTTP_PORT", defaults.http_port),
            admin_bind_address: env_string_or("ADMIN_BIND_ADDR", &defaults.admin_bind_address),
            admin_port: env_or("ADMIN_PORT", defaults.admin_port),
            drm_connector_id: env_string_or("DRM_CONNECTOR_ID", &defaults.drm_connector_id),
            drm_plane_id: env_string_or("DRM_PLANE_ID", &defaults.drm_plane_id),
            idle_dashboard: env_bool_or("IDLE_DASHBOARD", defaults.idle_dashboard),
            idle_dashboard_mode: env_string_or("IDLE_DASHBOARD_MODE", &defaults.idle_dashboard_mode),
            idle_timeout_sec: env_or("IDLE_TIMEOUT_SEC", defaults.idle_timeout_sec),
            sender_liveness_timeout_sec: env_or("SENDER_LIVENESS_TIMEOUT_SEC", defaults.sender_liveness_timeout_sec),
            udp_buffer_size_mb: env_or("UDP_BUFFER_SIZE_MB", defaults.udp_buffer_size_mb),
            cert_dir: env_string_or("CERTS_DIR", &defaults.cert_dir),
            pairing_worker_url: env_string_or("PAIRING_WORKER_URL", &defaults.pairing_worker_url),
            cloud_discovery_enabled: env_bool_or("CLOUD_DISCOVERY_ENABLED", defaults.cloud_discovery_enabled),
            receiver_id: env_string_or("RECEIVER_ID", &defaults.receiver_id),
            pairing_code_ttl_sec: env_or("PAIRING_CODE_TTL_SEC", defaults.pairing_code_ttl_sec),
            local_pairing_code_required: env_bool_or("LOCAL_PAIRING_CODE_REQUIRED", defaults.local_pairing_code_required),
            pairing_token_public_key_file: env_string_or("PAIRING_TOKEN_PUBLIC_KEY_FILE", &defaults.pairing_token_public_key_file),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [("admin_bind_address", &self.admin_bind_address), ("drm_connector_id", &self.drm_connector_id), ("drm_plane_id", &self.drm_plane_id), ("idle_dashboard_mode", &self.idle_dashboard_mode), ("cert_dir", &self.cert_dir), ("pairing_worker_url", &self.pairing_worker_url), ("receiver_id", &self.receiver_id), ("pairing_token_public_key_file", &self.pairing_token_public_key_file)] {
            if value.chars().any(|character| character == '\n' || character == '\r') { return Err(format!("{name} must be a single line")); }
        }
        for (name, value) in [("port", self.port), ("webtransport_port", self.webtransport_port), ("http_port", self.http_port), ("admin_port", self.admin_port)] {
            if value == 0 { return Err(format!("{name} must be between 1 and 65535")); }
        }
        if self.port == self.webtransport_port || self.port == self.http_port || self.port == self.admin_port || self.webtransport_port == self.http_port || self.webtransport_port == self.admin_port || self.http_port == self.admin_port {
            return Err("receiver ports must be unique".to_string());
        }
        if self.idle_timeout_sec == 0 || self.sender_liveness_timeout_sec == 0 || self.udp_buffer_size_mb == 0 || self.pairing_code_ttl_sec == 0 {
            return Err("timeouts, buffer size, and pairing TTL must be positive".to_string());
        }
        if self.drm_connector_id != "auto" && !self.drm_connector_id.chars().all(|c| c.is_ascii_digit()) {
            return Err("drm_connector_id must be auto or numeric".to_string());
        }
        if !self.drm_plane_id.chars().all(|c| c.is_ascii_digit()) {
            return Err("drm_plane_id must be numeric".to_string());
        }
        if self.idle_dashboard_mode != "raw" && self.idle_dashboard_mode != "hevc" {
            return Err("idle_dashboard_mode must be raw or hevc".to_string());
        }
        Ok(())
    }
}

static DEVICE_CONFIG: OnceLock<ReceiverSettings> = OnceLock::new();

pub fn initialize() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let settings = load_settings_at(Path::new(DEVICE_CONFIG_PATH))?;
    export_environment(&settings);
    let _ = DEVICE_CONFIG.set(settings);
    Ok(())
}

pub fn parse_document(text: &str) -> Result<DeviceConfigDocument, Box<dyn std::error::Error + Send + Sync>> {
    if let Ok(document) = serde_json::from_str(text) { return Ok(document); }
    // Accept the small, flat YAML shape used by config.yaml without adding a
    // second parser dependency. The deployment writer and this fallback share
    // the same deliberately limited, typed document shape.
    let mut version = None;
    let mut server = serde_json::Map::new();
    let mut in_server = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim_end();
        if line.trim().is_empty() { continue; }
        let trimmed = line.trim();
        if trimmed == "server:" { in_server = true; continue; }
        let Some((key, raw_value)) = trimmed.split_once(':') else { return Err(format!("invalid config line: {trimmed}").into()); };
        let key = key.trim();
        let value = raw_value.trim();
        if key == "version" { version = Some(value.parse::<u32>()?); continue; }
        if !in_server { return Err(format!("unknown top-level config key: {key}").into()); }
        let json_value = serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.trim_matches(|character| character == '"' || character == '\'').to_string()));
        server.insert(key.replace('-', "_"), json_value);
    }
    let value = serde_json::json!({"version": version.ok_or("config version is missing")?, "server": server});
    Ok(serde_json::from_value(value)?)
}

pub fn load_settings_at(path: &Path) -> Result<ReceiverSettings, Box<dyn std::error::Error + Send + Sync>> {
    let settings = if path.is_file() {
        let document = parse_document(&std::fs::read_to_string(path)?)?;
        if document.version != DEVICE_CONFIG_VERSION {
            return Err(format!("unsupported device config version {}", document.version).into());
        }
        document.server
    } else {
        ReceiverSettings::from_environment()
    };
    settings.validate().map_err(|error| format!("invalid receiver configuration: {error}"))?;
    Ok(settings)
}

pub fn settings() -> ReceiverSettings {
    DEVICE_CONFIG.get().cloned().unwrap_or_else(ReceiverSettings::from_environment)
}

pub fn codec_diagnostics_enabled() -> bool {
    env_bool_or("LLRDC_CODEC_DIAGNOSTICS", false)
}

pub fn persist_document(settings: &ReceiverSettings) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    persist_document_at(Path::new(DEVICE_CONFIG_PATH), settings)
}

pub fn persist_document_at(path: &Path, settings: &ReceiverSettings) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    settings.validate().map_err(|error| format!("invalid receiver configuration: {error}"))?;
    let parent = path.parent().ok_or("device config has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join("config.yaml.new");
    let bytes = render_yaml(settings).into_bytes();
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create(true).truncate(true).write(true).open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)] std::fs::set_permissions(&temporary, std::os::unix::fs::PermissionsExt::from_mode(0o640))?;
    std::fs::rename(&temporary, path)?;
    let directory = std::fs::File::open(parent)?;
    directory.sync_all()?;
    Ok(())
}

fn render_yaml(settings: &ReceiverSettings) -> String {
    let quote = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    format!("version: 1\nserver:\n  port: {}\n  webtransport_port: {}\n  http_port: {}\n  admin_bind_address: {}\n  admin_port: {}\n  drm_connector_id: {}\n  drm_plane_id: {}\n  idle_dashboard: {}\n  idle_dashboard_mode: {}\n  idle_timeout_sec: {}\n  sender_liveness_timeout_sec: {}\n  udp_buffer_size_mb: {}\n  cert_dir: {}\n  pairing_worker_url: {}\n  cloud_discovery_enabled: {}\n  receiver_id: {}\n  pairing_code_ttl_sec: {}\n  local_pairing_code_required: {}\n  pairing_token_public_key_file: {}\n", settings.port, settings.webtransport_port, settings.http_port, quote(&settings.admin_bind_address), settings.admin_port, quote(&settings.drm_connector_id), quote(&settings.drm_plane_id), settings.idle_dashboard, quote(&settings.idle_dashboard_mode), settings.idle_timeout_sec, settings.sender_liveness_timeout_sec, settings.udp_buffer_size_mb, quote(&settings.cert_dir), quote(&settings.pairing_worker_url), settings.cloud_discovery_enabled, quote(&settings.receiver_id), settings.pairing_code_ttl_sec, settings.local_pairing_code_required, quote(&settings.pairing_token_public_key_file))
}

fn export_environment(settings: &ReceiverSettings) {
    let pairs = [
        ("BOARD_PORT", settings.port.to_string()), ("UDP_PORT", settings.port.to_string()),
        ("WEBTRANSPORT_PORT", settings.webtransport_port.to_string()), ("HTTP_PORT", settings.http_port.to_string()),
        ("ADMIN_BIND_ADDR", settings.admin_bind_address.clone()), ("ADMIN_PORT", settings.admin_port.to_string()),
        ("DRM_CONNECTOR_ID", settings.drm_connector_id.clone()), ("DRM_PLANE_ID", settings.drm_plane_id.clone()),
        ("IDLE_DASHBOARD", settings.idle_dashboard.to_string()), ("IDLE_DASHBOARD_MODE", settings.idle_dashboard_mode.clone()),
        ("IDLE_TIMEOUT_SEC", settings.idle_timeout_sec.to_string()), ("SENDER_LIVENESS_TIMEOUT_SEC", settings.sender_liveness_timeout_sec.to_string()),
        ("UDP_BUFFER_SIZE_MB", settings.udp_buffer_size_mb.to_string()), ("CERTS_DIR", settings.cert_dir.clone()),
        ("PAIRING_WORKER_URL", settings.pairing_worker_url.clone()), ("CLOUD_DISCOVERY_ENABLED", settings.cloud_discovery_enabled.to_string()),
        ("RECEIVER_ID", settings.receiver_id.clone()), ("PAIRING_CODE_TTL_SEC", settings.pairing_code_ttl_sec.to_string()), ("LOCAL_PAIRING_CODE_REQUIRED", settings.local_pairing_code_required.to_string()),
        ("PAIRING_TOKEN_PUBLIC_KEY_FILE", settings.pairing_token_public_key_file.clone()),
    ];
    for (name, value) in pairs { std::env::set_var(name, value); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip_preserves_settings() {
        let settings = ReceiverSettings::default();
        let text = render_yaml(&settings);
        let parsed = parse_document(&text).unwrap();
        assert_eq!(parsed.server, settings);
    }

    #[test]
    fn atomic_persistence_replaces_previous_document() {
        let path = std::env::temp_dir().join(format!("llrdc-config-{}", std::process::id()));
        let first = ReceiverSettings::default();
        persist_document_at(&path, &first).unwrap();
        let mut second = first.clone(); second.http_port = 8081;
        persist_document_at(&path, &second).unwrap();
        let parsed = parse_document(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed.server, second);
        assert!(!path.with_file_name("config.yaml.new").exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_port_and_mode_are_rejected() {
        let mut settings = ReceiverSettings::default(); settings.http_port = settings.port;
        assert!(settings.validate().is_err());
        settings.http_port = 8080; settings.idle_dashboard_mode = "bad".to_string();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn hand_authored_yaml_is_supported_and_unknown_fields_fail() {
        let yaml = "version: 1\nserver:\n  port: 4434\n  webtransport_port: 4433\n  http_port: 8080\n  admin_bind_address: 100.100.1.72\n  admin_port: 9090\n  drm_connector_id: auto\n  drm_plane_id: '33'\n  idle_dashboard: true\n  idle_dashboard_mode: raw\n  idle_timeout_sec: 30\n  sender_liveness_timeout_sec: 90\n  udp_buffer_size_mb: 8\n  cert_dir: /certs\n  pairing_worker_url: https://cast.llrdc.com\n  cloud_discovery_enabled: false\n  receiver_id: ''\n  pairing_code_ttl_sec: 3600\n  local_pairing_code_required: true\n  pairing_token_public_key_file: /pairing/public.pem\n";
        assert!(parse_document(yaml).is_ok());
        assert!(parse_document(&yaml.replace("  port: 4434", "  unknown: true\n  port: 4434")).is_err());
    }

    #[test]
    fn older_device_documents_default_to_required_pairing() {
        let yaml = render_yaml(&ReceiverSettings::default()).replace("  local_pairing_code_required: true\n", "");
        assert!(parse_document(&yaml).unwrap().server.local_pairing_code_required);
    }

    #[test]
    fn environment_can_disable_local_pairing_requirement() {
        std::env::set_var("LOCAL_PAIRING_CODE_REQUIRED", "false");
        let settings = ReceiverSettings::from_environment();
        std::env::remove_var("LOCAL_PAIRING_CODE_REQUIRED");
        assert!(!settings.local_pairing_code_required);
    }
}

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
