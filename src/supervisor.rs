//! Receiver child-process supervision and manager-owned runtime state.

use serde::Serialize;
use std::collections::{HashSet, VecDeque};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc};

use crate::admin_protocol::{ReceiverRequest, ReceiverResponse, PROTOCOL_VERSION};
use crate::config::{self, ReceiverSettings};
use crate::local_pairing::PairingSnapshot;
use crate::management::Snapshot;
use crate::operational_log::OperationalLog;
use crate::receiver_ipc;

const STARTUP_DEADLINE: Duration = Duration::from_secs(30);
const HEARTBEAT_DEADLINE: Duration = Duration::from_secs(10);
const GRACEFUL_SHUTDOWN: Duration = Duration::from_secs(5);
const STABLE_RESET: Duration = Duration::from_secs(300);
const RING_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct WatchdogSnapshot {
    pub manager_uptime_sec: u64,
    pub receiver_state: String,
    pub receiver_generation: u64,
    pub receiver_pid: Option<u32>,
    pub receiver_uptime_sec: Option<u64>,
    pub restart_count: u64,
    pub consecutive_failures: u32,
    pub next_retry_sec: Option<u64>,
    pub last_failure: Option<String>,
    pub logging_healthy: bool,
    pub configuration_error: Option<String>,
}

#[derive(Clone)]
pub struct SupervisorHandle {
    inner: Arc<Mutex<Inner>>,
    commands: mpsc::Sender<SupervisorCommand>,
    updates: broadcast::Sender<()>,
    log: OperationalLog,
}

struct Inner {
    manager_started: Instant,
    receiver_started: Option<Instant>,
    receiver_state: String,
    receiver_generation: u64,
    receiver_pid: Option<u32>,
    restart_count: u64,
    consecutive_failures: u32,
    next_retry: Option<Instant>,
    last_failure: Option<String>,
    configuration_error: Option<String>,
    settings: ReceiverSettings,
    management: Option<Snapshot>,
    pairing: PairingSnapshot,
    ring: VecDeque<u8>,
    seen_events: usize,
    seen_sessions: HashSet<u64>,
}

enum SupervisorCommand {
    Restart { reason: String },
    StopSharing,
}

pub struct Supervisor {
    pub handle: SupervisorHandle,
    task: tokio::task::JoinHandle<()>,
}

impl Supervisor {
    pub async fn wait(self) { let _ = self.task.await; }
}

impl SupervisorHandle {
    pub fn subscribe(&self) -> broadcast::Receiver<()> { self.updates.subscribe() }

    pub fn watchdog(&self) -> WatchdogSnapshot {
        let inner = self.inner.lock().expect("supervisor lock poisoned");
        WatchdogSnapshot {
            manager_uptime_sec: inner.manager_started.elapsed().as_secs(),
            receiver_state: inner.receiver_state.clone(),
            receiver_generation: inner.receiver_generation,
            receiver_pid: inner.receiver_pid,
            receiver_uptime_sec: inner.receiver_started.map(|started| started.elapsed().as_secs()),
            restart_count: inner.restart_count,
            consecutive_failures: inner.consecutive_failures,
            next_retry_sec: inner.next_retry.map(|deadline| deadline.saturating_duration_since(Instant::now()).as_secs()),
            last_failure: inner.last_failure.clone(),
            logging_healthy: self.log.healthy(),
            configuration_error: inner.configuration_error.clone(),
        }
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let inner = self.inner.lock().expect("supervisor lock poisoned");
        let missing = crate::cloud_discovery::cloud_configuration_missing();
        let mut settings = serde_json::to_value(&inner.settings).unwrap_or_else(|_| serde_json::json!({}));
        if let Some(object) = settings.as_object_mut() {
            object.insert("cloud_configuration_ready".into(), missing.is_empty().into());
            object.insert("cloud_configuration_missing".into(), missing.clone().into());
            object.insert("cloud_state".into(), inner.pairing.cloud_status.clone().into());
            object.insert("pairing_code_source".into(), if std::env::var("PAIRING_CODE_FIXED").ok().is_some_and(|v| !v.is_empty()) { "fixed".into() } else { "rotating".into() });
        }
        serde_json::json!({
            "management": inner.management,
            "pairing": inner.pairing,
            "settings": settings,
            "watchdog": WatchdogSnapshot {
                manager_uptime_sec: inner.manager_started.elapsed().as_secs(),
                receiver_state: inner.receiver_state.clone(), receiver_generation: inner.receiver_generation,
                receiver_pid: inner.receiver_pid, receiver_uptime_sec: inner.receiver_started.map(|started| started.elapsed().as_secs()),
                restart_count: inner.restart_count, consecutive_failures: inner.consecutive_failures,
                next_retry_sec: inner.next_retry.map(|deadline| deadline.saturating_duration_since(Instant::now()).as_secs()),
                last_failure: inner.last_failure.clone(), logging_healthy: self.log.healthy(),
                configuration_error: inner.configuration_error.clone(),
            },
        })
    }

