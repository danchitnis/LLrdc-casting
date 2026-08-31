use std::collections::HashMap;
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

use crate::admin_protocol::MANAGEMENT_SOCKET_PATH;
use crate::config::ReceiverSettings;
use crate::supervisor::SupervisorHandle;

const MAX_REQUEST_BYTES: usize = 16 * 1024;
static ADMIN_HTML: &str = include_str!("../client/admin.html");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CloudSettingRequest { enabled: bool, confirm_restart: bool }

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartRequest { confirm_restart: bool }

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequest { confirm_update: bool }

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsRequest { settings: EditableSettings, confirm_restart: bool }

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
    fn apply_to(self, mut settings: ReceiverSettings) -> ReceiverSettings {
        settings.port = self.port; settings.webtransport_port = self.webtransport_port; settings.http_port = self.http_port;
        settings.drm_connector_id = self.drm_connector_id; settings.drm_plane_id = self.drm_plane_id;
        settings.idle_dashboard = self.idle_dashboard; settings.idle_dashboard_mode = self.idle_dashboard_mode;
        settings.idle_timeout_sec = self.idle_timeout_sec; settings.sender_liveness_timeout_sec = self.sender_liveness_timeout_sec;
        settings.udp_buffer_size_mb = self.udp_buffer_size_mb; settings.pairing_code_ttl_sec = self.pairing_code_ttl_sec;
        settings.local_pairing_code_required = self.local_pairing_code_required;
        settings.cloud_discovery_enabled = self.cloud_discovery_enabled;
        settings
    }
}

pub async fn run_server(supervisor: SupervisorHandle) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let socket_supervisor = supervisor.clone();
    tokio::spawn(async move { if let Err(error) = run_unix_server(socket_supervisor).await { eprintln!("[MANAGER SOCKET] {error}"); } });
    let settings = supervisor.settings();
    if settings.admin_bind_address.trim().is_empty() {
        return Err("management bind address is required; refusing wildcard bind".into());
    }
    let listener = TcpListener::bind((settings.admin_bind_address.as_str(), settings.admin_port)).await?;
    println!("[MANAGER] Portal ready on https://{}:{}/", settings.admin_bind_address, settings.admin_port);
    loop {
        let (socket, _) = listener.accept().await?;
        let cert_dir = Path::new(&settings.cert_dir);
        let certs = load_certs(&cert_dir.join("cert.pem"))?;
        let key = load_key(&cert_dir.join("key.pem"))?;
        let tls = ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?;
        let acceptor = TlsAcceptor::from(Arc::new(tls)); let supervisor = supervisor.clone();
        tokio::spawn(async move {
            match acceptor.accept(socket).await {
                Ok(stream) => if let Err(error) = handle_http(stream, supervisor).await { eprintln!("[MANAGER HTTP] {error}"); },
                Err(error) => eprintln!("[MANAGER TLS] handshake failed: {error}"),
            }
        });
    }
}

async fn run_unix_server(supervisor: SupervisorHandle) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = Path::new(MANAGEMENT_SOCKET_PATH);
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    match std::fs::remove_file(path) { Ok(()) => {}, Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}, Err(error) => return Err(error.into()) }
    let listener = UnixListener::bind(path)?;
    #[cfg(unix)] std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    loop {
        let (mut stream, _) = listener.accept().await?;
        let supervisor = supervisor.clone();
        tokio::spawn(async move {
            let mut request = String::new();
            let mut reader = TokioBufReader::new(&mut stream);
            if reader.read_line(&mut request).await.is_err() { return; }
            let response = if request.trim() == "pairing-code" { supervisor.pairing_code().await } else { Err("invalid admin command".into()) };
            let text = response.unwrap_or_else(|error| format!("ERROR {error}"));
            let _ = stream.write_all(format!("{text}\n").as_bytes()).await;
        });
    }
}

pub async fn run_client(command: Option<&str>, has_extra_arguments: bool) -> Result<(), Box<dyn std::error::Error>> {
    if command != Some("pairing-code") || has_extra_arguments { return Err("usage: llrdc-management admin pairing-code".into()) }
    let mut stream = UnixStream::connect(MANAGEMENT_SOCKET_PATH).await?;
    stream.write_all(b"pairing-code\n").await?; stream.shutdown().await?;
    let mut response = Vec::new(); stream.read_to_end(&mut response).await?;
    let response = String::from_utf8(response)?; let response = response.trim();
    if let Some(error) = response.strip_prefix("ERROR ") { return Err(error.to_string().into()) }
    if response.len() != 4 || !response.bytes().all(|byte| byte.is_ascii_alphanumeric()) { return Err("manager returned invalid pairing code".into()) }
    println!("{response}"); Ok(())
}

