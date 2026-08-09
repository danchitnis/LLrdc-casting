/*
 * Safe Rust WebTransport / QUIC UDP Server Module
 * Receives H.265 video streams over QUIC UDP port 4433
 */

use std::error::Error;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use wtransport::{Endpoint, Identity, ServerConfig};

use crate::cloud_discovery::ConnectionTokenVerifier;
use crate::local_pairing::PairingState;
use crate::v4l2_decoder::VideoFrame;

pub async fn get_or_create_identity() -> Result<Identity, Box<dyn Error + Send + Sync>> {
    crate::cert::get_or_create_identity().await
}

pub fn extract_cert_hash_hex(identity: &Identity) -> String {
    crate::cert::extract_cert_hash_hex(identity)
}

/// Start WebTransport QUIC UDP server on 0.0.0.0:4433 using existing identity
pub async fn run_server_with_identity(
    identity: Identity,
    frame_tx: mpsc::Sender<VideoFrame>,
    control_channel: crate::control::ControlChannel,
    pairing_state: PairingState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let hex_str = extract_cert_hash_hex(&identity);
    if !hex_str.is_empty() {
        println!(
            "[WEBTRANSPORT] Persistent Certificate SHA-256 (HEX): {}",
            hex_str
        );
    }

    let wt_port: u16 = std::env::var("WEBTRANSPORT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4433);

    let udp_port: u16 = std::env::var("BOARD_PORT")
        .or_else(|_| std::env::var("UDP_PORT"))
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(4434);

    let buf_mb: usize = std::env::var("UDP_BUFFER_SIZE_MB")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8);

    let idle_timeout_sec: u64 = std::env::var("IDLE_TIMEOUT_SEC")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(30);

    let config = ServerConfig::builder()
        .with_bind_default(wt_port)
        .with_identity(identity)
        .max_idle_timeout(Some(std::time::Duration::from_secs(idle_timeout_sec)))?
        .build();

    let server = Endpoint::server(config)?;
    let token_verifier = if crate::cloud_discovery::cloud_discovery_enabled() {
        match ConnectionTokenVerifier::from_environment() {
            Ok(verifier) => Some(Arc::new(verifier)),
            Err(error) => {
                eprintln!("[WEBTRANSPORT] Optional cloud token verification unavailable: {error}");
                None
            }
        }
    } else {
        None
    };
    println!("\n=====================================================");
    println!(" [WEBTRANSPORT SERVER] Listening on UDP 0.0.0.0:{wt_port}");
    println!(" Ready for incoming LLrdc Casting stream!");
    println!("=====================================================\n");

    // Spawn companion UDP listener for direct UDP video frame packets
    let frame_tx_udp = frame_tx.clone();
    tokio::spawn(async move {
        if let Ok(socket) = tokio::net::UdpSocket::bind(("0.0.0.0", udp_port)).await {
            println!("[UDP RECEIVER] Listening on 0.0.0.0:{udp_port} for direct UDP video stream packets");

            // Set socket receive buffer using nix/libc socket options
            use std::os::unix::io::AsRawFd;
            let raw_fd = socket.as_raw_fd();
            let buf_size: libc::c_int = (buf_mb * 1024 * 1024) as libc::c_int;
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
        let control_channel_clone = control_channel.clone();
        let token_verifier_clone = token_verifier.as_ref().map(Arc::clone);
        let pairing_state_clone = pairing_state.clone();

        tokio::spawn(async move {
            match handle_connection(
                incoming_session,
                frame_tx_clone,
                control_channel_clone,
                token_verifier_clone,
                pairing_state_clone,
            )
            .await
            {
                Ok(_) => println!("[WEBTRANSPORT] Session completed cleanly."),
                Err(e) => eprintln!("[WEBTRANSPORT] Session error: {}", e),
            }
        });
    }
}

async fn handle_connection(
    incoming_session: wtransport::endpoint::IncomingSession,
    frame_tx: mpsc::Sender<VideoFrame>,
    control_channel: crate::control::ControlChannel,
    token_verifier: Option<Arc<ConnectionTokenVerifier>>,
    pairing_state: PairingState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let session_request = incoming_session.await?;
    println!(
        "[WEBTRANSPORT] Connection requested from path: '{}'",
        session_request.path()
    );

    let (code, token) = connection_query(session_request.path());
    let Some(code) = code else {
        session_request.forbidden().await;
        return Err("missing local pairing code".into());
    };
    let peer = session_request.remote_address().ip().to_string();
    if let Err(reason) = pairing_state.validate_code(&code, &peer) {
        session_request.forbidden().await;
        return Err(format!("local pairing rejected: {reason}").into());
    }
    if let (Some(token), Some(token_verifier)) = (token.as_deref(), token_verifier.as_ref()) {
        if let Err(reason) = token_verifier.verify(token) {
            session_request.forbidden().await;
            return Err(format!("optional cloud token rejected: {reason}").into());
        }
    }
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
                        println!("[WEBTRANSPORT] Stream accept channel closed ({})", e);
                        break;
                    }
                }
            }
            bi_res = connection.accept_bi() => {
                match bi_res {
                    Ok((send_stream, recv_stream)) => {
                        tokio::spawn(handle_control_stream(
                            send_stream,
                            recv_stream,
                            control_channel.clone(),
                        ));
                    }
                    Err(e) => println!("[WEBTRANSPORT] Control stream channel closed ({})", e),
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
                        // Do not break loop on datagram channel closure; keep unidirectional stream active
                        println!("[WEBTRANSPORT] Datagram channel inactive ({})", e);
                        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    }
                }
            }
        }
    }

    let stop_frame = VideoFrame {
        seq: 0,
        width: 0,
        height: 0,
        codec: "stop".to_string(),
        access_unit: Vec::new(),
        first_packet_at: std::time::Instant::now(),
    };
    let _ = frame_tx.send(stop_frame).await;

    Ok(())
}

