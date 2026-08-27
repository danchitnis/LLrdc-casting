use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use rand::Rng;
use reqwest::Client;
use rsa::{pkcs8::DecodePublicKey, pss::VerifyingKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use signature::Verifier;
use sha2::Sha256;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config;
use crate::local_pairing::PairingState;

type HmacSha256 = Hmac<Sha256>;

const SETTINGS_PATH: &str = "/settings/cloud-discovery-enabled";

#[derive(Clone, Debug, Serialize)]
pub struct CloudSettingsSnapshot {
    pub cloud_discovery_enabled: bool,
    pub cloud_configuration_ready: bool,
    pub cloud_configuration_missing: Vec<String>,
}

#[derive(Serialize)]
struct RegistrationBody<'a> {
    receiver_id: &'a str,
    ip_address: &'a str,
    webtransport_port: u16,
    cert_hash_hex: &'a str,
    pairing_code: &'a str,
}

#[derive(Deserialize)]
struct TokenHeader {
    alg: String,
    typ: String,
    v: u8,
}

#[derive(Deserialize)]
struct TokenPayload {
    receiver_id: String,
    purpose: String,
    iat: u64,
    exp: u64,
    jti: String,
}

pub struct ConnectionTokenVerifier {
    receiver_id: String,
    public_key: RsaPublicKey,
    seen: Mutex<HashMap<String, u64>>,
}

impl ConnectionTokenVerifier {
    pub fn from_environment() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let receiver_id = std::env::var("RECEIVER_ID")?;
        let public_key_pem = std::env::var("PAIRING_TOKEN_PUBLIC_KEY")
            .or_else(|_| {
                let path = std::env::var("PAIRING_TOKEN_PUBLIC_KEY_FILE")
                    .unwrap_or_else(|_| config::server::DEFAULT_PAIRING_PUBLIC_KEY_FILE.to_string());
                std::fs::read_to_string(path).map_err(|_| std::env::VarError::NotPresent)
            })?;
        Ok(Self {
            receiver_id,
            public_key: RsaPublicKey::from_public_key_pem(&public_key_pem)?,
            seen: Mutex::new(HashMap::new()),
        })
    }

    pub fn verify(&self, token: &str) -> Result<(), &'static str> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 4 || parts[0] != config::discovery::TOKEN_PREFIX {
            return Err("invalid token format");
        }
        let decode = |value: &str| URL_SAFE_NO_PAD.decode(value).map_err(|_| "invalid token encoding");
        let header: TokenHeader = serde_json::from_slice(&decode(parts[1])?).map_err(|_| "invalid token header")?;
        let payload: TokenPayload = serde_json::from_slice(&decode(parts[2])?).map_err(|_| "invalid token payload")?;
        let signature = decode(parts[3])?;
        if header.alg != config::discovery::TOKEN_ALGORITHM
            || header.typ != config::discovery::TOKEN_TYPE
            || header.v != config::discovery::TOKEN_VERSION
        {
            return Err("unsupported token");
        }
        if payload.receiver_id != self.receiver_id || payload.purpose != config::discovery::TOKEN_PURPOSE {
            return Err("token receiver mismatch");
        }
        let now = unix_seconds();
        if payload.iat > now.saturating_add(config::pairing::PAIRING_TOKEN_ISSUED_AT_SKEW_SEC as u64)
            || payload.exp <= now
            || payload.exp <= payload.iat
            || payload.exp - payload.iat > config::pairing::PAIRING_TOKEN_MAX_LIFETIME_SEC as u64
        {
            return Err("expired token");
        }
        let signing_input = format!("{}.{}.{}", config::discovery::TOKEN_PREFIX, parts[1], parts[2]);
        let verifying_key = VerifyingKey::<Sha256>::new_with_salt_len(
            self.public_key.clone(),
            config::discovery::TOKEN_RSA_SALT_BYTES,
        );
        let signature = rsa::pss::Signature::try_from(signature.as_slice())
            .map_err(|_| "invalid token signature")?;
        verifying_key
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| "invalid token signature")?;
        let mut seen = self.seen.lock().map_err(|_| "token cache unavailable")?;
        seen.retain(|_, expires| *expires > now);
        if seen.contains_key(&payload.jti) {
            return Err("replayed token");
        }
        if seen.len() >= config::pairing::PAIRING_REPLAY_CACHE_LIMIT {
            if let Some(oldest) = seen.iter().min_by_key(|(_, expires)| *expires).map(|(id, _)| id.clone()) {
                seen.remove(&oldest);
            }
        }
        seen.insert(payload.jti, payload.exp);
        Ok(())
    }
}