struct PrefixStream<S> { prefix: Option<Vec<u8>>, inner: S }
impl<S: AsyncRead + Unpin> AsyncRead for PrefixStream<S> {
    fn poll_read(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, buffer: &mut tokio::io::ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
        if let Some(prefix) = self.prefix.take() {
            let count = prefix.len().min(buffer.remaining()); buffer.put_slice(&prefix[..count]);
            if count < prefix.len() { self.prefix = Some(prefix[count..].to_vec()); }
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixStream<S> {
    fn poll_write(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>, bytes: &[u8]) -> std::task::Poll<std::io::Result<usize>> { std::pin::Pin::new(&mut self.inner).poll_write(cx, bytes) }
    fn poll_flush(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> { std::pin::Pin::new(&mut self.inner).poll_flush(cx) }
    fn poll_shutdown(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<()>> { std::pin::Pin::new(&mut self.inner).poll_shutdown(cx) }
}

async fn handle_http<S>(mut stream: S, supervisor: SupervisorHandle) -> Result<(), Box<dyn std::error::Error + Send + Sync>> where S: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    let mut buffer = vec![0; 8192];
    let count = stream.read(&mut buffer).await?;
    if count == 0 { return Ok(()) }
    if String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase().contains("upgrade: websocket") {
        return handle_websocket(PrefixStream { prefix: Some(buffer[..count].to_vec()), inner: stream }, supervisor).await;
    }
    let mut request = buffer[..count].to_vec();
    let header_end = loop {
        if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") { break index }
        if request.len() >= MAX_REQUEST_BYTES { return response(&mut stream, "413 Payload Too Large", "text/plain", b"Payload Too Large", None).await }
        let count = stream.read(&mut buffer).await?; if count == 0 { return Ok(()) } request.extend_from_slice(&buffer[..count]);
    };
    let header = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let mut lines = header.lines(); let first = lines.next().unwrap_or("");
    let mut headers = HashMap::new();
    for line in lines { if let Some((name, value)) = line.split_once(':') { headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string()); } }
    let length = headers.get("content-length").and_then(|value| value.parse().ok()).unwrap_or(0usize);
    if length > MAX_REQUEST_BYTES { return response(&mut stream, "413 Payload Too Large", "application/json", br#"{"error":"payload_too_large"}"#, None).await }
    let body_start = header_end + 4;
    while request.len() < body_start + length { let count = stream.read(&mut buffer).await?; if count == 0 { break } request.extend_from_slice(&buffer[..count]); }
    if request.len() < body_start + length { return response(&mut stream, "400 Bad Request", "application/json", br#"{"error":"incomplete_body"}"#, None).await }
    let mut first = first.split_whitespace(); let method = first.next().unwrap_or(""); let path = first.next().unwrap_or("/");
    let body = &request[body_start..body_start + length];
    let same_origin = request_is_same_origin(&headers);
    let json_content = headers.get("content-type").is_some_and(|value| value.split(';').next().is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json")));

    if method == "GET" && (path == "/" || path == "/index.html") { return response(&mut stream, "200 OK", "text/html; charset=utf-8", ADMIN_HTML.as_bytes(), None).await }
    if method == "GET" && path == "/api/snapshot" { return response_json(&mut stream, "200 OK", &supervisor.snapshot()).await }
    if method == "GET" && path == "/api/update" { return response_json(&mut stream, "200 OK", &serde_json::to_value(crate::update::status()).unwrap_or_else(|_| json!({"state":"failed","installed":false}))).await }
    if method == "GET" && path == "/health/manager" { return response(&mut stream, "200 OK", "text/plain", b"OK", None).await }
    if method == "GET" && path == "/health" { return response(&mut stream, if supervisor.is_ready() { "200 OK" } else { "503 Service Unavailable" }, "text/plain", if supervisor.is_ready() { b"OK" } else { b"RECEIVER UNAVAILABLE" }, None).await }
    if method == "GET" && path.starts_with("/api/logs?") {
        let lines = path.split("lines=").nth(1).and_then(|value| value.split('&').next()).and_then(|value| value.parse().ok()).unwrap_or(200).clamp(1, 2000);
        return response_json(&mut stream, "200 OK", &json!({"events": supervisor.recent_logs(lines)})).await;
    }
    if method == "GET" && path == "/api/logs/download" {
        if !same_origin { return response_json(&mut stream, "403 Forbidden", &json!({"error":"cross_origin"})).await }
        match supervisor.diagnostic_zip() {
            Ok(zip) => return response(&mut stream, "200 OK", "application/zip", &zip, Some("Content-Disposition: attachment; filename=llrdc-diagnostics.zip\r\n")).await,
            Err(_) => return response_json(&mut stream, "500 Internal Server Error", &json!({"error":"diagnostic_export_failed"})).await,
        }
    }
    if matches!(method, "POST" | "PUT") {
        if !same_origin { return response_json(&mut stream, "403 Forbidden", &json!({"error":"cross_origin"})).await }
        if !json_content { return response_json(&mut stream, "415 Unsupported Media Type", &json!({"error":"application_json_required"})).await }
    }
    if method == "POST" && path == "/api/stream/stop" {
        return match supervisor.stop_sharing().await { Ok(()) => response_json(&mut stream, "202 Accepted", &json!({"ok":true})).await, Err(_) => response_json(&mut stream, "503 Service Unavailable", &json!({"error":"receiver_unavailable"})).await };
    }
    if method == "POST" && path == "/api/watchdog/restart" {
        let Ok(request) = serde_json::from_slice::<RestartRequest>(body) else { return response_json(&mut stream, "400 Bad Request", &json!({"error":"invalid_json"})).await };
        if !request.confirm_restart { return response_json(&mut stream, "409 Conflict", &json!({"error":"restart_confirmation_required"})).await }
        let target = supervisor.restart("manual_restart").await.map_err(std::io::Error::other)?;
        return response_json(&mut stream, "202 Accepted", &json!({"ok":true,"target_generation":target})).await;
    }
    if method == "POST" && path == "/api/update/check" {
        return match crate::update::request("check") {
            Ok(()) => response_json(&mut stream, "202 Accepted", &json!({"ok":true})).await,
            Err(_) => response_json(&mut stream, "503 Service Unavailable", &json!({"error":"updater_unavailable"})).await,
        };
    }
    if method == "POST" && path == "/api/update/apply" {
        let Ok(request) = serde_json::from_slice::<UpdateRequest>(body) else { return response_json(&mut stream, "400 Bad Request", &json!({"error":"invalid_json"})).await };
        if !request.confirm_update { return response_json(&mut stream, "409 Conflict", &json!({"error":"update_confirmation_required"})).await }
        if supervisor.is_streaming() { return response_json(&mut stream, "409 Conflict", &json!({"error":"stream_active"})).await }
        return match crate::update::request("apply") {
            Ok(()) => response_json(&mut stream, "202 Accepted", &json!({"ok":true})).await,
            Err(_) => response_json(&mut stream, "503 Service Unavailable", &json!({"error":"updater_unavailable"})).await,
        };
    }
    if method == "PUT" && (path == "/api/settings" || path == "/api/settings/cloud") {
        let current = supervisor.settings();
        let parsed = if path == "/api/settings/cloud" {
            serde_json::from_slice::<CloudSettingRequest>(body).map(|request| {
                let editable = editable_from(&current, Some(request.enabled)); SettingsRequest { settings: editable, confirm_restart: request.confirm_restart }
            })
        } else { serde_json::from_slice::<SettingsRequest>(body) };
        let Ok(request) = parsed else { return response_json(&mut stream, "400 Bad Request", &json!({"error":"invalid_json"})).await };
        let confirm = request.confirm_restart; let updated = request.settings.apply_to(current.clone());
        if let Err(detail) = updated.validate() { return response_json(&mut stream, "422 Unprocessable Entity", &json!({"error":"invalid_settings","detail":detail})).await }
        if updated.cloud_discovery_enabled && !current.cloud_discovery_enabled {
            let missing = crate::cloud_discovery::cloud_configuration_missing();
            if !missing.is_empty() { return response_json(&mut stream, "422 Unprocessable Entity", &json!({"error":"cloud_not_provisioned","missing":missing})).await }
        }
        if updated == current && supervisor.watchdog().configuration_error.is_none() { return response_json(&mut stream, "200 OK", &json!({"ok":true,"restart_scheduled":false,"target_generation":supervisor.watchdog().receiver_generation})).await }
        if !confirm { return response_json(&mut stream, "409 Conflict", &json!({"error":"restart_confirmation_required"})).await }
        if supervisor.apply_settings(updated).is_err() { return response_json(&mut stream, "500 Internal Server Error", &json!({"error":"settings_persist_failed"})).await }
        let target = supervisor.restart("settings_restart").await.map_err(std::io::Error::other)?;
        return response_json(&mut stream, "202 Accepted", &json!({"ok":true,"restart_scheduled":true,"target_generation":target})).await;
    }
    response(&mut stream, "404 Not Found", "text/plain", b"Not Found", None).await
}

fn editable_from(settings: &ReceiverSettings, cloud: Option<bool>) -> EditableSettings {
    EditableSettings { port: settings.port, webtransport_port: settings.webtransport_port, http_port: settings.http_port,
        drm_connector_id: settings.drm_connector_id.clone(), drm_plane_id: settings.drm_plane_id.clone(), idle_dashboard: settings.idle_dashboard,
        idle_dashboard_mode: settings.idle_dashboard_mode.clone(), idle_timeout_sec: settings.idle_timeout_sec,
        sender_liveness_timeout_sec: settings.sender_liveness_timeout_sec, udp_buffer_size_mb: settings.udp_buffer_size_mb,
        pairing_code_ttl_sec: settings.pairing_code_ttl_sec, local_pairing_code_required: settings.local_pairing_code_required,
        cloud_discovery_enabled: cloud.unwrap_or(settings.cloud_discovery_enabled) }
}

fn request_is_same_origin(headers: &HashMap<String, String>) -> bool {
    if headers.get("sec-fetch-site").is_some_and(|value| value.eq_ignore_ascii_case("cross-site")) { return false }
    let Some(host) = headers.get("host") else { return false };
    for name in ["origin", "referer"] {
        if let Some(value) = headers.get(name) {
            if value != &format!("https://{host}") && value != &format!("http://{host}") && !value.starts_with(&format!("https://{host}/")) && !value.starts_with(&format!("http://{host}/")) { return false }
        }
    }
    true
}

async fn handle_websocket<S>(stream: S, supervisor: SupervisorHandle) -> Result<(), Box<dyn std::error::Error + Send + Sync>> where S: AsyncRead + AsyncWrite + Unpin + Send + 'static {
    let socket = tokio_tungstenite::accept_async(stream).await?; let (mut sender, mut receiver) = socket.split();
    sender.send(tokio_tungstenite::tungstenite::Message::Text(supervisor.snapshot().to_string().into())).await?;
    let mut updates = supervisor.subscribe(); let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    loop { tokio::select! {
        _ = tick.tick() => if sender.send(tokio_tungstenite::tungstenite::Message::Text(supervisor.snapshot().to_string().into())).await.is_err() { break },
        _ = updates.recv() => if sender.send(tokio_tungstenite::tungstenite::Message::Text(supervisor.snapshot().to_string().into())).await.is_err() { break },
        message = receiver.next() => match message { Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) if text.contains("stop") => { let _ = supervisor.stop_sharing().await; }, Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => break, _ => {} }
    } }
    Ok(())
}

async fn response_json<S: AsyncWrite + Unpin>(stream: &mut S, status: &str, value: &serde_json::Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    response(stream, status, "application/json", &serde_json::to_vec(value)?, None).await
}

async fn response<S: AsyncWrite + Unpin>(stream: &mut S, status: &str, content_type: &str, body: &[u8], extra: Option<&str>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let header = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{}Connection: close\r\n\r\n", body.len(), extra.unwrap_or(""));
    stream.write_all(header.as_bytes()).await?; stream.write_all(body).await?; stream.flush().await?; Ok(())
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, Box<dyn std::error::Error + Send + Sync>> { let mut reader = BufReader::new(File::open(path)?); Ok(rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>()?) }
fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>, Box<dyn std::error::Error + Send + Sync>> { let mut reader = BufReader::new(File::open(path)?); rustls_pemfile::private_key(&mut reader)?.ok_or_else(|| "No private key found".into()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn strict_restart_schema() { assert!(serde_json::from_str::<RestartRequest>(r#"{"confirm_restart":true,"extra":1}"#).is_err()); }
    #[test] fn strict_update_schema() { assert!(serde_json::from_str::<UpdateRequest>(r#"{"confirm_update":true,"extra":1}"#).is_err()); }
    #[test] fn cross_site_is_rejected() { let mut headers = HashMap::new(); headers.insert("host".into(), "device:9090".into()); headers.insert("sec-fetch-site".into(), "cross-site".into()); assert!(!request_is_same_origin(&headers)); }
}
