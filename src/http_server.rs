/*
 * Lightweight HTTPS & HTTP Server Module with Integrated Independent WebSocket Control Socket
 * Serves the embedded LLrdc-casting web client (client/index.html)
 * and independent control/telemetry WebSocket endpoint at /ws.
 */

use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use crate::control::{ControlChannel, ControlCommand};

static INDEX_HTML: &str = include_str!("../client/index.html");

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, Box<dyn Error + Send + Sync>> {
    let certfile = File::open(path)?;
    let mut reader = BufReader::new(certfile);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>, Box<dyn Error + Send + Sync>> {
    let keyfile = File::open(path)?;
    let mut reader = BufReader::new(keyfile);
    if let Some(key) = rustls_pemfile::private_key(&mut reader)? {
        return Ok(key);
    }
    Err("No private key found in key.pem".into())
}

struct PrefixStream<S> {
    prefix: Option<Vec<u8>>,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for PrefixStream<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if let Some(prefix) = self.prefix.as_mut() {
            if prefix.is_empty() {
                self.prefix = None;
            } else if prefix.len() <= buf.remaining() {
                buf.put_slice(prefix);
                self.prefix = None;
                return std::task::Poll::Ready(Ok(()));
            } else {
                let rest = prefix.split_off(buf.remaining());
                buf.put_slice(prefix);
                self.prefix = Some(rest);
                return std::task::Poll::Ready(Ok(()));
            }
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for PrefixStream<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn handle_websocket_connection<S>(stream: S, control_channel: ControlChannel)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[WS CONTROL ERROR] Handshake failed: {e}");
            return;
        }
    };

    println!("[WS CONTROL] Client connected to independent control socket!");
    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let mut telemetry_rx = control_channel.telemetry_tx.subscribe();
    let cmd_tx = control_channel.cmd_tx.clone();

    // Broadcast telemetry to connected client
    tokio::spawn(async move {
        while let Ok(msg) = telemetry_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if ws_tx.send(tokio_tungstenite::tungstenite::Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Trigger status query so newly connected client gets current status immediately
    let _ = cmd_tx.send(ControlCommand::GetStatus).await;

    // Handle incoming JSON control commands from browser
    while let Some(msg_res) = ws_rx.next().await {
        match msg_res {
            Ok(tokio_tungstenite::tungstenite::Message::Text(txt)) => {
                if let Ok(cmd) = serde_json::from_str::<ControlCommand>(&txt) {
                    println!("[WS CONTROL] Received command: {:?}", cmd);
                    let _ = cmd_tx.send(cmd).await;
                }
            }
            Ok(tokio_tungstenite::tungstenite::Message::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }
    println!("[WS CONTROL] Client disconnected from control socket.");
}

async fn serve_http_request<S>(
    stream: &mut S,
    cert_hash: &str,
    initial_buf: &[u8],
) -> Result<(), Box<dyn Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let req_str = String::from_utf8_lossy(initial_buf);
    let first_line = req_str.lines().next().unwrap_or("");
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    if method != "GET" && method != "HEAD" {
        let response = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    let (status, content_type, body) = match path {
        "/" | "/index.html" => ("200 OK", "text/html; charset=utf-8", INDEX_HTML.as_bytes()),
        "/cert_hash" => ("200 OK", "text/plain; charset=utf-8", cert_hash.as_bytes()),
        "/health" => ("200 OK", "text/plain; charset=utf-8", b"OK" as &[u8]),
        _ => ("404 Not Found", "text/plain; charset=utf-8", b"Not Found" as &[u8]),
    };

    let response_headers = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );

    if method == "HEAD" {
        stream.write_all(response_headers.as_bytes()).await?;
    } else {
        stream.write_all(response_headers.as_bytes()).await?;
        stream.write_all(body).await?;
    }
    stream.flush().await?;
    Ok(())
}

async fn handle_connection<S>(
    mut stream: S,
    cert_hash: &str,
    control_channel: ControlChannel,
) -> Result<(), Box<dyn Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).await?;
    if n == 0 { return Ok(()); }

    let req_str = String::from_utf8_lossy(&buf[..n]);
    let req_lower = req_str.to_lowercase();

    let prefix_stream = PrefixStream {
        prefix: Some(buf[..n].to_vec()),
        inner: stream,
    };

    if req_lower.contains("upgrade: websocket") || req_str.contains("/ws") || req_str.contains("/control") {
        handle_websocket_connection(prefix_stream, control_channel).await;
    } else {
        let mut prefix_stream = prefix_stream;
        let _ = serve_http_request(&mut prefix_stream, cert_hash, &buf[..n]).await;
    }
    Ok(())
}

pub async fn run_server(
    cert_hash_hex: String,
    control_channel: ControlChannel,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let port: u16 = std::env::var("HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    let (cert_path, key_path) = crate::cert::get_cert_and_key_paths();

    let tls_acceptor = if cert_path.exists() && key_path.exists() {
        match (load_certs(&cert_path), load_key(&key_path)) {
            (Ok(certs), Ok(key)) => {
                let config = ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(certs, key)?;
                Some(TlsAcceptor::from(Arc::new(config)))
            }
            (Err(e), _) => {
                eprintln!("[HTTPS] Failed to load certs: {e}");
                None
            }
            (_, Err(e)) => {
                eprintln!("[HTTPS] Failed to load key: {e}");
                None
            }
        }
    } else {
        None
    };

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    let shared_cert_hash = Arc::new(cert_hash_hex);

    println!("\n=====================================================");
    println!(" [HTTP/HTTPS SERVER] Listening on port {port}");
    println!(" Control Socket : wss://<BOARD_IP>:{port}/ws");
    println!(" Web Share UI   : https://<BOARD_IP>:{port}/");
    println!("=====================================================\n");

    loop {
        let (socket, _peer_addr) = match listener.accept().await {
            Ok(val) => val,
            Err(e) => {
                eprintln!("[HTTP SERVER] Accept error: {e}");
                continue;
            }
        };

        let cert_hash = Arc::clone(&shared_cert_hash);
        let acceptor = tls_acceptor.clone();
        let channel = control_channel.clone();

        tokio::spawn(async move {
            let mut first_byte = [0u8; 1];
            let n = match socket.peek(&mut first_byte).await {
                Ok(n) => n,
                Err(_) => return,
            };

            if n > 0 && first_byte[0] == 0x16 {
                if let Some(acceptor) = acceptor {
                    if let Ok(tls_stream) = acceptor.accept(socket).await {
                        let _ = handle_connection(tls_stream, &cert_hash, channel).await;
                    }
                }
            } else {
                let _ = handle_connection(socket, &cert_hash, channel).await;
            }
        });
    }
}