pub fn cloud_discovery_enabled() -> bool {
    resolve_persisted_enabled(Path::new(SETTINGS_PATH), config::env_bool_or("CLOUD_DISCOVERY_ENABLED", false))
}

fn resolve_persisted_enabled(path: &Path, fallback: bool) -> bool {
    match std::fs::read_to_string(path) {
        Ok(value) => match parse_enabled(&value) {
            Some(enabled) => enabled,
            None => {
                eprintln!("[CLOUD DISCOVERY] Invalid persisted setting; cloud discovery is disabled");
                false
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fallback
        }
        Err(error) => {
            eprintln!("[CLOUD DISCOVERY] Could not read persisted setting ({error}); cloud discovery is disabled");
            false
        }
    }
}

fn parse_enabled(value: &str) -> Option<bool> {
    match value.trim() {
        "1" | "true" | "TRUE" | "yes" | "YES" => Some(true),
        "0" | "false" | "FALSE" | "no" | "NO" => Some(false),
        _ => None,
    }
}

pub fn persist_cloud_discovery_enabled(enabled: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    persist_cloud_discovery_enabled_at(Path::new(SETTINGS_PATH), enabled)
}

fn persist_cloud_discovery_enabled_at(path: &Path, enabled: bool) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parent = path.parent().ok_or("persisted cloud setting has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join("cloud-discovery-enabled.new");
    std::fs::write(&temporary, if enabled { "1\n" } else { "0\n" })?;
    #[cfg(unix)]
    std::fs::set_permissions(&temporary, std::os::unix::fs::PermissionsExt::from_mode(0o644))?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

pub fn cloud_configuration_missing() -> Vec<String> {
    let mut missing = Vec::new();
    let valid_worker_url = std::env::var("PAIRING_WORKER_URL")
        .ok()
        .and_then(|value| reqwest::Url::parse(value.trim()).ok())
        .is_some_and(|url| url.scheme() == "https" && url.host_str().is_some());
    if !valid_worker_url {
        missing.push("worker_url".to_string());
    }
    if std::env::var("RECEIVER_ID").ok().map_or(true, |value| value.trim().is_empty()) {
        missing.push("receiver_id".to_string());
    }
    let valid_secret = std::env::var("RECEIVER_REGISTRATION_SECRET")
        .ok()
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .is_some_and(|value| value.len() >= 32);
    if !valid_secret {
        missing.push("registration_secret".to_string());
    }
    let public_key_available = std::env::var("PAIRING_TOKEN_PUBLIC_KEY")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
        || std::env::var("PAIRING_TOKEN_PUBLIC_KEY_FILE")
            .ok()
            .map(|path| Path::new(&path).is_file())
            .unwrap_or_else(|| Path::new(config::server::DEFAULT_PAIRING_PUBLIC_KEY_FILE).is_file());
    if !public_key_available {
        missing.push("token_public_key".to_string());
    } else if ConnectionTokenVerifier::from_environment().is_err() {
        missing.push("token_public_key".to_string());
    }
    missing
}

pub fn settings_snapshot() -> CloudSettingsSnapshot {
    let missing = cloud_configuration_missing();
    CloudSettingsSnapshot {
        cloud_discovery_enabled: cloud_discovery_enabled(),
        cloud_configuration_ready: missing.is_empty(),
        cloud_configuration_missing: missing,
    }
}

pub fn spawn_registration(state: PairingState, cert_hash: String) {
    state.set_cloud_ip(None);
    state.set_cloud_status("WAITING");
    tokio::spawn(async move {
        let worker_url = match std::env::var("PAIRING_WORKER_URL") {
            Ok(value) if !value.trim().is_empty() => value.trim_end_matches('/').to_string(),
            _ => {
                state.set_cloud_ip(None);
                state.set_cloud_status("NO URL");
                return;
            }
        };
        let receiver_id = match std::env::var("RECEIVER_ID") {
            Ok(value) if !value.is_empty() => value,
            _ => {
                state.set_cloud_ip(None);
                state.set_cloud_status("NO ID");
                return;
            }
        };
        let key = match std::env::var("RECEIVER_REGISTRATION_SECRET").ok().and_then(|value| URL_SAFE_NO_PAD.decode(value).ok()) {
            Some(value) if value.len() >= 32 => value,
            _ => {
                state.set_cloud_ip(None);
                state.set_cloud_status("NO KEY");
                return;
            }
        };
        let client = Client::new();
        let port = config::env_or(
            "WEBTRANSPORT_PORT",
            config::server::DEFAULT_WEBTRANSPORT_PORT,
        );
        let mut retry_delay = Duration::from_secs(config::discovery::REGISTRATION_INITIAL_RETRY_SEC);
        loop {
            let Some(ip) = crate::net::get_preferred_private_ipv4() else {
                state.set_cloud_ip(None);
                state.set_cloud_status("WAITING");
                tokio::time::sleep(Duration::from_secs(config::discovery::REGISTRATION_NO_IP_RETRY_SEC)).await;
                continue;
            };
            let Some(code) = state.snapshot().code else {
                state.set_cloud_ip(None);
                state.set_cloud_status("WAITING");
                tokio::time::sleep(Duration::from_secs(config::discovery::REGISTRATION_NO_CODE_RETRY_SEC)).await;
                continue;
            };
            let body = match serde_json::to_vec(&RegistrationBody {
                receiver_id: &receiver_id,
                ip_address: &ip,
                webtransport_port: port,
                cert_hash_hex: &cert_hash,
                pairing_code: &code,
            }) {
                Ok(body) => body,
                Err(_) => continue,
            };
            let timestamp = unix_seconds();
            let nonce = random_nonce();
            let mut mac = HmacSha256::new_from_slice(&key).map_err(|_| ()).unwrap();
            mac.update(format!("{}\n{}\n", timestamp, nonce).as_bytes());
            mac.update(&body);
            let signature = mac.finalize().into_bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            let result = client
                .post(format!("{worker_url}/api/receiver/register"))
                .header("content-type", "application/json")
                .header("x-receiver-timestamp", timestamp.to_string())
                .header("x-receiver-nonce", &nonce)
                .header("x-receiver-signature", signature)
                .body(body)
                .send()
                .await;
            match result {
                Ok(response) if response.status().is_success() => {
                    println!("[CLOUD DISCOVERY] Receiver registration succeeded via {ip}");
                    state.set_cloud_ip(Some(ip));
                    state.set_cloud_status("READY");
                    retry_delay = Duration::from_secs(config::discovery::REGISTRATION_INITIAL_RETRY_SEC);
                    tokio::time::sleep(Duration::from_secs(config::discovery::REGISTRATION_SUCCESS_RETRY_SEC)).await;
                }
                Ok(response) => {
                    let status = response.status();
                    let detail = response.text().await.unwrap_or_default();
                    eprintln!(
                        "[CLOUD DISCOVERY] Receiver registration rejected: HTTP {status} ({detail})"
                    );
                    state.set_cloud_ip(None);
                    state.set_cloud_status("FAILED");
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(config::discovery::REGISTRATION_MAX_RETRY_SEC));
                }
                Err(error) => {
                    eprintln!("[CLOUD DISCOVERY] Receiver registration request failed: {error}");
                    state.set_cloud_ip(None);
                    state.set_cloud_status("FAILED");
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(config::discovery::REGISTRATION_MAX_RETRY_SEC));
                }
            }
        }
    });
}

