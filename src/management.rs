use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

const MAX_EVENTS: usize = 2_000;
const MAX_HISTORY: usize = 10_000;
const MAX_SAMPLES: usize = 300;

#[derive(Clone)]
pub struct ManagementState {
    inner: Arc<Mutex<Inner>>,
    updates: broadcast::Sender<()>,
}

struct Inner {
    started: Instant,
    next_session: u64,
    stream: Option<ActiveStream>,
    history: VecDeque<SessionRecord>,
    events: VecDeque<EventRecord>,
    connections: HashMap<String, ConnectionRecord>,
    health: HealthSnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClientMetadata {
    pub device_id: String,
    pub user_agent: String,
    pub platform: String,
    pub language: String,
    pub page_session_id: String,
    pub remote_ip: String,
    pub connection_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConnectionRecord {
    pub connection_id: String,
    pub remote_ip: String,
    pub device_id: String,
    pub user_agent: String,
    pub platform: String,
    pub language: String,
    pub page_session_id: String,
    pub connected: bool,
    pub connected_at_sec: f64,
    pub last_seen_at_sec: f64,
    pub sharing: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StreamConfigSnapshot {
    pub codec: String,
    pub resolution: String,
    pub fps: u32,
    pub bitrate_mbps: f32,
    pub latency_mode: String,
    pub aspect_mode: String,
    pub capture_resolution: String,
    pub encoded_resolution: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricSample {
    pub elapsed_sec: f64,
    pub bitrate_mbps: f64,
    pub fps: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: u64,
    pub sender: Option<ClientMetadata>,
    pub config: StreamConfigSnapshot,
    pub started_at_sec: f64,
    pub ended_at_sec: Option<f64>,
    pub duration_sec: f64,
    pub frames: u64,
    pub bytes: u64,
    pub average_bitrate_mbps: f64,
    pub peak_bitrate_mbps: f64,
    pub sequence_gaps: u64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_max_ms: f64,
    pub end_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRecord {
    pub elapsed_sec: f64,
    pub level: String,
    pub kind: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct HealthSnapshot {
    pub display_resolution: String,
    pub display_fps: u32,
    pub panel_resolution: String,
    pub edid_name: String,
    pub edid_type: String,
    pub pairing_status: String,
    pub cloud_status: String,
    pub decoder_state: String,
    pub queue_depth: usize,
    pub dropped_frames: u64,
    pub rejected_frames: u64,
    pub load_average: String,
    pub memory: String,
    pub temperature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActiveStreamSnapshot {
    pub id: u64,
    pub sender: Option<ClientMetadata>,
    pub config: StreamConfigSnapshot,
    pub started_at_sec: f64,
    pub duration_sec: f64,
    pub frames: u64,
    pub bytes: u64,
    pub measured_bitrate_mbps: f64,
    pub measured_fps: f64,
    pub average_bitrate_mbps: f64,
    pub peak_bitrate_mbps: f64,
    pub sequence_gaps: u64,
    pub server_latency_ms: f64,
    pub samples: Vec<MetricSample>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub server_uptime_sec: f64,
    pub state: String,
    pub active_stream: Option<ActiveStreamSnapshot>,
    pub connections: Vec<ConnectionRecord>,
    pub history: Vec<SessionRecord>,
    pub events: Vec<EventRecord>,
    pub health: HealthSnapshot,
}

struct ActiveStream {
    id: u64,
    sender: Option<ClientMetadata>,
    config: StreamConfigSnapshot,
    started_at: Instant,
    frames: u64,
    bytes: u64,
    last_seq: Option<u32>,
    sequence_gaps: u64,
    latency_total_ms: f64,
    latency_count: u64,
    peak_bitrate_mbps: f64,
    recent_bytes: VecDeque<(Instant, usize)>,
    recent_frames: VecDeque<Instant>,
    samples: VecDeque<MetricSample>,
    last_sample_at: Instant,
    sample_bytes: u64,
    sample_frames: u64,
    latency_window: VecDeque<(Instant, f64)>,
    latency_samples: Vec<f64>,
    last_latency_evaluation: Instant,
    high_latency: bool,
    healthy_latency_windows: u8,
}

impl ManagementState {
    pub fn new() -> Self {
        let (updates, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                started: Instant::now(),
                next_session: 1,
                stream: None,
                history: VecDeque::new(),
                events: VecDeque::new(),
                connections: HashMap::new(),
                health: HealthSnapshot { decoder_state: "idle".into(), ..HealthSnapshot::default() },
            })),
            updates,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<()> { self.updates.subscribe() }

    pub fn event(&self, level: &str, kind: &str, message: impl Into<String>) {
        if let Ok(mut inner) = self.inner.lock() {
            let elapsed = inner.started.elapsed().as_secs_f64();
            inner.events.push_back(EventRecord { elapsed_sec: elapsed, level: level.into(), kind: kind.into(), message: message.into() });
            while inner.events.len() > MAX_EVENTS { inner.events.pop_front(); }
        }
        let _ = self.updates.send(());
    }

    pub fn hello(&self, metadata: ClientMetadata) {
        if let Ok(mut inner) = self.inner.lock() {
            let elapsed = inner.started.elapsed().as_secs_f64();
            inner.connections.insert(metadata.connection_id.clone(), ConnectionRecord {
                connection_id: metadata.connection_id.clone(), remote_ip: metadata.remote_ip.clone(),
                device_id: metadata.device_id.clone(), user_agent: metadata.user_agent.clone(),
                platform: metadata.platform.clone(), language: metadata.language.clone(),
                page_session_id: metadata.page_session_id.clone(), connected: true,
                connected_at_sec: elapsed, last_seen_at_sec: elapsed, sharing: false,
            });
            inner.events.push_back(EventRecord { elapsed_sec: elapsed, level: "info".into(), kind: "connection_metadata".into(), message: format!("connection={} device={} ip={} platform={} user_agent={}", metadata.connection_id, metadata.device_id, metadata.remote_ip, metadata.platform, metadata.user_agent) });
            while inner.events.len() > MAX_EVENTS { inner.events.pop_front(); }
        }
        let _ = self.updates.send(());
    }

    pub fn connection_closed(&self, connection_id: &str) -> bool {
        let mut disconnect_message = format!("connection={connection_id} cause=peer_close");
        let sender_was_active = if let Ok(mut inner) = self.inner.lock() {
            let elapsed = inner.started.elapsed().as_secs_f64();
            if let Some(connection) = inner.connections.get_mut(connection_id) {
                disconnect_message = format!("connection={connection_id} device={} ip={} cause=peer_close duration_sec={:.1}", connection.device_id, connection.remote_ip, elapsed - connection.connected_at_sec);
                connection.connected = false; connection.sharing = false; connection.last_seen_at_sec = elapsed;
            }
            let sender_was_active = inner.stream.as_ref().and_then(|s| s.sender.as_ref()).is_some_and(|s| s.connection_id == connection_id);
            if sender_was_active {
                finish_locked(&mut inner, "disconnect");
            }
            sender_was_active
        } else { false };
        self.event("info", "connection_disconnected", disconnect_message);
        sender_was_active
    }

    /// Refresh activity for a live WebTransport connection. This is separate
    /// from frame accounting so a sender can remain connected while its
    /// browser media pipeline is temporarily paused in a background tab.
    pub fn touch_connection(&self, connection_id: &str) -> bool {
        let touched = if let Ok(mut inner) = self.inner.lock() {
            let elapsed = inner.started.elapsed().as_secs_f64();
            if let Some(connection) = inner.connections.get_mut(connection_id) {
                if connection.connected {
                    connection.last_seen_at_sec = elapsed;
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if touched { let _ = self.updates.send(()); }
        touched
    }

    /// Return whether the sender owning the active stream has recently
    /// touched its authenticated connection. A different paired client must
    /// never keep another client's stream alive.
    pub fn active_sender_heartbeat_fresh(&self, max_age: Duration) -> bool {
        let Ok(inner) = self.inner.lock() else { return false; };
        let Some(sender) = inner.stream.as_ref().and_then(|stream| stream.sender.as_ref()) else {
            return false;
        };
        let Some(connection) = inner.connections.get(&sender.connection_id) else {
            return false;
        };
        if !connection.connected { return false; }
        let now = inner.started.elapsed().as_secs_f64();
        now - connection.last_seen_at_sec <= max_age.as_secs_f64()
    }

    pub fn start(&self, config: StreamConfigSnapshot, sender: Option<ClientMetadata>) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.stream.is_some() { finish_locked(&mut inner, "replaced"); }
            let id = inner.next_session; inner.next_session += 1;
            let now = Instant::now();
            let elapsed = inner.started.elapsed().as_secs_f64();
            if let Some(sender) = sender.as_ref() {
                if let Some(c) = inner.connections.get_mut(&sender.connection_id) { c.sharing = true; c.last_seen_at_sec = elapsed; }
            }
            inner.stream = Some(ActiveStream { id, sender, config, started_at: now, frames: 0, bytes: 0, last_seq: None, sequence_gaps: 0, latency_total_ms: 0.0, latency_count: 0, peak_bitrate_mbps: 0.0, recent_bytes: VecDeque::new(), recent_frames: VecDeque::new(), samples: VecDeque::new(), last_sample_at: now, sample_bytes: 0, sample_frames: 0, latency_window: VecDeque::new(), latency_samples: Vec::new(), last_latency_evaluation: now, high_latency: false, healthy_latency_windows: 0 });
        }
        self.event("info", "stream_start", "sharing started");
    }

    pub fn record_frame(&self, seq: u32, bytes: usize, latency_ms: f64) {
        if let Ok(mut inner) = self.inner.lock() {
            let now = Instant::now();
            let elapsed = inner.started.elapsed().as_secs_f64();
            let sender_connection_id = inner.stream.as_ref()
                .and_then(|stream| stream.sender.as_ref())
                .map(|sender| sender.connection_id.clone());
            if let Some(connection_id) = sender_connection_id {
                if let Some(connection) = inner.connections.get_mut(&connection_id) {
                    connection.last_seen_at_sec = elapsed;
                }
            }
            let mut latency_event = None;
            if let Some(stream) = inner.stream.as_mut() {
                stream.frames += 1; stream.bytes += bytes as u64; stream.sample_bytes += bytes as u64; stream.sample_frames += 1;
                if let Some(last) = stream.last_seq { if seq > last + 1 { stream.sequence_gaps += (seq - last - 1) as u64; } }
                stream.last_seq = Some(seq); stream.latency_total_ms += latency_ms; stream.latency_count += 1;
                stream.recent_bytes.push_back((now, bytes)); stream.recent_frames.push_back(now);
                stream.latency_window.push_back((now, latency_ms));
                while stream.latency_window.front().is_some_and(|(time, _)| now.duration_since(*time) > Duration::from_secs(10)) { stream.latency_window.pop_front(); }
                if stream.latency_samples.len() < 10_000 { stream.latency_samples.push(latency_ms); }
                while stream.recent_bytes.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(10)) { stream.recent_bytes.pop_front(); }
                while stream.recent_frames.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(10)) { stream.recent_frames.pop_front(); }
                let bucket = now.duration_since(stream.last_sample_at).as_secs_f64();
                if bucket >= 1.0 {
                    let mbps = stream.sample_bytes as f64 * 8.0 / bucket / 1_000_000.0;
                    let fps = stream.sample_frames as f64 / bucket;
                    stream.peak_bitrate_mbps = stream.peak_bitrate_mbps.max(mbps);
                    stream.samples.push_back(MetricSample { elapsed_sec: elapsed, bitrate_mbps: mbps, fps });
                    while stream.samples.len() > MAX_SAMPLES { stream.samples.pop_front(); }
                    stream.sample_bytes = 0; stream.sample_frames = 0; stream.last_sample_at = now;
                }
                if now.duration_since(stream.last_latency_evaluation) >= Duration::from_secs(10) && !stream.latency_window.is_empty() {
                    let mut values = stream.latency_window.iter().map(|(_, value)| *value).collect::<Vec<_>>(); values.sort_by(f64::total_cmp);
                    let p95 = percentile(&values, 0.95);
                    let frame_period = if stream.config.fps > 0 { 1000.0 / stream.config.fps as f64 } else { 16.7 };
                    let threshold = (3.0 * frame_period).max(50.0);
                    if p95 > threshold {
                        stream.healthy_latency_windows = 0;
                        if !stream.high_latency { stream.high_latency = true; latency_event = Some(("warn", format!("receiver ingest p95={p95:.1}ms exceeded adaptive budget={threshold:.1}ms for 10s"))); }
                    } else if stream.high_latency {
                        stream.healthy_latency_windows = stream.healthy_latency_windows.saturating_add(1);
                        if stream.healthy_latency_windows >= 2 { stream.high_latency = false; stream.healthy_latency_windows = 0; latency_event = Some(("info", format!("receiver ingest latency recovered p95={p95:.1}ms budget={threshold:.1}ms"))); }
                    }
                    stream.last_latency_evaluation = now;
                }
            }
            if let Some((level, message)) = latency_event { inner.events.push_back(EventRecord { elapsed_sec: elapsed, level: level.into(), kind: "latency".into(), message }); while inner.events.len() > MAX_EVENTS { inner.events.pop_front(); } }
        }
        let _ = self.updates.send(());
    }

    pub fn stop(&self, reason: &str) -> bool {
        let changed = if let Ok(mut inner) = self.inner.lock() { if inner.stream.is_some() { finish_locked(&mut inner, reason); true } else { false } } else { false };
        if changed { self.event("info", "stream_stop", reason.to_string()); }
        changed
    }

    pub fn set_health(&self, health: HealthSnapshot) { if let Ok(mut inner) = self.inner.lock() { inner.health = health; } let _ = self.updates.send(()); }

    pub fn refresh_system_health(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Ok(load) = std::fs::read_to_string("/proc/loadavg") { inner.health.load_average = load.split_whitespace().take(3).collect::<Vec<_>>().join(" "); }
            if let Ok(memory) = std::fs::read_to_string("/proc/meminfo") {
                let mut total = 0u64; let mut available = 0u64;
                for line in memory.lines() { if let Some(value) = line.strip_prefix("MemTotal:") { total = value.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0); } if let Some(value) = line.strip_prefix("MemAvailable:") { available = value.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0); } }
                if total > 0 { inner.health.memory = format!("{} / {} MiB available", available / 1024, total / 1024); }
            }
            if let Ok(temp) = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp") { if let Ok(millidegrees) = temp.trim().parse::<f64>() { inner.health.temperature = format!("{:.1} °C", millidegrees / 1000.0); } }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let Ok(inner) = self.inner.lock() else { return Snapshot { server_uptime_sec: 0.0, state: "ERROR".into(), active_stream: None, connections: vec![], history: vec![], events: vec![], health: HealthSnapshot::default() }; };
        let now = Instant::now();
        let active_stream = inner.stream.as_ref().map(|stream| {
            let window_start = now - Duration::from_secs(1);
            let bytes_1s: usize = stream.recent_bytes.iter().filter(|(t, _)| *t >= window_start).map(|(_, b)| *b).sum();
            let frames_1s = stream.recent_frames.iter().filter(|t| **t >= window_start).count();
            let ten_bytes: usize = stream.recent_bytes.iter().map(|(_, b)| *b).sum();
            let avg = if stream.started_at.elapsed().as_secs_f64() > 0.0 { stream.bytes as f64 * 8.0 / stream.started_at.elapsed().as_secs_f64() / 1_000_000.0 } else { 0.0 };
            ActiveStreamSnapshot { id: stream.id, sender: stream.sender.clone(), config: stream.config.clone(), started_at_sec: stream.started_at.duration_since(inner.started).as_secs_f64(), duration_sec: stream.started_at.elapsed().as_secs_f64(), frames: stream.frames, bytes: stream.bytes, measured_bitrate_mbps: bytes_1s as f64 * 8.0 / 1_000_000.0, measured_fps: frames_1s as f64, average_bitrate_mbps: avg.max(if stream.started_at.elapsed().as_secs_f64() > 0.0 { ten_bytes as f64 * 8.0 / stream.started_at.elapsed().as_secs_f64() / 1_000_000.0 } else { 0.0 }), peak_bitrate_mbps: stream.peak_bitrate_mbps, sequence_gaps: stream.sequence_gaps, server_latency_ms: if stream.latency_count > 0 { stream.latency_total_ms / stream.latency_count as f64 } else { 0.0 }, samples: stream.samples.iter().cloned().collect() }
        });
        Snapshot { server_uptime_sec: inner.started.elapsed().as_secs_f64(), state: if active_stream.is_some() { "STREAMING".into() } else { "IDLE".into() }, active_stream, connections: inner.connections.values().cloned().collect(), history: inner.history.iter().cloned().collect(), events: inner.events.iter().cloned().collect(), health: inner.health.clone() }
    }
}

fn finish_locked(inner: &mut Inner, reason: &str) {
    let Some(stream) = inner.stream.take() else { return; };
    let now = Instant::now(); let duration = stream.started_at.elapsed().as_secs_f64();
    let avg = if duration > 0.0 { stream.bytes as f64 * 8.0 / duration / 1_000_000.0 } else { 0.0 };
    if let Some(sender) = stream.sender.as_ref() { if let Some(c) = inner.connections.get_mut(&sender.connection_id) { c.sharing = false; } }
    let mut latency = stream.latency_samples; latency.sort_by(f64::total_cmp);
    inner.history.push_front(SessionRecord { id: stream.id, sender: stream.sender, config: stream.config, started_at_sec: stream.started_at.duration_since(inner.started).as_secs_f64(), ended_at_sec: Some(now.duration_since(inner.started).as_secs_f64()), duration_sec: duration, frames: stream.frames, bytes: stream.bytes, average_bitrate_mbps: avg, peak_bitrate_mbps: stream.peak_bitrate_mbps, sequence_gaps: stream.sequence_gaps, latency_p50_ms: percentile(&latency, 0.50), latency_p95_ms: percentile(&latency, 0.95), latency_max_ms: latency.last().copied().unwrap_or(0.0), end_reason: Some(reason.into()) });
    while inner.history.len() > MAX_HISTORY { inner.history.pop_back(); }
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() { return 0.0; }
    sorted[((sorted.len() - 1) as f64 * quantile).round() as usize]
}

impl Default for ManagementState { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> StreamConfigSnapshot {
        StreamConfigSnapshot { codec: "H265".into(), resolution: "1920x1088".into(), fps: 30, bitrate_mbps: 6.0, latency_mode: "ULL".into(), aspect_mode: "preserve".into(), capture_resolution: "1920x1080".into(), encoded_resolution: "1920x1088".into() }
    }

    fn client(connection_id: &str) -> ClientMetadata {
        ClientMetadata {
            device_id: connection_id.into(), user_agent: "test".into(), platform: "test".into(),
            language: "en".into(), page_session_id: format!("page-{connection_id}"),
            remote_ip: "127.0.0.1".into(), connection_id: connection_id.into(),
        }
    }

    #[test]
    fn lifecycle_records_frames_and_stop_reason() {
        let state = ManagementState::new();
        state.start(config(), None);
        state.record_frame(1, 1_000, 2.0);
        state.record_frame(3, 2_000, 4.0);
        assert_eq!(state.snapshot().active_stream.as_ref().map(|s| s.sequence_gaps), Some(1));
        assert!(state.stop("admin_stop"));
        let snapshot = state.snapshot();
        assert!(snapshot.active_stream.is_none());
        assert_eq!(snapshot.history[0].end_reason.as_deref(), Some("admin_stop"));
        assert_eq!(snapshot.history[0].bytes, 3_000);
    }

    #[test]
    fn duplicate_stop_is_idempotent() {
        let state = ManagementState::new();
        assert!(!state.stop("admin_stop"));
        state.start(config(), None);
        assert!(state.stop("user_stop"));
        assert!(!state.stop("admin_stop"));
        assert_eq!(state.snapshot().history.len(), 1);
    }

    #[test]
    fn connection_activity_refreshes_only_live_connections() {
        let state = ManagementState::new();
        state.hello(client("sender"));
        assert!(state.touch_connection("sender"));
        state.connection_closed("sender");
        assert!(!state.touch_connection("sender"));
    }

    #[test]
    fn active_sender_heartbeat_is_scoped_to_stream_owner() {
        let state = ManagementState::new();
        state.hello(client("sender"));
        state.hello(client("other"));
        state.start(config(), Some(client("sender")));
        assert!(state.active_sender_heartbeat_fresh(Duration::from_secs(1)));
        state.connection_closed("sender");
        assert!(!state.active_sender_heartbeat_fresh(Duration::from_secs(1)));
    }
}
