//! Bounded, SD-card-friendly operational event journal.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
const SEGMENT_COUNT: usize = 4;
const RECENT_LIMIT: usize = 2_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationalEvent {
    pub timestamp_unix_ms: u64,
    pub severity: String,
    pub category: String,
    pub message: String,
    pub receiver_generation: u64,
}

#[derive(Clone)]
pub struct OperationalLog {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    directory: PathBuf,
    recent: VecDeque<OperationalEvent>,
    healthy: bool,
    pending_since_sync: bool,
    aggregates: HashMap<String, (u64, u32)>,
    pending: Vec<Vec<u8>>,
}

impl OperationalLog {
    pub fn open(directory: impl AsRef<Path>) -> std::io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        std::fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        std::fs::set_permissions(&directory, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
        let mut recent = VecDeque::new();
        for index in (0..SEGMENT_COUNT).rev() {
            let path = segment_path(&directory, index);
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            for line in text.lines() {
                if let Ok(event) = serde_json::from_str::<OperationalEvent>(line) {
                    recent.push_back(event);
                    while recent.len() > RECENT_LIMIT { recent.pop_front(); }
                }
            }
        }
        let last_start = recent.iter().rposition(|event| event.category == "stream_start");
        let last_end = recent.iter().rposition(|event| event.category == "stream_stop" || event.category == "stream_summary");
        let recovered_generation = last_start.filter(|start| last_end.map_or(true, |end| *start > end)).and_then(|start| recent.get(start)).map(|event| event.receiver_generation);
        let log = Self { inner: Arc::new(Mutex::new(Inner { directory, recent, healthy: true, pending_since_sync: false, aggregates: HashMap::new(), pending: Vec::new() })) };
        if let Some(generation) = recovered_generation { log.event("error", "stream_stop", "unclean_shutdown", generation, true); }
        Ok(log)
    }

    pub fn event(&self, severity: &str, category: &str, message: impl AsRef<str>, generation: u64, critical: bool) {
        let event = OperationalEvent {
            timestamp_unix_ms: now_ms(),
            severity: severity.to_string(),
            category: category.to_string(),
            message: redact(message.as_ref()),
            receiver_generation: generation,
        };
        let Ok(mut inner) = self.inner.lock() else { return };
        let mut write_events = Vec::new();
        let expired = inner.aggregates.iter().filter(|(_, (started, _))| event.timestamp_unix_ms.saturating_sub(*started) >= 60_000).map(|(key, value)| (key.clone(), *value)).collect::<Vec<_>>();
        for (key, (started, count)) in expired {
            inner.aggregates.remove(&key);
            if count > 1 { write_events.push(OperationalEvent { timestamp_unix_ms: event.timestamp_unix_ms, severity: "warn".into(), category: "failure_aggregate".into(), message: format!("{key} count={count} window_started={started}"), receiver_generation: generation }); }
        }
        let aggregate = matches!(category, "security_rejected" | "pairing_rejected" | "cloud_token_rejected") || (category.starts_with("cloud") && severity != "info");
        if aggregate {
            let key = format!("{} {}", event.category, event.message);
            if let Some((started, count)) = inner.aggregates.get_mut(&key) {
                if event.timestamp_unix_ms.saturating_sub(*started) < 60_000 {
                    *count = count.saturating_add(1);
                    for summary in write_events { append_event(&mut inner, &summary, false); }
                    return;
                }
            }
            inner.aggregates.insert(key, (event.timestamp_unix_ms, 1));
        }
        write_events.push(event);
        for event in write_events { append_event(&mut inner, &event, critical); }
    }

    pub fn sync(&self) {
        let Ok(mut inner) = self.inner.lock() else { return };
        if flush_pending(&mut inner).is_err() { inner.healthy = false; return; }
        if !inner.pending_since_sync { return; }
        match std::fs::OpenOptions::new().read(true).open(segment_path(&inner.directory, 0)).and_then(|file| file.sync_data()) {
            Ok(()) => { inner.pending_since_sync = false; inner.healthy = true; }
            Err(_) => inner.healthy = false,
        }
    }

    pub fn flush(&self) {
        let Ok(mut inner) = self.inner.lock() else { return };
        if flush_pending(&mut inner).is_err() { inner.healthy = false; }
    }

    pub fn recent(&self, lines: usize) -> Vec<OperationalEvent> {
        let Ok(inner) = self.inner.lock() else { return Vec::new() };
        inner.recent.iter().skip(inner.recent.len().saturating_sub(lines.min(RECENT_LIMIT))).cloned().collect()
    }

    pub fn healthy(&self) -> bool { self.inner.lock().map(|inner| inner.healthy).unwrap_or(false) }

