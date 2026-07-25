/*
 * Safe Rust WebTransport / QUIC UDP Server Module
 * Receives H.264 video streams over QUIC UDP port 4433
 */

use std::error::Error;
use tokio::sync::mpsc;
use wtransport::{Endpoint, Identity, ServerConfig};

/// Start WebTransport QUIC UDP server on 0.0.0.0:4433
pub async fn run_server(
    frame_tx: mpsc::Sender<Vec<u8>>,
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

    // Spawn companion UDP listener on 0.0.0.0:4434 for dev testing & UDP frames
    let frame_tx_udp = frame_tx.clone();
    tokio::spawn(async move {
        if let Ok(socket) = tokio::net::UdpSocket::bind("0.0.0.0:4434").await {
            println!("[UDP RECEIVER] Listening on 0.0.0.0:4434 for direct H.264 UDP frame packets");
            let mut buf = [0u8; 65536];
            while let Ok((len, addr)) = socket.recv_from(&mut buf).await {
                println!("[UDP RECEIVER] Received {} bytes of H.264 frame from {}", len, addr);
                let _ = frame_tx_udp.send(buf[..len].to_vec()).await;
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
    frame_tx: mpsc::Sender<Vec<u8>>,
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
                            println!("[WEBTRANSPORT] Received H.264 stream packet ({} bytes)", buffer.len());
                            let _ = frame_tx.send(buffer).await;
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
                            println!("[WEBTRANSPORT] Received H.264 datagram packet ({} bytes)", payload.len());
                            let _ = frame_tx.send(payload).await;
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
