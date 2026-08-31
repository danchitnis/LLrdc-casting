use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_REQUEST_DIR: &str = "/updates/requests";
const DEFAULT_STATUS_FILE: &str = "/updates/status/status.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UpdateStatus {
    pub state: String,
    pub current_digest: Option<String>,
    pub available_digest: Option<String>,
    pub current_version: Option<String>,
    pub message: Option<String>,
    pub updated_at_unix: Option<u64>,
    #[serde(alias = "managed")]
    pub installed: bool,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            current_digest: None,
            available_digest: None,
            current_version: Some(option_env!("LLRDC_BUILD_REVISION").unwrap_or(env!("CARGO_PKG_VERSION")).into()),
            message: Some("The host updater is not installed on this device.".into()),
            updated_at_unix: None,
            installed: false,
        }
    }
}

fn request_dir() -> PathBuf {
    std::env::var("LLRDC_UPDATE_REQUEST_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(DEFAULT_REQUEST_DIR))
}

fn status_file() -> PathBuf {
    std::env::var("LLRDC_UPDATE_STATUS_FILE").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(DEFAULT_STATUS_FILE))
}

pub fn status() -> UpdateStatus {
    let path = status_file();
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<UpdateStatus>(&bytes).ok())
        .map(|mut value| { value.installed = true; value })
        .unwrap_or_default()
}

pub fn request(action: &str) -> Result<(), String> {
    if !matches!(action, "check" | "apply") {
        return Err("invalid update action".into());
    }
    let directory = request_dir();
    if !directory.is_dir() {
        return Err("host updater unavailable".into());
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| "system clock unavailable")?;
    let name = format!("{}-{}-{}.request", action, now.as_secs(), now.subsec_nanos());
    let temporary = directory.join(format!(".{name}.tmp"));
    let destination = directory.join(name);
    fs::write(&temporary, b"1\n").map_err(|error| format!("update request failed: {error}"))?;
    fs::rename(&temporary, &destination).map_err(|error| format!("update request failed: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::UpdateStatus;

    #[test]
    fn status_schema_round_trips() {
        let status = UpdateStatus {
            state: "available".into(), current_digest: Some("sha256:old".into()),
            available_digest: Some("sha256:new".into()), current_version: Some("abc123".into()),
            message: None, updated_at_unix: Some(42), installed: true,
        };
        let encoded = serde_json::to_vec(&status).unwrap();
        let decoded: UpdateStatus = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.state, "available");
        assert!(decoded.installed);
    }

    #[test]
    fn legacy_managed_status_is_accepted_during_device_migration() {
        let decoded: UpdateStatus = serde_json::from_str(r#"{"state":"idle","current_digest":null,"available_digest":null,"current_version":null,"message":null,"updated_at_unix":null,"managed":true}"#).unwrap();
        assert!(decoded.installed);
    }
}
