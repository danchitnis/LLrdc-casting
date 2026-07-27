/*
 * Safe Rust WebTransport / QUIC UDP Server Module
 * Receives H.265 video streams over QUIC UDP port 4433
 */

use std::error::Error;
use std::path::Path;
use tokio::sync::mpsc;
use wtransport::{Endpoint, Identity, ServerConfig};

use crate::v4l2_decoder::VideoFrame;

async fn get_or_create_identity() -> Result<Identity, Box<dyn Error + Send + Sync>> {
    let cert_path = Path::new("cert.pem");
    let key_path = Path::new("key.pem");

    if !cert_path.exists() || !key_path.exists() {
        println!("[WEBTRANSPORT] Generating new persistent TLS certificate (13-day WebTransport spec)...");
        let subject_alt_names = vec![
            "localhost".to_string(),
            "127.0.0.1".to_string(),
            "192.168.1.72".to_string(),
            "0.0.0.0".to_string(),
        ];
        let key_pair = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::new(subject_alt_names)?;
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::days(1);
        params.not_after = now + time::Duration::days(13); // WebTransport spec limit: max 14 days
        let cert = params.self_signed(&key_pair)?;
        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();
        std::fs::write(cert_path, cert_pem)?;
        std::fs::write(key_path, key_pem)?;
        println!("[WEBTRANSPORT] Saved persistent TLS certificate to cert.pem and key.pem");
    } else {
        println!("[WEBTRANSPORT] Loading existing persistent TLS certificate from cert.pem / key.pem");
    }

    let identity = Identity::load_pemfiles(cert_path, key_path).await?;
    Ok(identity)
}

/// Start WebTransport QUIC UDP server on 0.0.0.0:4433
pub async fn run_server(
    frame_tx: mpsc::Sender<VideoFrame>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let identity = get_or_create_identity().await?;

    if let Some(cert) = identity.certificate_chain().as_slice().first() {
        let hash = cert.hash();
        let hash_bytes = hash.as_ref();
        let hex_str: String = hash_bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":");
        let array_str: String = hash_bytes.iter().map(|b| format!("{}", b)).collect::<Vec<_>>().join(",");
        println!("[WEBTRANSPORT] Persistent Certificate SHA-256 (HEX): {}", hex_str);
        println!("[WEBTRANSPORT] Persistent Certificate SHA-256 (BYTES): [{}]", array_str);
    }
    
    let config = ServerConfig::builder()
        .with_bind_default(4433)
        .with_identity(&identity)
        .build();

    let server = Endpoint::server(config)?;
    println!("\n=====================================================");
    println!(" [WEBTRANSPORT SERVER] Listening on UDP 0.0.0.0:4433");
    println!(" Ready for incoming WebTransport QUIC screen sharing!");
    println!("=====================================================\n");

    // Spawn companion UDP listener on 0.0.0.0:4434 for direct UDP video frame packets
    let frame_tx_udp = frame_tx.clone();
    tokio::spawn(async move {
        if let Ok(socket) = tokio::net::UdpSocket::bind("0.0.0.0:4434").await {
            println!("[UDP RECEIVER] Listening on 0.0.0.0:4434 for direct UDP video stream packets");

            // Set socket receive buffer to 8MB using nix/libc socket options
            use std::os::unix::io::AsRawFd;
            let raw_fd = socket.as_raw_fd();
            let buf_size: libc::c_int = 8 * 1024 * 1024; // 8MB socket buffer
            unsafe {
                libc::setsockopt(
                    raw_fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVBUF,
                    &buf_size as *const _ as *const libc::c_void,
                    std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                );
            }

            let mut buf = [0u8; 65536];
            while let Ok((len, _addr)) = socket.recv_from(&mut buf).await {
                if let Some(video_frame) = crate::v4l2_decoder::process_udp_chunk(&buf[..len]) {
                    // Never let the network task wait behind display/decode work.
                    // The receiver is intentionally latest-frame-wins for low latency.
                    if frame_tx_udp.try_send(video_frame).is_err() {
                        // A full queue means a newer access unit will be more useful.
                    }
                }
            }
        }
    });

    loop {
        let incoming_session = server.accept().await;
        let frame_tx_clone = frame_tx.clone();

        tokio::spawn(async move {
            match handle_connection(incoming_session, frame_tx_clone).await {
                Ok(_) => println!("[WEBTRANSPORT] Session completed cleanly."),
                Err(e) => eprintln!("[WEBTRANSPORT] Session error: {}", e),
            }
        });
    }
}

async fn handle_connection(
    incoming_session: wtransport::endpoint::IncomingSession,
    frame_tx: mpsc::Sender<VideoFrame>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let session_request = incoming_session.await?;
    println!(
        "[WEBTRANSPORT] Connection requested from path: '{}'",
        session_request.path()
    );

    let connection = session_request.accept().await?;
    println!("[WEBTRANSPORT] Client connected successfully via QUIC/UDP!");

    loop {
        tokio::select! {
            // Receive unidirectional streams for 100% reliable loss-free stream delivery
            uni_res = connection.accept_uni() => {
                match uni_res {
                    Ok(mut recv_stream) => {
                        let frame_tx_clone = frame_tx.clone();
                        tokio::spawn(async move {
                            let mut len_buf = [0u8; 4];
                            while recv_stream.read_exact(&mut len_buf).await.is_ok() {
                                let len = u32::from_be_bytes(len_buf) as usize;
                                if len == 0 || len > 16 * 1024 * 1024 { break; }
                                let mut packet = vec![0u8; len];
                                if recv_stream.read_exact(&mut packet).await.is_err() { break; }
                                if let Some(video_frame) = crate::v4l2_decoder::process_udp_chunk(&packet) {
                                    let _ = frame_tx_clone.try_send(video_frame);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        println!("[WEBTRANSPORT] Stream accept closed: {}", e);
                        break;
                    }
                }
            }
            // Also accept datagrams for ultra-low latency frame packets
            dgram_res = connection.receive_datagram() => {
                match dgram_res {
                    Ok(dgram) => {
                        let payload = dgram.payload().to_vec();
                        if !payload.is_empty() {
                            if let Some(video_frame) = crate::v4l2_decoder::process_udp_chunk(&payload) {
                                let _ = frame_tx.try_send(video_frame);
                            }
                        }
                    }
                    Err(e) => {
                        println!("[WEBTRANSPORT] Datagram receive closed: {}", e);
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
