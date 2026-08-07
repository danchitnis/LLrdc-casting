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
    Start {
        codec: Option<String>,
        resolution: Option<String>,
        fps: Option<u32>,
        #[serde(default)]
        bitrate_mbps: Option<f32>,
        #[serde(default)]
        latency_mode: Option<String>,
        #[serde(default)]
        aspect_mode: Option<String>,
        #[serde(default)]
        source_width: Option<u32>,
        #[serde(default)]
        source_height: Option<u32>,
        #[serde(default)]
        encoded_width: Option<u32>,
        #[serde(default)]
        encoded_height: Option<u32>,
        #[serde(default)]
        content_rect: Option<String>,
        #[serde(default)]
        signal_content_rect: Option<String>,
        #[serde(default)]
        panel_content_rect: Option<String>,
        #[serde(default)]
        signal_width: Option<u32>,
        #[serde(default)]
        signal_height: Option<u32>,
        #[serde(default)]
        panel_width: Option<u32>,
        #[serde(default)]
        panel_height: Option<u32>,
    },
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
        #[serde(default)]
        latency_ms: f32,
        #[serde(default)]
        display_resolution: String,
        #[serde(default)]
        display_fps: u32,
        #[serde(default)]
        bitrate_mbps: f32,
        #[serde(default)]
        latency_mode: String,
        #[serde(default)]
        edid_name: String,
        #[serde(default)]
        edid_type: String,
        #[serde(default)]
        edid_max_res: String,
        #[serde(default)]
        edid_max_fps: u32,
        #[serde(default)]
        capture_resolution: String,
        #[serde(default)]
        encoded_resolution: String,
        #[serde(default)]
        aspect_mode: String,
        #[serde(default)]
        content_rect: String,
        #[serde(default)]
        signal_resolution: String,
        #[serde(default)]
        panel_resolution: String,
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