    pub fn persist_incident_excerpt(&self, excerpt: &[u8]) {
        let Ok(mut inner) = self.inner.lock() else { return };
        let result = (|| -> std::io::Result<()> {
            for index in (0..3).rev() {
                let source = inner.directory.join(format!("incident.{index}.log"));
                if source.exists() { std::fs::rename(source, inner.directory.join(format!("incident.{}.log", index + 1)))?; }
            }
            let incident_path = inner.directory.join("incident.0.log");
            let mut file = std::fs::File::create(&incident_path)?;
            #[cfg(unix)] std::fs::set_permissions(&incident_path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
            let text = redact(&String::from_utf8_lossy(excerpt));
            file.write_all(text.as_bytes())?; file.sync_data()
        })();
        inner.healthy &= result.is_ok();
    }

    pub fn diagnostic_zip(&self, status: &serde_json::Value, redacted_config: &serde_json::Value, crash_excerpt: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        let events = self.recent(RECENT_LIMIT);
        let directory = self.inner.lock().map_err(|_| "operational log unavailable")?.directory.clone();
        let mut ndjson = Vec::new();
        let mut timeline = String::new();
        for event in &events {
            serde_json::to_writer(&mut ndjson, event)?;
            ndjson.push(b'\n');
            timeline.push_str(&format!("{} [{}] {}: {}\n", event.timestamp_unix_ms, event.severity, event.category, event.message));
        }
        let cursor = std::io::Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in [
            ("events.ndjson", ndjson),
            ("incident-timeline.txt", timeline.into_bytes()),
            ("status.json", serde_json::to_vec_pretty(status)?),
            ("device-config-redacted.json", serde_json::to_vec_pretty(redacted_config)?),
            ("receiver-incident-excerpt.txt", redact(crash_excerpt).into_bytes()),
        ] {
            archive.start_file(name, options)?;
            archive.write_all(&bytes)?;
        }
        for index in 0..4 {
            if let Ok(text) = std::fs::read_to_string(directory.join(format!("incident.{index}.log"))) {
                archive.start_file(format!("crash-excerpts/incident-{index}.txt"), options)?;
                archive.write_all(redact(&text).as_bytes())?;
            }
        }
        Ok(archive.finish()?.into_inner())
    }
}

fn append_event(inner: &mut Inner, event: &OperationalEvent, critical: bool) {
    let Ok(encoded) = serde_json::to_vec(event) else { return };
    let result = if critical {
        flush_pending(inner).and_then(|_| {
            rotate_if_needed(&inner.directory)?;
            let mut file = open_journal(&inner.directory)?;
            file.write_all(&encoded)?; file.write_all(b"\n")?; file.sync_data()
        })
    } else {
        let mut line = encoded; line.push(b'\n'); inner.pending.push(line); inner.pending_since_sync = true; Ok(())
    };
    inner.healthy = result.is_ok();
    inner.recent.push_back(event.clone()); while inner.recent.len() > RECENT_LIMIT { inner.recent.pop_front(); }
    println!("[{}] {}: {}", event.severity.to_ascii_uppercase(), event.category, event.message);
}

fn flush_pending(inner: &mut Inner) -> std::io::Result<()> {
    if inner.pending.is_empty() { return Ok(()) }
    rotate_if_needed(&inner.directory)?;
    let mut file = open_journal(&inner.directory)?;
    for line in inner.pending.drain(..) { file.write_all(&line)?; }
    file.flush()
}

fn open_journal(directory: &Path) -> std::io::Result<std::fs::File> {
    let path = segment_path(directory, 0);
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    #[cfg(unix)] std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(file)
}

fn segment_path(directory: &Path, index: usize) -> PathBuf { directory.join(format!("operations.{index}.ndjson")) }

fn rotate_if_needed(directory: &Path) -> std::io::Result<()> {
    let current = segment_path(directory, 0);
    if current.metadata().map(|metadata| metadata.len()).unwrap_or(0) < SEGMENT_BYTES { return Ok(()) }
    let oldest = segment_path(directory, SEGMENT_COUNT - 1);
    if oldest.exists() { std::fs::remove_file(oldest)?; }
    for index in (0..SEGMENT_COUNT - 1).rev() {
        let source = segment_path(directory, index);
        if source.exists() { std::fs::rename(source, segment_path(directory, index + 1))?; }
    }
    std::fs::File::create(current)?.sync_all()?;
    prune_segments(directory)?;
    std::fs::File::open(directory)?.sync_all()
}

fn prune_segments(directory: &Path) -> std::io::Result<()> {
    let cutoff = now_ms().saturating_sub(30 * 24 * 60 * 60 * 1_000);
    let (mut sessions, mut connections, mut general) = (0usize, 0usize, 0usize);
    for index in 0..SEGMENT_COUNT {
        let path = segment_path(directory, index);
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let parsed = text.lines().map(serde_json::from_str::<OperationalEvent>).collect::<Result<Vec<_>, _>>();
        let Ok(events) = parsed else { continue }; // Preserve corrupt segments intact for diagnosis.
        let mut retained = Vec::new();
        for event in events.into_iter().rev() {
            if event.timestamp_unix_ms < cutoff { continue; }
            let keep = if event.category == "stream_summary" { sessions += 1; sessions <= 10_000 }
                else if event.category.starts_with("connection") { connections += 1; connections <= 10_000 }
                else { general += 1; general <= 2_000 };
            if keep { retained.push(event); }
        }
        retained.reverse();
        let temporary = directory.join(format!("operations.{index}.prune"));
        let mut file = std::fs::File::create(&temporary)?;
        for event in retained { serde_json::to_writer(&mut file, &event).map_err(std::io::Error::other)?; file.write_all(b"\n")?; }
        file.sync_data()?; std::fs::rename(temporary, path)?;
    }
    Ok(())
}