    pub fn settings(&self) -> ReceiverSettings { self.inner.lock().expect("supervisor lock poisoned").settings.clone() }

    pub fn is_ready(&self) -> bool { self.inner.lock().map(|inner| inner.receiver_state == "ready").unwrap_or(false) }

    pub async fn restart(&self, reason: impl Into<String>) -> Result<u64, String> {
        let target = self.inner.lock().map_err(|_| "supervisor unavailable")?.receiver_generation + 1;
        self.commands.send(SupervisorCommand::Restart { reason: reason.into() }).await.map_err(|_| "supervisor unavailable")?;
        Ok(target)
    }

    pub async fn stop_sharing(&self) -> Result<(), String> {
        if !self.is_ready() { return Err("receiver unavailable".into()) }
        self.commands.send(SupervisorCommand::StopSharing).await.map_err(|_| "supervisor unavailable".into())
    }

    pub fn apply_settings(&self, updated: ReceiverSettings) -> Result<bool, String> {
        updated.validate()?;
        let mut inner = self.inner.lock().map_err(|_| "supervisor unavailable")?;
        if updated == inner.settings && inner.configuration_error.is_none() { return Ok(false) }
        config::persist_document(&updated).map_err(|error| error.to_string())?;
        inner.settings = updated;
        inner.configuration_error = None;
        drop(inner);
        self.log.event("info", "configuration_persisted", "receiver configuration atomically replaced", self.watchdog().receiver_generation, true);
        let _ = self.updates.send(());
        Ok(true)
    }

    pub fn recent_logs(&self, lines: usize) -> Vec<crate::operational_log::OperationalEvent> { self.log.recent(lines) }

    pub fn record_event(&self, severity: &str, category: &str, message: impl AsRef<str>, critical: bool) {
        self.log.event(severity, category, message, self.watchdog().receiver_generation, critical);
    }

    pub fn diagnostic_zip(&self) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let snapshot = self.snapshot();
        let (settings, excerpt) = {
            let inner = self.inner.lock().map_err(|_| "supervisor unavailable")?;
            (inner.settings.clone(), String::from_utf8_lossy(&inner.ring.iter().copied().collect::<Vec<_>>()).into_owned())
        };
        let redacted = serde_json::to_value(settings)?;
        self.log.diagnostic_zip(&snapshot, &redacted, &excerpt)
    }

    pub async fn pairing_code(&self) -> Result<String, String> {
        match receiver_ipc::request(&ReceiverRequest::PairingCode { version: PROTOCOL_VERSION }).await {
            Ok(ReceiverResponse::PairingCode { code, .. }) => Ok(code),
            Ok(ReceiverResponse::Error { code, .. }) => Err(code),
            Ok(_) => Err("invalid receiver response".into()),
            Err(error) => Err(error.to_string()),
        }
    }
}

pub fn start() -> Result<Supervisor, Box<dyn std::error::Error + Send + Sync>> {
    let log_directory = std::env::var("LLRDC_LOG_DIR").unwrap_or_else(|_| "/management".into());
    let log = OperationalLog::open(log_directory)?;
    let config_result = config::load_settings_at(Path::new(config::DEVICE_CONFIG_PATH));
    let (settings, configuration_error) = match config_result {
        Ok(settings) => (settings, None),
        Err(error) => (ReceiverSettings::from_environment(), Some(error.to_string())),
    };
    let (commands, receiver) = mpsc::channel(16);
    let (updates, _) = broadcast::channel(32);
    let inner = Arc::new(Mutex::new(Inner {
        manager_started: Instant::now(), receiver_started: None,
        receiver_state: if configuration_error.is_some() { "configuration_error".into() } else { "starting".into() },
        receiver_generation: 0, receiver_pid: None, restart_count: 0, consecutive_failures: 0,
        next_retry: None, last_failure: None, configuration_error, settings,
        management: Some(crate::management::ManagementState::new().snapshot()),
        pairing: PairingSnapshot { code: None, local_status: "UNAVAILABLE".into(), cloud_status: "UNAVAILABLE".into(), cloud_ip: None },
        ring: VecDeque::new(), seen_events: 0, seen_sessions: HashSet::new(),
    }));
    let handle = SupervisorHandle { inner, commands, updates, log };
    let task_handle = handle.clone();
    let task = tokio::spawn(async move { run_supervisor(task_handle, receiver).await });
    Ok(Supervisor { handle, task })
}

