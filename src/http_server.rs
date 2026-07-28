/*
 * Lightweight HTTPS & HTTP Server Module
 * Serves the embedded screen sharing web client (client/index.html)
 * and TLS certificate fingerprint endpoint on port 8080.
 */

use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::rustls::{Certificate, PrivateKey, ServerConfig};
use tokio_rustls::TlsAcceptor;

static INDEX_HTML: &str = include_str!("../client/index.html");

fn load_certs(path: &Path) -> Result<Vec<Certificate>, Box<dyn Error + Send + Sync>> {
    let certfile = File::open(path)?;
    let mut reader = BufReader::new(certfile);
    let certs = rustls_pemfile::certs(&mut reader)?
        .into_iter()
        .map(Certificate)
        .collect();
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKey, Box<dyn Error + Send + Sync>> {
    let keyfile = File::open(path)?;
    let mut reader = BufReader::new(keyfile);
    let keys = rustls_pemfile::pkcs8_private_keys(&mut reader)?;
    if let Some(key) = keys.into_iter().next() {
        return Ok(PrivateKey(key));
    }
    let keyfile = File::open(path)?;
    let mut reader = BufReader::new(keyfile);
    let keys = rustls_pemfile::rsa_private_keys(&mut reader)?;
    if let Some(key) = keys.into_iter().next() {
        return Ok(PrivateKey(key));
    }
    Err("No private key found in key.pem".into())
}

async fn serve_http_request<S>(
    stream: &mut S,
    cert_hash: &str,
    initial_buf: Option<&[u8]>,
) -> Result<(), Box<dyn Error + Send + Sync>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = [0u8; 2048];
    let mut read_bytes = 0;

    if let Some(init) = initial_buf {
        let copy_len = init.len().min(buf.len());
        buf[..copy_len].copy_from_slice(&init[..copy_len]);
        read_bytes = copy_len;
    }

    if read_bytes == 0 {
        let n = stream.read(&mut buf).await?;
        if n == 0 { return Ok(()); }
        read_bytes = n;
    }

    let req_str = String::from_utf8_lossy(&buf[..read_bytes]);
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

pub async fn run_server(
    cert_hash_hex: String,
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
                    .with_safe_defaults()
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
    println!(" HTTPS: https://<BOARD_IP>:{port}/");
    println!(" HTTP : http://<BOARD_IP>:{port}/");
    println!("=====================================================\n");

    loop {
        let (mut socket, _peer_addr) = match listener.accept().await {
            Ok(val) => val,
            Err(e) => {
                eprintln!("[HTTP SERVER] Accept error: {e}");
                continue;
            }
        };

        let cert_hash = Arc::clone(&shared_cert_hash);
        let acceptor = tls_acceptor.clone();

        tokio::spawn(async move {
            // Peek at first byte to distinguish TLS ClientHello (0x16) vs Plain HTTP ('G', 'P', etc.)
            let mut first_byte = [0u8; 1];
            let n = match socket.peek(&mut first_byte).await {
                Ok(n) => n,
                Err(_) => return,
            };

            if n > 0 && first_byte[0] == 0x16 {
                // TLS Handshake
                if let Some(acceptor) = acceptor {
                    if let Ok(mut tls_stream) = acceptor.accept(socket).await {
                        let _ = serve_http_request(&mut tls_stream, &cert_hash, None).await;
                    }
                }
            } else {
                // Plain HTTP
                let _ = serve_http_request(&mut socket, &cert_hash, None).await;
            }
        });
    }
}
