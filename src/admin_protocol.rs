use serde::{Deserialize, Serialize};

use crate::local_pairing::PairingSnapshot;
use crate::management::Snapshot;

pub const RECEIVER_SOCKET_PATH: &str = "/run/llrdc/receiver.sock";
pub const MANAGEMENT_SOCKET_PATH: &str = "/run/llrdc/management.sock";
pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiverRequest {
    Ping { version: u8 },
    Snapshot { version: u8 },
    StopSharing { version: u8 },
    Shutdown { version: u8, reason: String },
    PairingCode { version: u8 },
}

impl ReceiverRequest {
    pub fn version(&self) -> u8 {
        match self {
            Self::Ping { version }
            | Self::Snapshot { version }
            | Self::StopSharing { version }
            | Self::Shutdown { version, .. }
            | Self::PairingCode { version } => *version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReceiverResponse {
    Pong { version: u8, ready: bool },
    Snapshot { version: u8, ready: bool, management: Snapshot, pairing: PairingSnapshot },
    PairingCode { version: u8, code: String },
    Ack { version: u8 },
    Error { version: u8, code: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_schema_is_strict_and_versioned() {
        let request: ReceiverRequest = serde_json::from_str(r#"{"type":"ping","version":1}"#).unwrap();
        assert_eq!(request.version(), PROTOCOL_VERSION);
        assert!(serde_json::from_str::<ReceiverRequest>(r#"{"type":"ping","version":1,"extra":true}"#).is_err());
    }
}
