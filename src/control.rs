/*
 * Independent Control & Telemetry Module
 * Handles JSON commands (start, stop, status, ping) and live device telemetry
 * over a dedicated WebSocket control channel.
 */

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};

use crate::config::telemetry::DEFAULT_TELEMETRY_CHANNEL_CAPACITY;

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
        #[serde(default)]
        connection_id: Option<String>,
        #[serde(default)]
        device_id: Option<String>,
    },
    Stop,
    AdminStop,
    RestartReceiver,
    Shutdown { reason: String },
    ClientHello {
        device_id: String,
        #[serde(default)]
        user_agent: String,
        #[serde(default)]
        platform: String,
        #[serde(default)]
        language: String,
        #[serde(default)]
        page_session_id: String,
        #[serde(default)]
        connection_id: Option<String>,
        #[serde(default)]
        remote_ip: Option<String>,
    },
    /// Application-level RTT probe. The optional ID keeps older clients that
    /// sent `{ "type": "ping" }` compatible with the control protocol.
    Ping {
        #[serde(default)]
        id: Option<u64>,
    },
    GetStatus,
    /// Browser-side diagnostic forwarded over the authenticated control stream.
    /// The receiver derives the originating connection id from the stream and
    /// must never trust a client-supplied identity.
    ClientDiagnostic {
        level: String,
        message: String,
    },
    /// Sender-computed encoder-input-to-display estimate. The transport
    /// boundary attributes this to the authenticated connection before it is
    /// exposed by the management portal.
    LatencyReport {
        seq: u32,
        total_ms: f64,
        encode_ms: f64,
        transport_queue_ms: f64,
        decode_display_ms: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TelemetryMessage {
    Pong {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<u64>,
    },
    LatencySample {
        seq: u32,
        capture_time_ms: f64,
        encode_duration_ms: f32,
    },
    Status {
        state: String,
        resolution: String,
        fps: u32,
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
        /// Highest refresh rate advertised by the monitor EDID. This is a
        /// capability, not necessarily the refresh rate of the active HDMI
        /// mode (`display_fps`).
        #[serde(default)]
        display_max_fps: u32,
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
        let (telemetry_tx, _) = broadcast::channel(DEFAULT_TELEMETRY_CHANNEL_CAPACITY);
        Self {
            cmd_tx,
            telemetry_tx,
        }
    }

    pub fn send_telemetry(&self, msg: TelemetryMessage) {
        let _ = self.telemetry_tx.send(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::{ControlCommand, TelemetryMessage};

    #[test]
    fn latency_sample_uses_snake_case_wire_fields() {
        let json = serde_json::to_value(TelemetryMessage::LatencySample {
            seq: 30,
            capture_time_ms: 1_725_000_000_123.5,
            encode_duration_ms: 7.25,
        }).unwrap();
        assert_eq!(json["type"], "latency_sample");
        assert_eq!(json["seq"], 30);
        assert_eq!(json["capture_time_ms"], 1_725_000_000_123.5);
        assert_eq!(json["encode_duration_ms"], 7.25);
    }

    #[test]
    fn latency_report_uses_snake_case_wire_fields() {
        let command: ControlCommand = serde_json::from_value(serde_json::json!({
            "type": "latency_report",
            "seq": 31,
            "total_ms": 29.5,
            "encode_ms": 7.0,
            "transport_queue_ms": 5.8,
            "decode_display_ms": 16.7
        })).unwrap();
        assert!(matches!(command, ControlCommand::LatencyReport { seq: 31, .. }));
    }
}