fn connection_query(path: &str) -> (Option<String>, Option<String>) {
    let Some((_, query)) = path.split_once('?') else {
        return (None, None);
    };
    let mut code = None;
    let mut token = None;
    for item in query.split('&') {
        if let Some(value) = item.strip_prefix("code=") {
            code = Some(value.to_string());
        } else if let Some(value) = item.strip_prefix("token=") {
            token = Some(value.to_string());
        }
    }
    (code, token)
}

async fn handle_control_stream(
    mut send_stream: wtransport::SendStream,
    mut recv_stream: wtransport::RecvStream,
    control_channel: crate::control::ControlChannel,
) {
    let mut length = [0u8; 4];
    if recv_stream.read_exact(&mut length).await.is_err() {
        return;
    }
    let size = u32::from_be_bytes(length) as usize;
    if size == 0 || size > 64 * 1024 {
        return;
    }
    let mut first_payload = vec![0u8; size];
    if recv_stream.read_exact(&mut first_payload).await.is_err() {
        return;
    }
    if crate::ui_delivery::is_ui_request(&first_payload) {
        if let Err(error) = crate::ui_delivery::send_embedded_ui(&mut send_stream).await {
            eprintln!("[WEBTRANSPORT UI] Failed to send embedded UI: {error}");
        }
        return;
    }

    let mut telemetry_rx = control_channel.telemetry_tx.subscribe();
    let telemetry_task = tokio::spawn(async move {
        while let Ok(message) = telemetry_rx.recv().await {
            let Ok(payload) = serde_json::to_vec(&message) else {
                continue;
            };
            if write_control_message(&mut send_stream, &payload)
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let _ = control_channel
        .cmd_tx
        .send(crate::control::ControlCommand::GetStatus)
        .await;
    if let Ok(command) = serde_json::from_slice::<crate::control::ControlCommand>(&first_payload) {
        let _ = control_channel.cmd_tx.send(command).await;
    }
    loop {
        let mut length = [0u8; 4];
        if recv_stream.read_exact(&mut length).await.is_err() {
            break;
        }
        let size = u32::from_be_bytes(length) as usize;
        if size == 0 || size > 64 * 1024 {
            break;
        }
        let mut payload = vec![0u8; size];
        if recv_stream.read_exact(&mut payload).await.is_err() {
            break;
        }
        if let Ok(command) = serde_json::from_slice::<crate::control::ControlCommand>(&payload) {
            let _ = control_channel.cmd_tx.send(command).await;
        }
    }
    telemetry_task.abort();
}

async fn write_control_message(
    stream: &mut wtransport::SendStream,
    payload: &[u8],
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let length = u32::try_from(payload.len())?;
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    Ok(())
}
