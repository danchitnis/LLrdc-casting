use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

const MAX_EVENTS: usize = 2_000;
const MAX_HISTORY: usize = 10_000;
const MAX_SAMPLES: usize = 300;
const ESTIMATED_LATENCY_MAX_MS: f64 = 30_000.0;
const CLOCK_DRIFT_MS_PER_MS: f64 = 0.00005;

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
    clock_syncs: HashMap<String, ClockSyncState>,
    health: HealthSnapshot,
}

#[derive(Clone, Copy)]
struct ClockSyncState {
    offset_ms: f64,
    uncertainty_ms: f64,
    received_at: Instant,
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
pub struct LatencyMetricSample {
    pub elapsed_sec: f64,
    pub total_ms: f64,
    pub encode_ms: f64,
    pub sender_queue_ms: f64,
    pub delivery_ms: f64,
    pub receiver_queue_ms: f64,
    pub decode_display_ms: f64,
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
    pub playback_state: String,
    pub reassembly_in_flight: usize,
    pub dropped_access_units: u64,
    pub ignored_media_packets: u64,
    pub load_1m: Option<f64>,
    pub load_5m: Option<f64>,
    pub load_15m: Option<f64>,
    pub memory_available_mib: Option<u64>,
    pub memory_total_mib: Option<u64>,
    pub soc_temperature_c: Option<f64>,
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
    pub estimated_latency: Option<EstimatedLatencySnapshot>,
    pub estimated_latency_age_ms: Option<f64>,
    pub samples: Vec<MetricSample>,
    pub latency_samples: Vec<LatencyMetricSample>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EstimatedLatencySnapshot {
    pub seq: u32,
    pub total_ms: f64,
    pub encode_ms: f64,
    pub sender_queue_ms: f64,
    pub delivery_ms: f64,
    pub receiver_queue_ms: f64,
    /// Deprecated aggregate retained for older management consumers.
    pub transport_queue_ms: f64,
    pub decode_display_ms: f64,
    pub access_unit_bytes: u64,
    pub media_write_blocked_ms: f64,
    pub clock_uncertainty_ms: f64,
    pub clock_sync_age_ms: f64,
    pub configured_bitrate_mbps: f64,
    pub adaptive_bitrate_mbps: f64,
    pub dropped_input_frames: u64,
    pub effective_fps: f64,
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
    estimated_latency: Option<(Instant, EstimatedLatencySnapshot)>,
    estimated_latency_samples: VecDeque<LatencyMetricSample>,
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
                clock_syncs: HashMap::new(),
                health: HealthSnapshot { playback_state: "idle_dashboard".into(), ..HealthSnapshot::default() },
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
            inner.clock_syncs.remove(connection_id);
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

    pub fn record_clock_sync(&self, connection_id: &str, offset_ms: f64, uncertainty_ms: f64) -> bool {
        if !offset_ms.is_finite() || !uncertainty_ms.is_finite() || !(0.0..=ESTIMATED_LATENCY_MAX_MS).contains(&uncertainty_ms) {
            return false;
        }
        let accepted = if let Ok(mut inner) = self.inner.lock() {
            if inner.connections.get(connection_id).is_some_and(|connection| connection.connected) {
                inner.clock_syncs.insert(connection_id.to_string(), ClockSyncState { offset_ms, uncertainty_ms, received_at: Instant::now() });
                true
            } else { false }
        } else { false };
        if accepted { let _ = self.updates.send(()); }
        accepted
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
                inner.clock_syncs.remove(&sender.connection_id);
                if let Some(c) = inner.connections.get_mut(&sender.connection_id) { c.sharing = true; c.last_seen_at_sec = elapsed; }
            }
            inner.stream = Some(ActiveStream { id, sender, config, started_at: now, frames: 0, bytes: 0, last_seq: None, sequence_gaps: 0, latency_total_ms: 0.0, latency_count: 0, peak_bitrate_mbps: 0.0, recent_bytes: VecDeque::new(), recent_frames: VecDeque::new(), samples: VecDeque::new(), last_sample_at: now, sample_bytes: 0, sample_frames: 0, latency_window: VecDeque::new(), latency_samples: Vec::new(), last_latency_evaluation: now, high_latency: false, healthy_latency_windows: 0, estimated_latency: None, estimated_latency_samples: VecDeque::new() });
        }
        self.event("info", "stream_start", "sharing started");
    }

