/*
 * Independent Control & Telemetry Module
 * Handles JSON commands (start, stop, status, ping) and live device telemetry
 * over a dedicated WebSocket control channel.
 */

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlCommand {
    Start { codec: Option<String>, resolution: Option<String> },
    Stop,
    Ping,
    GetStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TelemetryMessage {
    Pong,
    Status {
        state: String,
        resolution: String,
        fps: u32,
        delivery_rate: f32,
        frames_submitted: u64,
    },
    Event {
        level: String,
        message: String,
    },
}

#[derive(Clone)]
pub struct ControlChannel {
    pub cmd_tx: mpsc::Sender<ControlCommand>,
    pub telemetry_tx: broadcast::Sender<TelemetryMessage>,
}

impl ControlChannel {
    pub fn new(cmd_tx: mpsc::Sender<ControlCommand>) -> Self {
        let (telemetry_tx, _) = broadcast::channel(100);
        Self {
            cmd_tx,
            telemetry_tx,
        }
    }

    pub fn send_telemetry(&self, msg: TelemetryMessage) {
        let _ = self.telemetry_tx.send(msg);
    }
}