fn now_ms() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }

pub fn redact(input: &str) -> String {
    let mut value = input.replace('\r', " ").replace('\n', " ");
    for name in ["PAIRING_CODE_FIXED", "PAIRING_REGISTRATION_SECRET", "PAIRING_TOKEN_PRIVATE_KEY", "CLOUDFLARE_API_TOKEN"] {
        if let Ok(secret) = std::env::var(name) {
            if !secret.is_empty() { value = value.replace(&secret, "[REDACTED]"); }
        }
    }
    // Authenticated URLs and common credential fields are never useful incident data.
    for marker in ["?code=", "&code=", "?token=", "&token=", "signature="] {
        let mut cursor = 0;
        while let Some(relative) = value[cursor..].to_ascii_lowercase().find(marker) {
            let start = cursor + relative;
            let value_start = start + marker.len();
            let end = value[value_start..].find(['&', ' ', '\"']).map(|offset| value_start + offset).unwrap_or(value.len());
            value.replace_range(value_start..end, "[REDACTED]");
            cursor = value_start + "[REDACTED]".len();
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn test_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "llrdc-log-{label}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
        ))
    }

    #[test]
    fn redacts_authenticated_urls() {
        let value = redact("connect https://host/x?code=ABCD&token=secret signature=sig");
        assert!(!value.contains("ABCD"));
        assert!(!value.contains("secret"));
        assert!(!value.contains("signature=sig"));
    }

    #[test]
    fn persists_and_recovers_events() {
        let directory = test_directory("recover");
        let log = OperationalLog::open(&directory).unwrap();
        log.event("info", "test", "safe", 3, true);
        let reopened = OperationalLog::open(&directory).unwrap();
        assert_eq!(reopened.recent(1)[0].receiver_generation, 3);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn noncritical_events_are_batched_until_flush() {
        let directory = test_directory("batch");
        let log = OperationalLog::open(&directory).unwrap();
        log.event("info", "connection_open", "safe", 1, false);
        assert!(!segment_path(&directory, 0).exists());
        log.flush();
        assert!(std::fs::read_to_string(segment_path(&directory, 0)).unwrap().contains("connection_open"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unmatched_stream_start_is_recovered_as_unclean_shutdown() {
        let directory = test_directory("unclean");
        let log = OperationalLog::open(&directory).unwrap();
        log.event("info", "stream_start", "codec=hevc", 7, true);
        drop(log);
        let reopened = OperationalLog::open(&directory).unwrap();
        let recovered = reopened.recent(1).pop().unwrap();
        assert_eq!(recovered.category, "stream_stop");
        assert_eq!(recovered.message, "unclean_shutdown");
        assert_eq!(recovered.receiver_generation, 7);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn corrupt_lines_are_preserved_but_do_not_block_recovery() {
        let directory = test_directory("corrupt");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(segment_path(&directory, 0), b"not-json\n").unwrap();
        let log = OperationalLog::open(&directory).unwrap();
        assert!(log.healthy());
        log.event("error", "receiver_crash", "signal=9", 2, true);
        let text = std::fs::read_to_string(segment_path(&directory, 0)).unwrap();
        assert!(text.starts_with("not-json\n"));
        assert!(text.contains("receiver_crash"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn diagnostic_zip_has_bounded_redacted_operational_contents() {
        let directory = test_directory("zip");
        let log = OperationalLog::open(&directory).unwrap();
        log.event("error", "security_rejected", "https://receiver.invalid/?code=ABCD&token=very-secret", 4, true);
        log.persist_incident_excerpt(b"failed https://receiver.invalid/?token=incident-secret");
        let bytes = log.diagnostic_zip(
            &serde_json::json!({"receiver_state": "backoff"}),
            &serde_json::json!({"registration_secret": "[REDACTED]"}),
            "https://receiver.invalid/?code=WXYZ&token=ring-secret",
        ).unwrap();
        assert!(bytes.len() < 2 * 1024 * 1024);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        for required in ["events.ndjson", "incident-timeline.txt", "status.json", "device-config-redacted.json", "receiver-incident-excerpt.txt"] {
            assert!(archive.by_name(required).is_ok(), "missing {required}");
        }
        let mut combined = String::new();
        for index in 0..archive.len() {
            archive.by_index(index).unwrap().read_to_string(&mut combined).unwrap();
        }
        for forbidden in ["ABCD", "WXYZ", "very-secret", "incident-secret", "ring-secret"] {
            assert!(!combined.contains(forbidden), "ZIP leaked {forbidden}");
        }
        assert!(combined.contains("[REDACTED]"));
        let _ = std::fs::remove_dir_all(directory);
    }
}