    /// Record a sender-computed end-to-end estimate only when it belongs to
    /// the authenticated owner of the active stream and is internally
    /// consistent. Returns false for stale, spoofed, or implausible reports.
    pub fn record_estimated_latency(
        &self,
        connection_id: &str,
        sample: EstimatedLatencySnapshot,
    ) -> bool {
        let aggregate = sample.sender_queue_ms + sample.delivery_ms + sample.receiver_queue_ms;
        let components_total = sample.encode_ms + aggregate + sample.decode_display_ms;
        let values = [sample.total_ms, sample.encode_ms, sample.sender_queue_ms, sample.delivery_ms,
            sample.receiver_queue_ms, sample.decode_display_ms, sample.media_write_blocked_ms,
            sample.clock_uncertainty_ms, sample.clock_sync_age_ms, sample.configured_bitrate_mbps,
            sample.adaptive_bitrate_mbps, sample.effective_fps];
        let valid_values = values.into_iter().all(|value| value.is_finite() && (0.0..=ESTIMATED_LATENCY_MAX_MS).contains(&value));
        if sample.seq == 0 || !valid_values || (sample.total_ms - components_total).abs() > 2.0
            || (sample.transport_queue_ms - aggregate).abs() > 2.0 {
            return false;
        }

        let accepted = if let Ok(mut inner) = self.inner.lock() {
            let Some(stream) = inner.stream.as_mut() else { return false; };
            let owned = stream.sender.as_ref().is_some_and(|sender| sender.connection_id == connection_id);
            let previous_seq = stream.estimated_latency.as_ref().map(|(_, previous)| previous.seq);
            let newer = previous_seq.is_none_or(|previous| sample.seq > previous);
            let same = previous_seq == Some(sample.seq);
            let submitted = stream.last_seq.is_some_and(|last_seq| sample.seq <= last_seq);
            if owned && (newer || same) && submitted {
                if newer {
                    stream.estimated_latency_samples.push_back(LatencyMetricSample {
                        elapsed_sec: stream.started_at.elapsed().as_secs_f64(),
                        total_ms: sample.total_ms,
                        encode_ms: sample.encode_ms,
                        sender_queue_ms: sample.sender_queue_ms,
                        delivery_ms: sample.delivery_ms,
                        receiver_queue_ms: sample.receiver_queue_ms,
                        decode_display_ms: sample.decode_display_ms,
                    });
                    while stream.estimated_latency_samples.len() > MAX_SAMPLES {
                        stream.estimated_latency_samples.pop_front();
                    }
                }
                stream.estimated_latency = Some((Instant::now(), sample));
                true
            } else {
                false
            }
        } else {
            false
        };
        if accepted { let _ = self.updates.send(()); }
        accepted
    }