async fn run_supervisor(handle: SupervisorHandle, mut commands: mpsc::Receiver<SupervisorCommand>) {
    let manager_checksum = std::env::current_exe().ok().and_then(|path| binary_checksum(&path)).unwrap_or_else(|| "unavailable".into());
    let receiver_path = std::env::var("LLRDC_RECEIVER_BIN").unwrap_or_else(|_| "/usr/local/bin/llrdc-casting".into());
    let receiver_checksum = binary_checksum(Path::new(&receiver_path)).unwrap_or_else(|| "unavailable".into());
    handle.log.event("info", "manager_start", format!("management started build={} manager_sha256={} receiver_sha256={}", env!("CARGO_PKG_VERSION"), manager_checksum, receiver_checksum), 0, true);
    let sync_log = handle.log.clone();
    tokio::spawn(async move { let mut tick = tokio::time::interval(Duration::from_secs(10)); let mut intervals = 0u8; loop { tick.tick().await; intervals = intervals.wrapping_add(1); if intervals % 6 == 0 { sync_log.sync(); } else { sync_log.flush(); } } });
    let mut child: Option<Child> = None;
    let mut startup_at = Instant::now();
    let mut last_heartbeat = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let configuration_error = handle.inner.lock().ok().and_then(|inner| inner.configuration_error.clone());
                if child.is_none() && configuration_error.is_none() {
                    let retry_ready = handle.inner.lock().ok().and_then(|inner| inner.next_retry).map_or(true, |deadline| deadline <= Instant::now());
                    if retry_ready {
                        match spawn_receiver(&handle).await {
                            Ok(new_child) => { child = Some(new_child); startup_at = Instant::now(); last_heartbeat = Instant::now(); }
                            Err(error) => schedule_failure(&handle, format!("startup failure: {error}")),
                        }
                    }
                }
                if let Some(running) = child.as_mut() {
                    match running.try_wait() {
                        Ok(Some(status)) => {
                            child = None;
                            schedule_failure(&handle, classify_exit(&handle, status));
                            continue;
                        }
                        Err(error) => { child = None; schedule_failure(&handle, format!("wait failure: {error}")); continue; }
                        Ok(None) => {}
                    }
                    match receiver_ipc::request(&ReceiverRequest::Snapshot { version: PROTOCOL_VERSION }).await {
                        Ok(ReceiverResponse::Snapshot { ready, management, pairing, .. }) => {
                            last_heartbeat = Instant::now();
                            let mut became_ready = false;
                            let mut new_events = Vec::new();
                            let mut new_sessions = Vec::new();
                            let mut cloud_transition = None;
                            if let Ok(mut inner) = handle.inner.lock() {
                                if management.events.len() < inner.seen_events { inner.seen_events = 0; }
                                new_events.extend(management.events.iter().skip(inner.seen_events).cloned());
                                new_sessions.extend(management.history.iter().filter(|session| !inner.seen_sessions.contains(&session.id)).cloned());
                                inner.seen_events = management.events.len(); inner.seen_sessions.extend(management.history.iter().map(|session| session.id));
                                if inner.pairing.cloud_status != pairing.cloud_status || inner.pairing.cloud_ip != pairing.cloud_ip {
                                    cloud_transition = Some(format!("state={} lan_address={}", pairing.cloud_status, pairing.cloud_ip.as_deref().unwrap_or("unavailable")));
                                }
                                inner.management = Some(management); inner.pairing = pairing;
                                if ready && inner.receiver_state != "ready" { inner.receiver_state = "ready".into(); became_ready = true; }
                                if inner.receiver_started.is_some_and(|started| started.elapsed() >= STABLE_RESET) { inner.consecutive_failures = 0; }
                            }
                            let generation = handle.watchdog().receiver_generation;
                            for event in new_events { handle.log.event(&event.level, &event.kind, event.message, generation, event.level == "error"); }
                            if let Some(message) = cloud_transition { handle.log.event("info", "cloud_registration", message, generation, false); }
                            for session in new_sessions {
                                handle.log.event("info", "stream_summary", format!("session={} reason={} duration={:.1}s frames={} bytes={} average_mbps={:.2} peak_mbps={:.2} sequence_gaps={} ingest_latency_p50_ms={:.1} ingest_latency_p95_ms={:.1} ingest_latency_max_ms={:.1}", session.id, session.end_reason.unwrap_or_else(|| "unknown".into()), session.duration_sec, session.frames, session.bytes, session.average_bitrate_mbps, session.peak_bitrate_mbps, session.sequence_gaps, session.latency_p50_ms, session.latency_p95_ms, session.latency_max_ms), generation, false);
                            }
                            if became_ready { handle.log.event("info", "receiver_ready", format!("receiver ready after {} ms", startup_at.elapsed().as_millis()), handle.watchdog().receiver_generation, false); }
                            let _ = handle.updates.send(());
                        }
                        _ if startup_at.elapsed() > STARTUP_DEADLINE => {
                            handle.log.event("error", "receiver_startup_timeout", "receiver did not become ready within 30 seconds", handle.watchdog().receiver_generation, true);
                            terminate_child(&handle, running, "startup_timeout").await;
                        }
                        _ if last_heartbeat.elapsed() > HEARTBEAT_DEADLINE => {
                            handle.log.event("error", "receiver_unresponsive", "receiver heartbeat missing for 10 seconds", handle.watchdog().receiver_generation, true);
                            terminate_child(&handle, running, "heartbeat_loss").await;
                        }
                        _ => {}
                    }
                }
            }
            Some(command) = commands.recv() => match command {
                SupervisorCommand::StopSharing => {
                    let _ = receiver_ipc::request(&ReceiverRequest::StopSharing { version: PROTOCOL_VERSION }).await;
                    handle.log.event("info", "administrative_stop", "active sharing stop requested", handle.watchdog().receiver_generation, false);
                }
                SupervisorCommand::Restart { reason } => {
                    if let Some(mut running) = child.take() {
                        terminate_child(&handle, &mut running, &reason).await;
                        let _ = running.wait().await;
                    }
                    schedule_requested_restart(&handle, &reason);
                }
            },
            else => break,
        }
    }
}