fn random_nonce() -> String {
    let mut bytes = [0u8; config::discovery::REGISTRATION_NONCE_BYTES];
    rand::thread_rng().fill(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("llrdc-{name}-{}", std::process::id()))
    }

    #[test]
    fn environment_fallback_is_used_when_persisted_setting_is_absent() {
        let path = temporary_path("cloud-fallback");
        let _ = std::fs::remove_file(&path);
        assert!(resolve_persisted_enabled(&path, true));
        assert!(!resolve_persisted_enabled(&path, false));
    }

    #[test]
    fn persisted_setting_takes_precedence_over_environment_fallback() {
        let path = temporary_path("cloud-precedence");
        std::fs::write(&path, "0\n").expect("write setting");
        assert!(!resolve_persisted_enabled(&path, true));
        std::fs::write(&path, "1\n").expect("write setting");
        assert!(resolve_persisted_enabled(&path, false));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn malformed_persisted_setting_fails_closed() {
        let path = temporary_path("cloud-malformed");
        std::fs::write(&path, "maybe\n").expect("write setting");
        assert!(!resolve_persisted_enabled(&path, true));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn persisted_updates_are_atomic_and_replace_the_old_value() {
        let path = temporary_path("cloud-atomic");
        let temporary = path.with_file_name("cloud-atomic.new");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&temporary);
        persist_cloud_discovery_enabled_at(&path, true).expect("persist setting");
        assert!(resolve_persisted_enabled(&path, false));
        assert!(!temporary.exists());
        persist_cloud_discovery_enabled_at(&path, false).expect("replace setting");
        assert!(!resolve_persisted_enabled(&path, true));
        assert!(!temporary.exists());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn persistence_failure_does_not_report_success() {
        let parent = temporary_path("cloud-failure-parent");
        std::fs::write(&parent, "not a directory").expect("write blocker");
        let path = parent.join("setting");
        assert!(persist_cloud_discovery_enabled_at(&path, true).is_err());
        let _ = std::fs::remove_file(parent);
    }
}