    /// Record portal latency directly at the receiver so background-tab
    /// throttling cannot interrupt management sampling.
    pub fn record_receiver_estimated_latency(
        &self,
        seq: u32,
        capture_time_ms: f64,
        encode_duration_ms: f32,
        send_start_time_ms: f64,
        receiver_complete_time_ms: f64,
        receiver_queue_ms: f64,
        display_fps: u32,
    ) -> bool {
        if display_fps == 0 || seq == 0 { return false; }
        let accepted = if let Ok(mut inner) = self.inner.lock() {
            let Some(connection_id) = inner.stream.as_ref().and_then(|stream| stream.sender.as_ref()).map(|sender| sender.connection_id.clone()) else { return false; };
            let Some(sync) = inner.clock_syncs.get(&connection_id).copied() else { return false; };
            let sync_age_ms = sync.received_at.elapsed().as_secs_f64() * 1_000.0;
            let uncertainty_ms = sync.uncertainty_ms + sync_age_ms * CLOCK_DRIFT_MS_PER_MS;
            let sender_raw_ms = send_start_time_ms - capture_time_ms - f64::from(encode_duration_ms);
            let delivery_raw_ms = receiver_complete_time_ms - sync.offset_ms - send_start_time_ms;
            if ![capture_time_ms, f64::from(encode_duration_ms), send_start_time_ms, receiver_complete_time_ms,
                receiver_queue_ms, uncertainty_ms, sender_raw_ms, delivery_raw_ms].into_iter().all(f64::is_finite)
                || sender_raw_ms < -uncertainty_ms || delivery_raw_ms < -uncertainty_ms || receiver_queue_ms < 0.0 {
                return false;
            }
            let Some(stream) = inner.stream.as_mut() else { return false; };
            if stream.estimated_latency.as_ref().is_some_and(|(_, previous)| seq <= previous.seq) { return false; }
            let previous = stream.estimated_latency.as_ref().map(|(_, sample)| sample.clone());
            let blend = |previous: f64, current: f64| previous + 0.25 * (current - previous);
            let encode_ms = previous.as_ref().map_or(f64::from(encode_duration_ms), |sample| blend(sample.encode_ms, f64::from(encode_duration_ms)));
            let sender_queue_ms = previous.as_ref().map_or(sender_raw_ms.max(0.0), |sample| blend(sample.sender_queue_ms, sender_raw_ms.max(0.0)));
            let delivery_ms = previous.as_ref().map_or(delivery_raw_ms.max(0.0), |sample| blend(sample.delivery_ms, delivery_raw_ms.max(0.0)));
            let receiver_queue_ms = previous.as_ref().map_or(receiver_queue_ms, |sample| blend(sample.receiver_queue_ms, receiver_queue_ms));
            let decode_display_ms = 1_000.0 / f64::from(display_fps);
            let total_ms = encode_ms + sender_queue_ms + delivery_ms + receiver_queue_ms + decode_display_ms;
            if total_ms > ESTIMATED_LATENCY_MAX_MS { return false; }
            let sample = EstimatedLatencySnapshot {
                seq,
                total_ms,
                encode_ms,
                sender_queue_ms,
                delivery_ms,
                receiver_queue_ms,
                transport_queue_ms: sender_queue_ms + delivery_ms + receiver_queue_ms,
                decode_display_ms,
                access_unit_bytes: previous.as_ref().map_or(0, |sample| sample.access_unit_bytes),
                media_write_blocked_ms: previous.as_ref().map_or(0.0, |sample| sample.media_write_blocked_ms),
                clock_uncertainty_ms: uncertainty_ms,
                clock_sync_age_ms: sync_age_ms,
                configured_bitrate_mbps: previous.as_ref().map_or(f64::from(stream.config.bitrate_mbps), |sample| sample.configured_bitrate_mbps),
                adaptive_bitrate_mbps: previous.as_ref().map_or(f64::from(stream.config.bitrate_mbps), |sample| sample.adaptive_bitrate_mbps),
                dropped_input_frames: previous.as_ref().map_or(0, |sample| sample.dropped_input_frames),
                effective_fps: previous.as_ref().map_or(f64::from(stream.config.fps), |sample| sample.effective_fps),
            };
            stream.estimated_latency_samples.push_back(LatencyMetricSample {
                elapsed_sec: stream.started_at.elapsed().as_secs_f64(), total_ms, encode_ms: sample.encode_ms,
                sender_queue_ms, delivery_ms, receiver_queue_ms, decode_display_ms,
            });
            while stream.estimated_latency_samples.len() > MAX_SAMPLES { stream.estimated_latency_samples.pop_front(); }
            stream.estimated_latency = Some((Instant::now(), sample));
            true
        } else { false };
        if accepted { let _ = self.updates.send(()); }
        accepted
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
                let stream_elapsed = stream.started_at.elapsed().as_secs_f64();
                stream.frames += 1; stream.bytes += bytes as u64; stream.sample_bytes += bytes as u64; stream.sample_frames += 1;
                if let Some(last) = stream.last_seq { if seq > last + 1 { stream.sequence_gaps += (seq - last - 1) as u64; } }
                stream.last_seq = Some(seq); stream.latency_total_ms += latency_ms; stream.latency_count += 1;
                stream.recent_bytes.push_back((now, bytes)); stream.recent_frames.push_back(now);
                let rolling_bytes: usize = stream.recent_bytes.iter()
                    .filter(|(time, _)| now.duration_since(*time) <= Duration::from_secs(1))
                    .map(|(_, size)| *size)
                    .sum();
                stream.peak_bitrate_mbps = stream.peak_bitrate_mbps
                    .max(rolling_bytes as f64 * 8.0 / 1_000_000.0);
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
                    stream.samples.push_back(MetricSample { elapsed_sec: stream_elapsed, bitrate_mbps: mbps, fps });
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

    pub fn refresh_pipeline_health(
        &self,
        playback_state: &str,
        stats: crate::v4l2_decoder::ReassemblyStats,
    ) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.health.playback_state = playback_state.into();
            inner.health.reassembly_in_flight = stats.in_flight;
            inner.health.dropped_access_units = stats.dropped_access_units;
            inner.health.ignored_media_packets = stats.ignored_packets;
        }
        let _ = self.updates.send(());
    }

    pub fn refresh_system_health(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            let load = std::fs::read_to_string("/proc/loadavg").ok().and_then(|value| parse_load_average(&value));
            (inner.health.load_1m, inner.health.load_5m, inner.health.load_15m) = load
                .map_or((None, None, None), |(one, five, fifteen)| (Some(one), Some(five), Some(fifteen)));
            let memory = std::fs::read_to_string("/proc/meminfo").ok().and_then(|value| parse_memory_info(&value));
            (inner.health.memory_available_mib, inner.health.memory_total_mib) = memory
                .map_or((None, None), |(available, total)| (Some(available), Some(total)));
            inner.health.soc_temperature_c = read_soc_temperature();
        }
        let _ = self.updates.send(());
    }

    pub fn snapshot(&self) -> Snapshot {
        let Ok(inner) = self.inner.lock() else { return Snapshot { server_uptime_sec: 0.0, state: "ERROR".into(), active_stream: None, connections: vec![], history: vec![], events: vec![], health: HealthSnapshot::default() }; };
        let now = Instant::now();
        let active_stream = inner.stream.as_ref().map(|stream| {
            let window_start = now - Duration::from_secs(1);
            let bytes_1s: usize = stream.recent_bytes.iter().filter(|(t, _)| *t >= window_start).map(|(_, b)| *b).sum();
            let frames_1s = stream.recent_frames.iter().filter(|t| **t >= window_start).count();
            let avg = if stream.started_at.elapsed().as_secs_f64() > 0.0 { stream.bytes as f64 * 8.0 / stream.started_at.elapsed().as_secs_f64() / 1_000_000.0 } else { 0.0 };
            let estimated_latency = stream.estimated_latency.as_ref().map(|(_, sample)| sample.clone());
            let estimated_latency_age_ms = stream.estimated_latency.as_ref()
                .map(|(received_at, _)| now.duration_since(*received_at).as_secs_f64() * 1_000.0);
            ActiveStreamSnapshot { id: stream.id, sender: stream.sender.clone(), config: stream.config.clone(), started_at_sec: stream.started_at.duration_since(inner.started).as_secs_f64(), duration_sec: stream.started_at.elapsed().as_secs_f64(), frames: stream.frames, bytes: stream.bytes, measured_bitrate_mbps: bytes_1s as f64 * 8.0 / 1_000_000.0, measured_fps: frames_1s as f64, average_bitrate_mbps: avg, peak_bitrate_mbps: stream.peak_bitrate_mbps, sequence_gaps: stream.sequence_gaps, server_latency_ms: if stream.latency_count > 0 { stream.latency_total_ms / stream.latency_count as f64 } else { 0.0 }, estimated_latency, estimated_latency_age_ms, samples: stream.samples.iter().cloned().collect(), latency_samples: stream.estimated_latency_samples.iter().cloned().collect() }
        });
        Snapshot { server_uptime_sec: inner.started.elapsed().as_secs_f64(), state: if active_stream.is_some() { "STREAMING".into() } else { "IDLE".into() }, active_stream, connections: inner.connections.values().cloned().collect(), history: inner.history.iter().cloned().collect(), events: inner.events.iter().cloned().collect(), health: inner.health.clone() }
    }
}

