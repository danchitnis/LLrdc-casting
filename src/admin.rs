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
        else if method == "PUT" && path == "/api/settings/cloud" {
            if !origin_ok { ("403 Forbidden", "application/json", br#"{"error":"cross_origin"}"#.to_vec()) }
            else if !headers.get("content-type").is_some_and(|value| value.split(';').next().is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"))) { ("415 Unsupported Media Type", "application/json", br#"{"error":"application_json_required"}"#.to_vec()) }
            else {
                match serde_json::from_slice::<CloudSettingRequest>(body) {
                    Err(_) => ("400 Bad Request", "application/json", br#"{"error":"invalid_json"}"#.to_vec()),
                    Ok(request) => {
                        let current = crate::cloud_discovery::cloud_discovery_enabled();
                        if request.enabled == current { ("200 OK", "application/json", serde_json::to_vec(&json!({"ok": true, "cloud_discovery_enabled": current, "restart_scheduled": false})).unwrap_or_default()) }
                        else if !request.confirm_restart { ("409 Conflict", "application/json", br#"{"error":"restart_confirmation_required"}"#.to_vec()) }
                        else if request.enabled {
                            let missing = crate::cloud_discovery::cloud_configuration_missing();
                            if !missing.is_empty() { ("422 Unprocessable Entity", "application/json", serde_json::to_vec(&json!({"error":"cloud_not_provisioned", "missing": missing})).unwrap_or_default()) }
                            else if let Err(error) = crate::cloud_discovery::persist_cloud_discovery_enabled(true) { eprintln!("[ADMIN] Could not persist cloud setting: {error}"); ("500 Internal Server Error", "application/json", br#"{"error":"settings_persist_failed"}"#.to_vec()) }
                            else { restart = true; management.event("info", "settings", "cloud discovery enabled; receiver restart scheduled"); ("202 Accepted", "application/json", br#"{"ok":true,"cloud_discovery_enabled":true,"restart_scheduled":true}"#.to_vec()) }
                        } else if let Err(error) = crate::cloud_discovery::persist_cloud_discovery_enabled(false) { eprintln!("[ADMIN] Could not persist cloud setting: {error}"); ("500 Internal Server Error", "application/json", br#"{"error":"settings_persist_failed"}"#.to_vec()) }
                        else { restart = true; management.event("info", "settings", "cloud discovery disabled; receiver restart scheduled"); ("202 Accepted", "application/json", br#"{"ok":true,"cloud_discovery_enabled":false,"restart_scheduled":true}"#.to_vec()) }
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
