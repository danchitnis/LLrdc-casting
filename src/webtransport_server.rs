/*
 * Safe Rust WebTransport / QUIC UDP Server Module
 * Receives H.265 video streams over QUIC UDP port 4433
 */

use std::error::Error;
use tokio::sync::mpsc;
use wtransport::{Endpoint, Identity, ServerConfig};

use crate::v4l2_decoder::VideoFrame;

pub async fn get_or_create_identity() -> Result<Identity, Box<dyn Error + Send + Sync>> {
    crate::cert::get_or_create_identity().await
}

pub fn extract_cert_hash_hex(identity: &Identity) -> String {
    crate::cert::extract_cert_hash_hex(identity)
}

pub async fn run_server(
    frame_tx: mpsc::Sender<VideoFrame>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let identity = get_or_create_identity().await?;
    run_server_with_identity(identity, frame_tx).await
}

/// Start WebTransport QUIC UDP server on 0.0.0.0:4433 using existing identity
pub async fn run_server_with_identity(
    identity: Identity,
    frame_tx: mpsc::Sender<VideoFrame>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let hex_str = extract_cert_hash_hex(&identity);
    if !hex_str.is_empty() {
        println!("[WEBTRANSPORT] Persistent Certificate SHA-256 (HEX): {}", hex_str);
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
                    if frame_tx_udp.send(video_frame).await.is_err() {
                        break;
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
                                    if frame_tx_clone.send(video_frame).await.is_err() { break; }
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
                                let _ = frame_tx.send(video_frame).await;
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
