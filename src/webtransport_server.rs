/*
 * Safe Rust WebTransport / QUIC UDP Server Module
 * Receives H.264 video streams over QUIC UDP port 4433
 */

use std::error::Error;
use tokio::sync::mpsc;
use wtransport::{Endpoint, Identity, ServerConfig};

use crate::v4l2_decoder::VideoFrame;

/// Start WebTransport QUIC UDP server on 0.0.0.0:4433
pub async fn run_server(
    frame_tx: mpsc::Sender<VideoFrame>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let identity = Identity::self_signed(["localhost", "127.0.0.1", "192.168.1.72", "0.0.0.0"])?;
    
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
            // Receive unidirectional streams containing frame packets
            uni_res = connection.accept_uni() => {
                match uni_res {
                    Ok(mut recv_stream) => {
                        let mut buffer = Vec::new();
                        let mut temp = [0u8; 65536];
                        while let Ok(Some(n)) = recv_stream.read(&mut temp).await {
                            if n == 0 { break; }
                            buffer.extend_from_slice(&temp[..n]);
                        }
                        if !buffer.is_empty() {
                            if let Some(video_frame) = crate::v4l2_decoder::process_udp_chunk(&buffer) {
                                let _ = frame_tx.try_send(video_frame);
                            }
                        }
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