fn parse_load_average(value: &str) -> Option<(f64, f64, f64)> {
    let mut fields = value.split_whitespace();
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

fn parse_memory_info(value: &str) -> Option<(u64, u64)> {
    let mut total_kib: Option<u64> = None;
    let mut available_kib: Option<u64> = None;
    for line in value.lines() {
        let mut fields = line.split_whitespace();
        let Some(key) = fields.next() else { continue; };
        match key {
            "MemTotal:" => total_kib = fields.next().and_then(|field| field.parse().ok()),
            "MemAvailable:" => available_kib = fields.next().and_then(|field| field.parse().ok()),
            _ => {}
        }
    }
    Some((available_kib? / 1024, total_kib? / 1024))
}

fn read_soc_temperature() -> Option<f64> {
    let zones = std::fs::read_dir("/sys/class/thermal").ok()?;
    let mut fallback = None;
    for zone in zones.flatten() {
        let path = zone.path();
        if !zone.file_name().to_string_lossy().starts_with("thermal_zone") { continue; }
        let temperature = std::fs::read_to_string(path.join("temp")).ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .map(|millidegrees| millidegrees / 1000.0)
            .filter(|value| value.is_finite() && (-40.0..=150.0).contains(value));
        let Some(temperature) = temperature else { continue; };
        let kind = std::fs::read_to_string(path.join("type")).unwrap_or_default().to_ascii_lowercase();
        if kind.contains("cpu") || kind.contains("soc") || kind.contains("package") { return Some(temperature); }
        fallback.get_or_insert(temperature);
    }
    fallback
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

    fn latency_sample(seq: u32) -> EstimatedLatencySnapshot {
        EstimatedLatencySnapshot {
            seq, total_ms: 29.5, encode_ms: 7.0, sender_queue_ms: 1.0,
            delivery_ms: 2.0, receiver_queue_ms: 2.8, transport_queue_ms: 5.8,
            decode_display_ms: 16.7, access_unit_bytes: 100_000,
            media_write_blocked_ms: 1.0, clock_uncertainty_ms: 0.5,
            clock_sync_age_ms: 100.0, configured_bitrate_mbps: 8.0,
            adaptive_bitrate_mbps: 6.4, dropped_input_frames: 2, effective_fps: 29.0,
        }
    }

    #[test]
    fn lifecycle_records_frames_and_stop_reason() {
        let state = ManagementState::new();
        state.start(config(), None);
        state.record_frame(1, 1_000, 2.0);
        state.record_frame(3, 2_000, 4.0);
        let active = state.snapshot().active_stream.expect("active stream");
        assert_eq!(active.sequence_gaps, 1);
        assert!(active.peak_bitrate_mbps > 0.0);
        assert!(state.stop("admin_stop"));
        let snapshot = state.snapshot();
        assert!(snapshot.active_stream.is_none());
        assert_eq!(snapshot.history[0].end_reason.as_deref(), Some("admin_stop"));
        assert_eq!(snapshot.history[0].bytes, 3_000);
        assert!(snapshot.history[0].peak_bitrate_mbps > 0.0);
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

    #[test]
    fn estimated_latency_is_validated_and_scoped_to_stream_owner() {
        let state = ManagementState::new();
        state.hello(client("sender"));
        state.hello(client("other"));
        state.start(config(), Some(client("sender")));
        state.record_frame(7, 1_000, 2.0);
        let sample = latency_sample(7);
        assert!(!state.record_estimated_latency("other", sample.clone()));
        assert!(state.record_estimated_latency("sender", sample.clone()));
        let active = state.snapshot().active_stream.unwrap();
        assert!(active.estimated_latency_age_ms.is_some());
        assert_eq!(active.estimated_latency.unwrap().seq, 7);
        assert_eq!(active.latency_samples.len(), 1);
        assert_eq!(active.latency_samples[0].total_ms, 29.5);
        assert!(state.record_estimated_latency("sender", sample));
        assert_eq!(state.snapshot().active_stream.unwrap().latency_samples.len(), 1);
    }

    #[test]
    fn estimated_latency_rejects_unsubmitted_and_inconsistent_samples() {
        let state = ManagementState::new();
        state.hello(client("sender"));
        state.start(config(), Some(client("sender")));
        let unsubmitted = latency_sample(1);
        assert!(!state.record_estimated_latency("sender", unsubmitted));
        state.record_frame(1, 1_000, 2.0);
        let mut inconsistent = latency_sample(1);
        inconsistent.total_ms = 200.0;
        assert!(!state.record_estimated_latency("sender", inconsistent));
        let active = state.snapshot().active_stream.unwrap();
        assert!(active.estimated_latency.is_none());
        assert!(active.latency_samples.is_empty());
    }

    #[test]
    fn estimated_latency_is_retained_and_aged_after_three_seconds() {
        let state = ManagementState::new();
        state.hello(client("sender"));
        state.start(config(), Some(client("sender")));
        state.record_frame(1, 1_000, 2.0);
        assert!(state.record_estimated_latency("sender", latency_sample(1)));
        if let Ok(mut inner) = state.inner.lock() {
            if let Some((received_at, _)) = inner.stream.as_mut().and_then(|stream| stream.estimated_latency.as_mut()) {
                *received_at = Instant::now() - Duration::from_secs(4);
            }
        }
        let active = state.snapshot().active_stream.unwrap();
        assert!(active.estimated_latency.is_some());
        assert!(active.estimated_latency_age_ms.is_some_and(|age| age >= 4_000.0));
    }

    #[test]
    fn receiver_records_continuous_phases_without_browser_ack_processing() {
        let state = ManagementState::new();
        state.hello(client("sender"));
        state.start(config(), Some(client("sender")));
        assert!(state.record_clock_sync("sender", 100.0, 1.0));
        state.record_frame(1, 1_000, 2.0);
        assert!(state.record_receiver_estimated_latency(1, 1_000.0, 5.0, 1_010.0, 1_115.0, 3.0, 60));
        let active = state.snapshot().active_stream.unwrap();
        let sample = active.estimated_latency.unwrap();
        assert_eq!(sample.encode_ms, 5.0);
        assert_eq!(sample.sender_queue_ms, 5.0);
        assert_eq!(sample.delivery_ms, 5.0);
        assert_eq!(sample.receiver_queue_ms, 3.0);
        assert_eq!(active.latency_samples.len(), 1);
    }

    #[test]
    fn parses_typed_system_health_values() {
        assert_eq!(parse_load_average("0.12 1.34 2.56 1/100 42"), Some((0.12, 1.34, 2.56)));
        assert_eq!(parse_load_average("unavailable"), None);
        assert_eq!(
            parse_memory_info("MemTotal:       4096000 kB\n\nMemAvailable:   2048000 kB\n"),
            Some((2000, 4000)),
        );
        assert_eq!(parse_memory_info("MemTotal: 4096000 kB\n"), None);
    }

    #[test]
    fn pipeline_health_uses_explicit_reassembly_boundaries() {
        let state = ManagementState::new();
        state.refresh_pipeline_health("h265", crate::v4l2_decoder::ReassemblyStats {
            in_flight: 2,
            dropped_access_units: 3,
            ignored_packets: 4,
        });
        let health = state.snapshot().health;
        assert_eq!(health.playback_state, "h265");
        assert_eq!(health.reassembly_in_flight, 2);
        assert_eq!(health.dropped_access_units, 3);
        assert_eq!(health.ignored_media_packets, 4);
    }
}
