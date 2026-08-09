use rand::Rng;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct PairingSnapshot {
    pub code: Option<String>,
    pub status: String,
}

struct PairingData {
    snapshot: PairingSnapshot,
    expires_at: Option<Instant>,
    failed_attempts: HashMap<String, (u32, Instant)>,
}

#[derive(Clone)]
pub struct PairingState {
    inner: Arc<Mutex<PairingData>>,
}

impl PairingState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PairingData {
                snapshot: PairingSnapshot {
                    code: None,
                    status: "WAITING FOR NETWORK".to_string(),
                },
                expires_at: None,
                failed_attempts: HashMap::new(),
            })),
        }
    }

    pub fn rotate_code(&self) -> String {
        let code = format!("{:04}", rand::thread_rng().gen_range(0..10_000));
        let ttl = pairing_code_ttl_seconds();
        if let Ok(mut data) = self.inner.lock() {
            data.snapshot.code = Some(code.clone());
            data.snapshot.status = "READY".to_string();
            data.expires_at = Some(Instant::now() + Duration::from_secs(ttl));
            data.failed_attempts.clear();
        }
        code
    }

    pub fn set_status(&self, status: impl Into<String>) {
        if let Ok(mut data) = self.inner.lock() {
            data.snapshot.status = status.into();
        }
    }

    pub fn snapshot(&self) -> PairingSnapshot {
        let Ok(mut data) = self.inner.lock() else {
            return PairingSnapshot {
                code: None,
                status: "PAIRING UNAVAILABLE".to_string(),
            };
        };
        if data.expires_at.is_some_and(|expires_at| expires_at <= Instant::now()) {
            data.snapshot.code = None;
            data.snapshot.status = "CODE EXPIRED".to_string();
        }
        data.snapshot.clone()
    }

    pub fn validate_code(&self, code: &str, peer: &str) -> Result<(), &'static str> {
        let Ok(mut data) = self.inner.lock() else {
            return Err("pairing unavailable");
        };
        let now = Instant::now();
        let valid = data.snapshot.code.as_deref() == Some(code)
            && data.expires_at.is_some_and(|expires_at| expires_at > now);
        let attempts = data.failed_attempts.entry(peer.to_string()).or_insert((0, now));
        if attempts.1 + Duration::from_secs(60) <= now {
            *attempts = (0, now);
        }
        if attempts.0 >= 5 {
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
    std::env::var("PAIRING_CODE_TTL_SEC")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(3600)
        .max(3600)
}

pub fn spawn_local_pairing(state: PairingState) {
    tokio::spawn(async move {
        loop {
            state.rotate_code();
            tokio::time::sleep(Duration::from_secs(pairing_code_ttl_seconds())).await;
        }
    });
}