async fn spawn_receiver(handle: &SupervisorHandle) -> Result<Child, Box<dyn std::error::Error + Send + Sync>> {
    let binary = std::env::var("LLRDC_RECEIVER_BIN").unwrap_or_else(|_| "/usr/local/bin/llrdc-casting".into());
    let mut command = Command::new(&binary);
    command.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    command.as_std_mut().process_group(0);
    let mut child = command.spawn()?;
    let pid = child.id();
    if let Some(stdout) = child.stdout.take() { spawn_output_reader(handle.clone(), stdout, false); }
    if let Some(stderr) = child.stderr.take() { spawn_output_reader(handle.clone(), stderr, true); }
    let generation = {
        let mut inner = handle.inner.lock().map_err(|_| "supervisor unavailable")?;
        inner.receiver_generation += 1; inner.receiver_pid = pid; inner.receiver_started = Some(Instant::now());
        inner.seen_events = 0; inner.seen_sessions.clear();
        inner.receiver_state = "starting".into(); inner.next_retry = None; inner.receiver_generation
    };
    handle.log.event("info", "receiver_start", format!("receiver generation={generation} pid={}", pid.unwrap_or(0)), generation, false);
    let _ = handle.updates.send(());
    Ok(child)
}

fn spawn_output_reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(handle: SupervisorHandle, stream: R, stderr: bool) {
    tokio::spawn(async move {
        let verbose = std::env::var("LLRDC_CODEC_DIAGNOSTICS").ok().is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(mut inner) = handle.inner.lock() {
                for byte in line.bytes().chain(std::iter::once(b'\n')) {
                    inner.ring.push_back(byte); while inner.ring.len() > RING_BYTES { inner.ring.pop_front(); }
                }
            }
            if verbose { if stderr { eprintln!("[RECEIVER] {line}") } else { println!("[RECEIVER] {line}") } }
        }
    });
}

