use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use rand::Rng;
use reqwest::Client;
use rsa::{pkcs8::DecodePublicKey, pss::VerifyingKey, RsaPublicKey};
use serde::{Deserialize, Serialize};
use signature::Verifier;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config;
use crate::local_pairing::PairingState;

type HmacSha256 = Hmac<Sha256>;

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
    matches!(std::env::var("CLOUD_DISCOVERY_ENABLED").as_deref(), Ok("1") | Ok("true") | Ok("yes"))
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
