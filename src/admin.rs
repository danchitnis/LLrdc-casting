use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader as TokioBufReader};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

use crate::config;
use crate::local_pairing::PairingState;
use crate::management::ManagementState;
use tokio::sync::mpsc;
use crate::admin_protocol::AdminCommand;

const ADMIN_SOCKET_PATH: &str = "/run/llrdc-casting-admin.sock";
static ADMIN_HTML: &str = include_str!("../client/admin.html");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudSettingRequest {
    enabled: bool,
    confirm_restart: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsRequest {
    settings: EditableSettings,
    confirm_restart: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditableSettings {
    port: u16,
    webtransport_port: u16,
    http_port: u16,
    drm_connector_id: String,
    drm_plane_id: String,
    idle_dashboard: bool,
    idle_dashboard_mode: String,
    idle_timeout_sec: u64,
    sender_liveness_timeout_sec: u64,
    udp_buffer_size_mb: usize,
    pairing_code_ttl_sec: u64,
    local_pairing_code_required: bool,
    cloud_discovery_enabled: bool,
}

impl EditableSettings {
    fn apply_to(&self, mut current: config::ReceiverSettings) -> config::ReceiverSettings {
        current.port = self.port;
        current.webtransport_port = self.webtransport_port;
        current.http_port = self.http_port;
        current.drm_connector_id = self.drm_connector_id.clone();
        current.drm_plane_id = self.drm_plane_id.clone();
        current.idle_dashboard = self.idle_dashboard;
        current.idle_dashboard_mode = self.idle_dashboard_mode.clone();
        current.idle_timeout_sec = self.idle_timeout_sec;
        current.sender_liveness_timeout_sec = self.sender_liveness_timeout_sec;
        current.udp_buffer_size_mb = self.udp_buffer_size_mb;
        current.pairing_code_ttl_sec = self.pairing_code_ttl_sec;
        current.local_pairing_code_required = self.local_pairing_code_required;
        current.cloud_discovery_enabled = self.cloud_discovery_enabled;
        current
    }
}

fn settings_request_from_cloud(request: CloudSettingRequest, current: config::ReceiverSettings) -> SettingsRequest {
    SettingsRequest {
        settings: EditableSettings {
            port: current.port, webtransport_port: current.webtransport_port, http_port: current.http_port,
            drm_connector_id: current.drm_connector_id, drm_plane_id: current.drm_plane_id,
            idle_dashboard: current.idle_dashboard, idle_dashboard_mode: current.idle_dashboard_mode,
            idle_timeout_sec: current.idle_timeout_sec, sender_liveness_timeout_sec: current.sender_liveness_timeout_sec,
            udp_buffer_size_mb: current.udp_buffer_size_mb, pairing_code_ttl_sec: current.pairing_code_ttl_sec,
            local_pairing_code_required: current.local_pairing_code_required,
            cloud_discovery_enabled: request.enabled,
        },
        confirm_restart: request.confirm_restart,
    }
}

fn settings_response(current: &config::ReceiverSettings, request: &SettingsRequest) -> Result<(config::ReceiverSettings, bool), (String, Vec<u8>)> {
    let updated = request.settings.apply_to(current.clone());
    if let Err(error) = updated.validate() {
        return Err(("422 Unprocessable Entity".to_string(), serde_json::to_vec(&json!({"error":"invalid_settings", "detail": error})).unwrap_or_default()));
    }
    if updated.cloud_discovery_enabled && !current.cloud_discovery_enabled {
        let missing = crate::cloud_discovery::cloud_configuration_missing();
        if !missing.is_empty() {
            return Err(("422 Unprocessable Entity".to_string(), serde_json::to_vec(&json!({"error":"cloud_not_provisioned", "missing": missing})).unwrap_or_default()));
        }
    }
    Ok((updated.clone(), updated != *current))
}

pub async fn run_server(pairing_state: PairingState, management: ManagementState, commands: mpsc::Sender<AdminCommand>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket_state = pairing_state.clone();
    tokio::spawn(async move { if let Err(error) = run_unix_server(socket_state).await { eprintln!("[ADMIN SOCKET] Server stopped: {error}"); } });
    let Some(bind_addr) = std::env::var("ADMIN_BIND_ADDR").ok().filter(|v| !v.trim().is_empty()) else {
        eprintln!("[ADMIN HTTP] Disabled: ADMIN_BIND_ADDR is required; refusing wildcard bind");
        return Ok(());
    };
    let port = config::env_or("ADMIN_PORT", config::server::DEFAULT_ADMIN_PORT);
    run_http_server(bind_addr, port, commands, management, pairing_state).await
}

async fn run_unix_server(pairing_state: PairingState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = Path::new(ADMIN_SOCKET_PATH);
    match std::fs::remove_file(path) { Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => return Err(e.into()) }
    let listener = UnixListener::bind(path)?;
    #[cfg(unix)] std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    loop { let (stream, _) = listener.accept().await?; let state = pairing_state.clone(); tokio::spawn(async move { if let Err(e) = handle_unix_request(stream, state).await { eprintln!("[ADMIN SOCKET] Request failed: {e}"); } }); }
}

async fn handle_unix_request(stream: UnixStream, pairing_state: PairingState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut reader = TokioBufReader::new(stream); let mut request = String::new(); reader.read_line(&mut request).await?; let mut stream = reader.into_inner();
    if request.trim() != "pairing-code" { stream.write_all(b"ERROR invalid admin command\n").await?; return Ok(()); }
    match pairing_state.snapshot().code { Some(code) => { stream.write_all(code.as_bytes()).await?; stream.write_all(b"\n").await?; }, None => stream.write_all(b"ERROR pairing code unavailable\n").await? }
    Ok(())
}

pub async fn run_client(command: Option<&str>, has_extra_arguments: bool) -> Result<(), Box<dyn std::error::Error>> {
    if command != Some("pairing-code") || has_extra_arguments { return Err("usage: llrdc-casting admin pairing-code".into()); }
    let mut stream = UnixStream::connect(ADMIN_SOCKET_PATH).await?; stream.write_all(b"pairing-code\n").await?; stream.shutdown().await?;
    let mut response = Vec::new(); stream.read_to_end(&mut response).await?; let response = String::from_utf8(response)?; let response = response.trim();
    if let Some(error) = response.strip_prefix("ERROR ") { return Err(error.to_string().into()); }
    if response.len() != 4 || !response.bytes().all(|b| b.is_ascii_alphanumeric()) { return Err("admin socket returned an invalid pairing code".into()); }
    println!("{response}"); Ok(())
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, Box<dyn std::error::Error + Send + Sync>> { let mut reader = BufReader::new(File::open(path)?); Ok(rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?) }
fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>, Box<dyn std::error::Error + Send + Sync>> { let mut reader = BufReader::new(File::open(path)?); rustls_pemfile::private_key(&mut reader)?.ok_or_else(|| "No private key found in key.pem".into()) }

struct PrefixStream<S> { prefix: Option<Vec<u8>>, inner: S }
impl<S: AsyncRead + Unpin> AsyncRead for PrefixStream<S> {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> { if let Some(prefix) = self.prefix.as_mut() { if prefix.is_empty() { self.prefix = None; } else if prefix.len() <= buf.remaining() { buf.put_slice(prefix); self.prefix = None; return std::task::Poll::Ready(Ok(())); } else { let rest = prefix.split_off(buf.remaining()); buf.put_slice(prefix); self.prefix = Some(rest); return std::task::Poll::Ready(Ok(())); } } std::pin::Pin::new(&mut self.inner).poll_read(cx, buf) }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixStream<S> {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> std::task::Poll<std::io::Result<usize>> { std::pin::Pin::new(&mut self.inner).poll_write(cx, buf) }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> { std::pin::Pin::new(&mut self.inner).poll_flush(cx) }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> { std::pin::Pin::new(&mut self.inner).poll_shutdown(cx) }
}

async fn run_http_server(bind_addr: String, port: u16, commands: mpsc::Sender<AdminCommand>, management: ManagementState, pairing: PairingState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (cert_path, key_path) = crate::cert::get_cert_and_key_paths(); let certs = load_certs(&cert_path)?; let key = load_key(&key_path)?;
    let tls = ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?; let acceptor = TlsAcceptor::from(Arc::new(tls));
    let listener = TcpListener::bind((bind_addr.as_str(), port)).await?; println!("[ADMIN HTTP] Management portal listening on https://{bind_addr}:{port}/");
    loop { let (socket, peer) = listener.accept().await?; let acceptor = acceptor.clone(); let commands = commands.clone(); let management = management.clone(); let pairing = pairing.clone(); tokio::spawn(async move { match acceptor.accept(socket).await { Ok(stream) => { if let Err(e) = handle_http_connection(stream, peer.ip().to_string(), commands, management, pairing).await { eprintln!("[ADMIN HTTP] Request failed: {e}"); } }, Err(e) => eprintln!("[ADMIN HTTP] TLS handshake failed: {e}"), } }); }
}

async fn handle_http_connection<S>(mut stream: S, _peer: String, commands: mpsc::Sender<AdminCommand>, management: ManagementState, pairing: PairingState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> where S: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    let mut buf = vec![0u8; config::server::HTTP_REQUEST_BUFFER_BYTES];
    let n = stream.read(&mut buf).await?;
    if n == 0 { return Ok(()); }
    let initial = &buf[..n];
    let request_text = String::from_utf8_lossy(initial);
    if request_text.to_ascii_lowercase().contains("upgrade: websocket") {
        return handle_websocket(PrefixStream { prefix: Some(initial.to_vec()), inner: stream }, commands, management, pairing).await;
    }
    let mut request = initial.to_vec();
    let header_end = loop {
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") { break index; }
        if request.len() >= config::server::HTTP_REQUEST_BUFFER_BYTES { return write_http_response(&mut stream, "413 Payload Too Large", "text/plain; charset=utf-8", b"Payload Too Large").await; }
        let read = stream.read(&mut buf).await?;
        if read == 0 { return Ok(()); }
        request.extend_from_slice(&buf[..read]);
    };
    let header_text = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let mut lines = header_text.lines();
    let first = lines.next().unwrap_or("").to_string();
    let mut headers = std::collections::HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') { headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string()); }
    }
    let content_length = headers.get("content-length").and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
    if content_length > 4096 { return write_http_response(&mut stream, "413 Payload Too Large", "text/plain; charset=utf-8", b"Payload Too Large").await; }
    let body_start = header_end + 4;
    while request.len() < body_start + content_length {
        let read = stream.read(&mut buf).await?;
        if read == 0 { return write_http_response(&mut stream, "400 Bad Request", "text/plain; charset=utf-8", b"Incomplete request body").await; }
        request.extend_from_slice(&buf[..read]);
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let body = &request[body_start..body_start + content_length];
    let origin_ok = headers.get("origin").map_or(true, |origin| headers.get("host").map_or(false, |host| origin == &format!("https://{host}") || origin == &format!("http://{host}")));
    let mut restart = false;
    let (status, content_type, response_body) = if method == "GET" && (path == "/" || path == "/index.html") { ("200 OK", "text/html; charset=utf-8", ADMIN_HTML.as_bytes().to_vec()) }
        else if method == "GET" && path == "/api/snapshot" { ("200 OK", "application/json", snapshot_json(&management, &pairing)) }
        else if method == "GET" && path == "/health" { ("200 OK", "text/plain; charset=utf-8", b"OK".to_vec()) }
        else if method == "GET" && path == "/favicon.ico" { ("204 No Content", "image/x-icon", Vec::new()) }
        else if method == "POST" && path == "/api/stream/stop" { let _ = commands.send(AdminCommand::StopSharing).await; management.event("info", "admin_action", "administrative stop requested"); ("202 Accepted", "application/json", br#"{"ok":true}"#.to_vec()) }
        else if method == "PUT" && (path == "/api/settings" || path == "/api/settings/cloud") {
            if !origin_ok { ("403 Forbidden", "application/json", br#"{"error":"cross_origin"}"#.to_vec()) }
            else if !headers.get("content-type").is_some_and(|value| value.split(';').next().is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))) { ("415 Unsupported Media Type", "application/json", br#"{"error":"application_json_required"}"#.to_vec()) }
            else {
                let parsed = if path == "/api/settings/cloud" {
                    serde_json::from_slice::<CloudSettingRequest>(body).map(|request| settings_request_from_cloud(request, config::settings()))
                } else {
                    serde_json::from_slice::<SettingsRequest>(body)
                };
                match parsed {
                    Err(_) => ("400 Bad Request", "application/json", br#"{"error":"invalid_json"}"#.to_vec()),
                    Ok(request) => {
                        let current = config::settings();
                        match settings_response(&current, &request) {
                            Err((_status, body)) => ("422 Unprocessable Entity", "application/json", body),
                            Ok((_updated, changed)) if !changed => ("200 OK", "application/json", serde_json::to_vec(&json!({"ok": true, "cloud_discovery_enabled": current.cloud_discovery_enabled, "local_pairing_code_required": current.local_pairing_code_required, "restart_scheduled": false})).unwrap_or_default()),
                            Ok((_updated, _)) if !request.confirm_restart => ("409 Conflict", "application/json", br#"{"error":"restart_confirmation_required"}"#.to_vec()),
                            Ok((updated, _)) => {
                                if let Err(error) = config::persist_document(&updated) {
                                    eprintln!("[ADMIN] Could not persist receiver settings: {error}");
                                    ("500 Internal Server Error", "application/json", br#"{"error":"settings_persist_failed"}"#.to_vec())
                                } else {
                                    restart = true;
                                    management.event("info", "settings", "receiver settings updated; restart scheduled");
                                    ("202 Accepted", "application/json", serde_json::to_vec(&json!({"ok":true,"cloud_discovery_enabled":updated.cloud_discovery_enabled,"local_pairing_code_required":updated.local_pairing_code_required,"restart_scheduled":true})).unwrap_or_default())
                                }
                            }
                        }
                    }
                }
            }
        }
        else if method != "GET" && method != "POST" && method != "PUT" { ("405 Method Not Allowed", "text/plain; charset=utf-8", b"Method Not Allowed".to_vec()) }
        else { ("404 Not Found", "text/plain; charset=utf-8", b"Not Found".to_vec()) };
    write_http_response(&mut stream, status, content_type, &response_body).await?;
    if restart { let _ = commands.send(AdminCommand::RestartReceiver).await; }
    Ok(())
}

async fn write_http_response<S>(stream: &mut S, status: &str, content_type: &str, body: &[u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> where S: AsyncWrite + Unpin {
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

fn snapshot_json(management: &ManagementState, pairing: &PairingState) -> Vec<u8> { management.refresh_system_health(); serde_json::to_vec(&json!({ "management": management.snapshot(), "pairing": pairing.snapshot(), "settings": crate::cloud_discovery::settings_snapshot() })).unwrap_or_else(|_| b"{}".to_vec()) }

async fn handle_websocket<S>(stream: S, commands: mpsc::Sender<AdminCommand>, management: ManagementState, pairing: PairingState) -> Result<(), Box<dyn std::error::Error + Send + Sync>> where S: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    let ws = tokio_tungstenite::accept_async(stream).await?; let (mut tx, mut rx) = ws.split();
    tx.send(tokio_tungstenite::tungstenite::Message::Text(String::from_utf8(snapshot_json(&management, &pairing)).unwrap_or_default().into())).await?;
    let mut updates = management.subscribe(); let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop { tokio::select! { _ = tick.tick() => { let text = String::from_utf8(snapshot_json(&management, &pairing)).unwrap_or_default(); if tx.send(tokio_tungstenite::tungstenite::Message::Text(text.into())).await.is_err() { break; } }, _ = updates.recv() => { let text = String::from_utf8(snapshot_json(&management, &pairing)).unwrap_or_default(); if tx.send(tokio_tungstenite::tungstenite::Message::Text(text.into())).await.is_err() { break; } }, message = rx.next() => match message { Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) if text.contains("stop") => { let _ = commands.send(AdminCommand::StopSharing).await; management.event("info", "admin_action", "administrative stop requested"); }, Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break, _ => {} } } }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{settings_response, CloudSettingRequest, EditableSettings, SettingsRequest};
    use crate::config;

    fn editable(settings: &config::ReceiverSettings) -> EditableSettings {
        EditableSettings {
            port: settings.port, webtransport_port: settings.webtransport_port, http_port: settings.http_port,
            drm_connector_id: settings.drm_connector_id.clone(), drm_plane_id: settings.drm_plane_id.clone(),
            idle_dashboard: settings.idle_dashboard, idle_dashboard_mode: settings.idle_dashboard_mode.clone(),
            idle_timeout_sec: settings.idle_timeout_sec, sender_liveness_timeout_sec: settings.sender_liveness_timeout_sec,
            udp_buffer_size_mb: settings.udp_buffer_size_mb, pairing_code_ttl_sec: settings.pairing_code_ttl_sec,
            local_pairing_code_required: settings.local_pairing_code_required,
            cloud_discovery_enabled: settings.cloud_discovery_enabled,
        }
    }

    #[test]
    fn unchanged_settings_are_a_noop() {
        let current = config::ReceiverSettings::default();
        let request = SettingsRequest { settings: editable(&current), confirm_restart: false };
        let (_, changed) = settings_response(&current, &request).unwrap();
        assert!(!changed);
    }

    #[test]
    fn settings_schema_rejects_unknown_fields() {
        let result = serde_json::from_str::<CloudSettingRequest>(r#"{"enabled":true,"confirm_restart":true,"extra":1}"#);
        assert!(result.is_err());
    }
}
