use rand::Rng;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::pairing::{
    PAIRING_ATTEMPT_LIMIT, PAIRING_ATTEMPT_WINDOW_SEC, PAIRING_CODE_ALPHABET,
    PAIRING_CODE_LENGTH, PAIRING_CODE_TTL_SEC,
};

#[derive(Clone, Debug)]
pub struct PairingSnapshot {
    pub code: Option<String>,
    pub local_status: String,
    pub cloud_status: String,
    pub cloud_ip: Option<String>,
}

struct PairingData {
    snapshot: PairingSnapshot,
    expires_at: Option<Instant>,
    failed_attempts: HashMap<String, (u32, Instant)>,
    fixed_code: Option<String>,
}

#[derive(Clone)]
pub struct PairingState {
    inner: Arc<Mutex<PairingData>>,
}

impl PairingState {
    pub fn with_fixed_code(code: Option<String>) -> Result<Self, &'static str> {
        let fixed_code = code
            .map(|value| value.to_ascii_uppercase())
            .map(|value| {
                if is_valid_pairing_code(&value) {
                    Ok(value)
                } else {
                    Err("fixed pairing code must be four alphanumeric characters")
                }
            })
            .transpose()?;

        Ok(Self {
            inner: Arc::new(Mutex::new(PairingData {
                snapshot: PairingSnapshot {
                    code: None,
                    local_status: "WAITING".to_string(),
                    cloud_status: "DISABLED".to_string(),
                    cloud_ip: None,
                },
                expires_at: None,
                failed_attempts: HashMap::new(),
                fixed_code,
            })),
        })
    }

    pub fn rotate_code(&self) -> String {
        let mut rng = rand::thread_rng();
        if let Ok(mut data) = self.inner.lock() {
            let code = data.fixed_code.clone().unwrap_or_else(|| {
                (0..PAIRING_CODE_LENGTH)
                    .map(|_| PAIRING_CODE_ALPHABET[rng.gen_range(0..PAIRING_CODE_ALPHABET.len())] as char)
                    .collect()
            });
            let ttl = pairing_code_ttl_seconds();
            data.snapshot.code = Some(code.clone());
            data.snapshot.local_status = "READY".to_string();
            data.expires_at = Some(Instant::now() + Duration::from_secs(ttl));
            data.failed_attempts.clear();
            return code;
        }
        String::new()
    }

    pub fn set_cloud_status(&self, status: impl Into<String>) {
        if let Ok(mut data) = self.inner.lock() {
            data.snapshot.cloud_status = status.into();
        }
    }

    pub fn set_cloud_ip(&self, ip: Option<String>) {
        if let Ok(mut data) = self.inner.lock() {
            data.snapshot.cloud_ip = ip;
        }
    }

    pub fn snapshot(&self) -> PairingSnapshot {
        let Ok(mut data) = self.inner.lock() else {
            return PairingSnapshot {
                code: None,
                local_status: "ERROR".to_string(),
                cloud_status: "ERROR".to_string(),
                cloud_ip: None,
            };
        };
        if data.expires_at.is_some_and(|expires_at| expires_at <= Instant::now()) {
            data.snapshot.code = None;
            data.snapshot.local_status = "EXPIRED".to_string();
        }
        data.snapshot.clone()
    }

    pub fn validate_code(&self, code: &str, peer: &str) -> Result<(), &'static str> {
        let Ok(mut data) = self.inner.lock() else {
            return Err("pairing unavailable");
        };
        let now = Instant::now();
        let valid = is_valid_pairing_code(code)
            && data
                .snapshot
                .code
                .as_deref()
                .is_some_and(|expected| expected.eq_ignore_ascii_case(code))
            && data.expires_at.is_some_and(|expires_at| expires_at > now);
        let attempts = data.failed_attempts.entry(peer.to_string()).or_insert((0, now));
        if attempts.1 + Duration::from_secs(PAIRING_ATTEMPT_WINDOW_SEC) <= now {
            *attempts = (0, now);
        }
        if attempts.0 >= PAIRING_ATTEMPT_LIMIT {
            return Err("too many pairing attempts");
        }
        if !valid {
            attempts.0 += 1;
            return Err("invalid or expired pairing code");
        }
        Ok(())
    }
}

pub fn pairing_code_ttl_seconds() -> u64 {
    crate::config::env_or("PAIRING_CODE_TTL_SEC", PAIRING_CODE_TTL_SEC)
        .max(PAIRING_CODE_TTL_SEC)
}

fn is_valid_pairing_code(code: &str) -> bool {
    code.len() == PAIRING_CODE_LENGTH && code.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

pub fn spawn_local_pairing(state: PairingState) {
    tokio::spawn(async move {
        loop {
            state.rotate_code();
            tokio::time::sleep(Duration::from_secs(pairing_code_ttl_seconds())).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{is_valid_pairing_code, PairingState};

    #[test]
    fn generated_codes_are_four_uppercase_alphanumeric_characters() {
        let code = PairingState::with_fixed_code(None).unwrap().rotate_code();

        assert_eq!(code.len(), 4);
        assert!(code.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()));
    }

    #[test]
    fn fixed_code_is_used_and_normalized() {
        let state = PairingState::with_fixed_code(Some("ab12".to_string())).unwrap();

        assert_eq!(state.rotate_code(), "AB12");
        assert!(state.validate_code("ab12", "peer").is_ok());
        assert!(state.validate_code("0000", "peer").is_err());
    }

    #[test]
    fn local_and_cloud_statuses_are_independent() {
        let state = PairingState::with_fixed_code(None).unwrap();

        let initial = state.snapshot();
        assert_eq!(initial.local_status, "WAITING");
        assert_eq!(initial.cloud_status, "DISABLED");

        state.set_cloud_status("READY");
        state.set_cloud_ip(Some("wired-test-ip".to_string()));
        assert_eq!(state.snapshot().local_status, "WAITING");

        state.rotate_code();
        let ready = state.snapshot();
        assert_eq!(ready.local_status, "READY");
        assert_eq!(ready.cloud_status, "READY");
        assert_eq!(ready.cloud_ip.as_deref(), Some("wired-test-ip"));

        state.set_cloud_status("FAILED");
        state.set_cloud_ip(None);
        let failed_cloud = state.snapshot();
        assert_eq!(failed_cloud.local_status, "READY");
        assert_eq!(failed_cloud.cloud_status, "FAILED");
        assert_eq!(failed_cloud.cloud_ip, None);
    }

    #[test]
    fn fixed_code_requires_four_alphanumeric_characters() {
        assert!(PairingState::with_fixed_code(Some("123".to_string())).is_err());
        assert!(PairingState::with_fixed_code(Some("12-4".to_string())).is_err());
    }

    #[test]
    fn pairing_code_validation_accepts_only_four_alphanumeric_characters() {
        assert!(is_valid_pairing_code("A78Q"));
        assert!(is_valid_pairing_code("o0Z9"));
        assert!(!is_valid_pairing_code("123"));
        assert!(!is_valid_pairing_code("12345"));
        assert!(!is_valid_pairing_code("A7-Q"));
    }
}
