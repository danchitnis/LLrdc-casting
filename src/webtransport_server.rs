/*
 * Safe Rust WebTransport / QUIC UDP Server Module
 * Receives H.265 video streams over QUIC UDP port 4433
 */

use std::error::Error;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use wtransport::{Endpoint, Identity, ServerConfig};

use crate::cloud_discovery::ConnectionTokenVerifier;
use crate::config;
use crate::local_pairing::PairingState;
use crate::v4l2_decoder::VideoFrame;
use crate::management::ManagementState;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub async fn get_or_create_identity() -> Result<Identity, Box<dyn Error + Send + Sync>> {
    let (cert_path, key_path) = crate::cert::get_cert_and_key_paths();
    Ok(Identity::load_pemfiles(cert_path, key_path).await?)
}

pub fn extract_cert_hash_hex(identity: &Identity) -> String {
    crate::cert::extract_cert_hash_hex(identity)
}

fn udp_receive_buffer_bytes(megabytes: usize) -> Option<usize> {
    megabytes.checked_mul(1024 * 1024)
}

fn normalize_peer_ip(address: IpAddr) -> String {
    match address {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => ip.to_ipv4().map_or_else(|| ip.to_string(), |mapped| mapped.to_string()),
    }
}

/// Start WebTransport QUIC UDP server on 0.0.0.0:4433 using existing identity
pub async fn run_server_with_identity(
    identity: Identity,
    frame_tx: mpsc::Sender<VideoFrame>,
    control_channel: crate::control::ControlChannel,
    pairing_state: PairingState,
    management: ManagementState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let hex_str = extract_cert_hash_hex(&identity);
    if !hex_str.is_empty() {
        println!(
            "[WEBTRANSPORT] Persistent Certificate SHA-256 (HEX): {}",
            hex_str
        );
    }

    let wt_port = config::env_or(
        "WEBTRANSPORT_PORT",
        config::server::DEFAULT_WEBTRANSPORT_PORT,
    );

    let udp_port: u16 = std::env::var("BOARD_PORT")
        .or_else(|_| std::env::var("UDP_PORT"))
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(config::server::DEFAULT_BOARD_PORT);

    let buf_mb: usize = std::env::var("UDP_BUFFER_SIZE_MB")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(config::server::DEFAULT_UDP_BUFFER_SIZE_MB);

    let idle_timeout_sec: u64 = std::env::var("IDLE_TIMEOUT_SEC")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(config::server::DEFAULT_IDLE_TIMEOUT_SEC);

    // Use the wildcard bind so every receiver interface accepts WebTransport.
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
                eprintln!("[WEBTRANSPORT] Cloud token verification unavailable: {error}");
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

            // Set socket receive buffer using Rustix's safe socket-option wrapper.
            if let Some(buf_size) = udp_receive_buffer_bytes(buf_mb) {
                if let Err(error) =
                    rustix::net::sockopt::set_socket_recv_buffer_size(&socket, buf_size)
                {
                    eprintln!(
                        "[UDP RECEIVER] Could not set receive buffer to {buf_size} bytes: {error}"
                    );
                }
            } else {
                eprintln!("[UDP RECEIVER] UDP_BUFFER_SIZE_MB is too large; keeping the OS default");
            }

            let mut buf = [0u8; config::transport::DATAGRAM_BUFFER_BYTES];
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
        let management_clone = management.clone();

        tokio::spawn(async move {
            match handle_connection(
                incoming_session,
                frame_tx_clone,
                control_channel_clone,
                token_verifier_clone,
                pairing_state_clone,
                management_clone,
            )
            .await
            {
                Ok(_) => println!("[WEBTRANSPORT] Session completed cleanly."),
                Err(e) => eprintln!("[WEBTRANSPORT] Session error: {}", e),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_peer_ip, pairing_admission, route_control_payload, udp_receive_buffer_bytes};
    use crate::control::{ControlCommand, TelemetryMessage};
    use crate::management::{ClientMetadata, ManagementState};
    use std::net::IpAddr;
    use tokio::sync::mpsc;

    #[test]
    fn udp_buffer_size_uses_megabytes() {
        assert_eq!(udp_receive_buffer_bytes(8), Some(8 * 1024 * 1024));
        assert_eq!(udp_receive_buffer_bytes(0), Some(0));
    }

    #[test]
    fn udp_buffer_size_rejects_overflow() {
        assert_eq!(udp_receive_buffer_bytes(usize::MAX), None);
    }

    #[test]
    fn normalizes_ipv4_mapped_peer_addresses() {
        let mapped: IpAddr = "::ffff:192.0.2.44".parse().expect("mapped IPv4 address");
        assert_eq!(normalize_peer_ip(mapped), "192.0.2.44");
        assert_eq!(normalize_peer_ip("2001:db8::1".parse().unwrap()), "2001:db8::1");
    }

    #[test]
    fn pairing_admission_allows_only_code_free_lan_when_disabled() {
        assert!(pairing_admission(false, false, false).is_ok());
        assert!(pairing_admission(true, false, false).is_err());
        assert!(pairing_admission(false, true, false).is_ok());
        assert!(pairing_admission(false, false, true).is_err());
    }

    #[tokio::test]
    async fn ping_is_answered_on_the_originating_control_channel() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let management = ManagementState::new();
        management.hello(ClientMetadata {
            device_id: "test".into(), user_agent: "test".into(), platform: "test".into(),
            language: "en".into(), page_session_id: "test-page".into(),
            remote_ip: "127.0.0.1".into(), connection_id: "test".into(),
        });

        route_control_payload(
            br#"{"type":"ping","id":42}"#,
            &cmd_tx,
            &outbound_tx,
            "test",
            "127.0.0.1",
            &management,
        )
        .await;

        assert!(cmd_rx.try_recv().is_err());
        assert!(management.touch_connection("test"));
        let payload = outbound_rx.recv().await.expect("targeted pong");
        let pong: TelemetryMessage = serde_json::from_slice(&payload).expect("valid pong");
        assert!(matches!(pong, TelemetryMessage::Pong { id: Some(42) }));
    }

    #[tokio::test]
    async fn legacy_ping_without_id_is_still_answered() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let management = ManagementState::new();

        route_control_payload(br#"{"type":"ping"}"#, &cmd_tx, &outbound_tx, "test", "127.0.0.1", &management).await;

        assert!(cmd_rx.try_recv().is_err());
        let payload = outbound_rx.recv().await.expect("legacy pong");
        let pong: TelemetryMessage = serde_json::from_slice(&payload).expect("valid pong");
        assert!(matches!(pong, TelemetryMessage::Pong { id: None }));
    }

    #[tokio::test]
    async fn non_ping_commands_still_reach_the_device_command_queue() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let management = ManagementState::new();

        route_control_payload(br#"{"type":"get_status"}"#, &cmd_tx, &outbound_tx, "test", "127.0.0.1", &management).await;

        assert!(matches!(cmd_rx.recv().await, Some(ControlCommand::GetStatus)));
        assert!(outbound_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn client_diagnostics_are_bounded_and_attributed_to_authenticated_connection() {
        let (cmd_tx, mut cmd_rx) = mpsc::channel(4);
        let (outbound_tx, mut outbound_rx) = mpsc::channel(4);
        let management = ManagementState::new();
        management.hello(ClientMetadata {
            device_id: "test-device".into(), user_agent: "test".into(), platform: "test".into(),
            language: "en".into(), page_session_id: "test-page".into(),
            remote_ip: "127.0.0.1".into(), connection_id: "wt-7".into(),
        });

        let long_message = "x".repeat(5000);
        let payload = serde_json::to_vec(&serde_json::json!({
            "type": "client_diagnostic",
            "level": "warning",
            "message": long_message,
        })).expect("diagnostic payload");
        route_control_payload(&payload, &cmd_tx, &outbound_tx, "wt-7", "127.0.0.1", &management).await;

        assert!(cmd_rx.try_recv().is_err());
        assert!(outbound_rx.try_recv().is_err());
        let event = management.snapshot().events.into_iter().last().expect("diagnostic event");
        assert_eq!(event.kind, "client_diagnostic");
        assert_eq!(event.level, "warn");
        assert!(event.message.starts_with("client=wt-7: "));
        assert_eq!(event.message.chars().count(), "client=wt-7: ".chars().count() + 4096);
    }
}

async fn handle_connection(
    incoming_session: wtransport::endpoint::IncomingSession,
    frame_tx: mpsc::Sender<VideoFrame>,
    control_channel: crate::control::ControlChannel,
    token_verifier: Option<Arc<ConnectionTokenVerifier>>,
    pairing_state: PairingState,
    management: ManagementState,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let session_request = incoming_session.await?;
    println!("[WEBTRANSPORT] Connection request received");

    let (code, token) = connection_query(session_request.path());
    let pairing_required = crate::config::settings().local_pairing_code_required;
    let peer = normalize_peer_ip(session_request.remote_address().ip());
    if let Err(reason) = pairing_admission(pairing_required, code.is_some(), token.is_some()) {
        management.event("warn", "security_rejected", format!("peer={peer} reason={reason}"));
        session_request.forbidden().await;
        return Err(reason.into());
    }
    let connection_id = format!("wt-{}", NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed));
    if let Some(code) = code.as_deref() {
        if let Err(reason) = pairing_state.validate_code(code, &peer) {
            management.event("warn", "pairing_rejected", format!("peer={peer} reason={reason}"));
            session_request.forbidden().await;
            return Err(format!("local pairing rejected: {reason}").into());
        }
    }
    if let Some(token) = token.as_deref() {
        let Some(token_verifier) = token_verifier.as_ref() else {
            management.event("error", "cloud_token_rejected", format!("peer={peer} reason=verifier_unavailable"));
            session_request.forbidden().await;
            return Err("cloud token verifier unavailable".into());
        };
        if let Err(reason) = token_verifier.verify(token) {
            management.event("warn", "cloud_token_rejected", format!("peer={peer} reason={reason}"));
            session_request.forbidden().await;
            return Err(format!("cloud token rejected: {reason}").into());
        }
    }
    let connection = session_request.accept().await?;
    println!("[WEBTRANSPORT] Client connected successfully via QUIC/UDP!");
    let authentication = if token.is_some() { "cloud_token" } else if code.is_some() { "lan_security_code" } else { "direct_lan" };
    management.event("info", "connection_accepted", format!("connection={connection_id} peer={peer} authentication={authentication}"));

    loop {
        tokio::select! {
            // Receive unidirectional streams for 100% reliable loss-free stream delivery
            uni_res = connection.accept_uni() => {
                match uni_res {
                    Ok(mut recv_stream) => {
                        let frame_tx_clone = frame_tx.clone();
                        tokio::spawn(async move {
                            let mut len_buf = [0u8; config::transport::LENGTH_PREFIX_BYTES];
                            while recv_stream.read_exact(&mut len_buf).await.is_ok() {
                                let len = u32::from_be_bytes(len_buf) as usize;
                                if len == 0 || len > config::packet::MAX_UNI_STREAM_MESSAGE_BYTES { break; }
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
                            connection_id.clone(),
                            peer.clone(),
                            management.clone(),
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
                        tokio::time::sleep(std::time::Duration::from_secs(config::transport::DATAGRAM_ERROR_RETRY_SEC)).await;
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
    management.connection_closed(&connection_id);

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

fn pairing_admission(pairing_required: bool, has_code: bool, has_token: bool) -> Result<(), &'static str> {
    if !has_code && (pairing_required || has_token) {
        Err(if has_token { "cloud token requires a pairing code" } else { "missing local pairing code" })
    } else {
        Ok(())
    }
}

async fn handle_control_stream(
    mut send_stream: wtransport::SendStream,
    mut recv_stream: wtransport::RecvStream,
    control_channel: crate::control::ControlChannel,
    connection_id: String,
    remote_ip: String,
    management: ManagementState,
) {
    let mut length = [0u8; config::transport::LENGTH_PREFIX_BYTES];
    if recv_stream.read_exact(&mut length).await.is_err() {
        return;
    }
    let size = u32::from_be_bytes(length) as usize;
    if size == 0 || size > config::packet::MAX_CONTROL_MESSAGE_BYTES {
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

    // A single writer owns the stream so broadcast telemetry and a direct pong
    // can never interleave their length-prefixed messages.
    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Vec<u8>>(32);
    let writer_task = tokio::spawn(async move {
        while let Some(payload) = outbound_rx.recv().await {
            if write_control_message(&mut send_stream, &payload).await.is_err() {
                break;
            }
        }
    });

    let mut telemetry_rx = control_channel.telemetry_tx.subscribe();
    let telemetry_tx = outbound_tx.clone();
    let telemetry_task = tokio::spawn(async move {
        while let Ok(message) = telemetry_rx.recv().await {
            let Ok(payload) = serde_json::to_vec(&message) else {
                continue;
            };
            if telemetry_tx.send(payload).await.is_err() {
                break;
            }
        }
    });

    let _ = control_channel
        .cmd_tx
        .send(crate::control::ControlCommand::GetStatus)
        .await;
    route_control_payload(&first_payload, &control_channel.cmd_tx, &outbound_tx, &connection_id, &remote_ip, &management).await;
    loop {
        let mut length = [0u8; config::transport::LENGTH_PREFIX_BYTES];
        if recv_stream.read_exact(&mut length).await.is_err() {
            break;
        }
        let size = u32::from_be_bytes(length) as usize;
        if size == 0 || size > config::packet::MAX_CONTROL_MESSAGE_BYTES {
            break;
        }
        let mut payload = vec![0u8; size];
        if recv_stream.read_exact(&mut payload).await.is_err() {
            break;
        }
        route_control_payload(&payload, &control_channel.cmd_tx, &outbound_tx, &connection_id, &remote_ip, &management).await;
    }
    telemetry_task.abort();
    writer_task.abort();
    if management.connection_closed(&connection_id) {
        let _ = control_channel.cmd_tx.send(crate::control::ControlCommand::Stop).await;
    }
}

async fn route_control_payload(
    payload: &[u8],
    cmd_tx: &mpsc::Sender<crate::control::ControlCommand>,
    outbound_tx: &mpsc::Sender<Vec<u8>>,
    connection_id: &str,
    remote_ip: &str,
    management: &ManagementState,
) {
    let Ok(mut command) = serde_json::from_slice::<crate::control::ControlCommand>(payload) else {
        return;
    };
    match &mut command {
        crate::control::ControlCommand::ClientHello { connection_id: id, remote_ip: ip, .. } => {
            *id = Some(connection_id.to_string());
            *ip = Some(remote_ip.to_string());
        }
        crate::control::ControlCommand::Start { connection_id: id, .. } => *id = Some(connection_id.to_string()),
        _ => {}
    }
    match command {
        crate::control::ControlCommand::Ping { id } => {
            management.touch_connection(connection_id);
            let response = crate::control::TelemetryMessage::Pong { id };
            if let Ok(response_payload) = serde_json::to_vec(&response) {
                let _ = outbound_tx.send(response_payload).await;
            }
        }
        crate::control::ControlCommand::ClientDiagnostic { level, message } => {
            // Diagnostics are recorded at the authenticated transport boundary
            // so the client cannot spoof another connection's identity.
            management.touch_connection(connection_id);
            let normalized_level = match level.to_ascii_lowercase().as_str() {
                "error" => "error",
                "warn" | "warning" => "warn",
                "debug" => "debug",
                _ => "info",
            };
            let bounded_message: String = message.chars().take(4096).collect();
            if !bounded_message.is_empty() {
                management.event(
                    normalized_level,
                    "client_diagnostic",
                    format!("client={connection_id}: {bounded_message}"),
                );
            }
        }
        command => {
            let _ = cmd_tx.send(command).await;
        }
    }
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