async fn terminate_child(handle: &SupervisorHandle, child: &mut Child, reason: &str) {
    let _ = receiver_ipc::request(&ReceiverRequest::Shutdown { version: PROTOCOL_VERSION, reason: reason.into() }).await;
    let deadline = Instant::now() + GRACEFUL_SHUTDOWN;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() { return; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if let Some(pid) = child.id() {
        let group = nix::unistd::Pid::from_raw(pid as i32);
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGTERM);
        tokio::time::sleep(Duration::from_millis(500)).await;
        if child.try_wait().ok().flatten().is_none() {
            let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
            handle.log.event("error", "receiver_forced_kill", format!("receiver process group force-killed reason={reason}"), handle.watchdog().receiver_generation, true);
        }
    }
}

fn schedule_requested_restart(handle: &SupervisorHandle, reason: &str) {
    if let Ok(mut inner) = handle.inner.lock() {
        inner.receiver_pid = None; inner.receiver_started = None; inner.receiver_state = "restarting".into();
        inner.restart_count += 1; inner.next_retry = Some(Instant::now());
    }
    handle.log.event("info", "receiver_restart_requested", format!("receiver restart reason={reason}"), handle.watchdog().receiver_generation, false);
    let _ = handle.updates.send(());
}

fn schedule_failure(handle: &SupervisorHandle, reason: String) {
    if let Ok(inner) = handle.inner.lock() {
        handle.log.persist_incident_excerpt(&inner.ring.iter().copied().collect::<Vec<_>>());
    }
    let (generation, delay) = {
        let mut inner = match handle.inner.lock() { Ok(inner) => inner, Err(_) => return };
        inner.receiver_pid = None; inner.receiver_started = None; inner.receiver_state = "backoff".into();
        inner.consecutive_failures = inner.consecutive_failures.saturating_add(1); inner.restart_count += 1;
        let delay = backoff(inner.consecutive_failures); inner.next_retry = Some(Instant::now() + delay); inner.last_failure = Some(reason.clone());
        (inner.receiver_generation, delay)
    };
    handle.log.event("error", "receiver_failure", format!("{reason}; retry in {}s; {}", delay.as_secs(), system_context()), generation, true);
    let _ = handle.updates.send(());
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(match attempt { 0 | 1 => 1, 2 => 2, 3 => 4, 4 => 8, 5 => 16, _ => 30 })
}

fn format_exit(status: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    if let Some(signal) = status.signal() { format!("receiver exited by signal {signal}") }
    else { format!("receiver exited with code {}", status.code().unwrap_or(-1)) }
}

fn classify_exit(handle: &SupervisorHandle, status: std::process::ExitStatus) -> String {
    let excerpt = handle.inner.lock().ok().map(|inner| String::from_utf8_lossy(&inner.ring.iter().copied().collect::<Vec<_>>()).into_owned()).unwrap_or_default();
    if excerpt.contains("panicked at") { format!("receiver panic; {}", format_exit(status)) }
    else if excerpt.contains("GStreamer") && excerpt.to_ascii_lowercase().contains("error") { format!("receiver GStreamer failure; {}", format_exit(status)) }
    else { format_exit(status) }
}

fn binary_checksum(path: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path).ok()?;
    Some(format!("{:x}", Sha256::digest(bytes)))
}

fn system_context() -> String {
    let load = std::fs::read_to_string("/proc/loadavg").ok().map(|text| text.split_whitespace().take(3).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "unavailable".into());
    let memory = std::fs::read_to_string("/proc/meminfo").ok().map(|text| text.lines().filter(|line| line.starts_with("MemAvailable:")).collect::<Vec<_>>().join(" ")).unwrap_or_else(|| "unavailable".into());
    let temperature = std::fs::read_to_string("/sys/class/thermal/thermal_zone0/temp").ok().map(|value| value.trim().to_string()).unwrap_or_else(|| "unavailable".into());
    let disk_mib = nix::sys::statvfs::statvfs("/management").ok().map(|stats| stats.blocks_available().saturating_mul(stats.fragment_size()) / 1024 / 1024).unwrap_or(0);
    format!("load={load} {memory} temperature_millidegrees={temperature} management_disk_available_mib={disk_mib}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_is_exponential_then_capped_forever() {
        assert_eq!((1..=7).map(|n| backoff(n).as_secs()).collect::<Vec<_>>(), vec![1, 2, 4, 8, 16, 30, 30]);
        assert_eq!(backoff(u32::MAX), Duration::from_secs(30));
    }
}
