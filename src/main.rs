//! Verified RK3399 fallback: UDP HEVC -> V4L2 stateless decoder -> KMS.
//! The atomic two-plane presenter is developed separately; this keeps HDMI
//! playback on the proven pipeline while it is completed.
#![deny(dead_code)]
#![forbid(unsafe_code)]

use llrdc_casting::{cloud_discovery, config, control, dashboard, drm_kms, http_server,
    local_pairing, management, playback, receiver_ipc, sys_monitor, v4l2_decoder,
    webtransport_server};

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    config::initialize().map_err(|error| std::io::Error::other(error.to_string()))?;
    let receiver_settings = config::settings();
    sys_monitor::spawn_dmesg_kernel_monitor();
    let _ = drm_kms::inspect_live_scanout_status();

    let (tx, mut rx) = mpsc::channel::<v4l2_decoder::VideoFrame>(config::transport::FRAME_CHANNEL_CAPACITY);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<control::ControlCommand>(config::transport::CONTROL_CHANNEL_CAPACITY);
    let control_channel = control::ControlChannel::new(cmd_tx);
    let management = management::ManagementState::new();

    let identity = webtransport_server::get_or_create_identity()
        .await
        .map_err(|e| e as Box<dyn std::error::Error>)?;
    let cert_hash_hex = webtransport_server::extract_cert_hash_hex(&identity);
    let fixed_pairing_code = std::env::var("PAIRING_CODE_FIXED")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let cloud_discovery_enabled = cloud_discovery::cloud_discovery_enabled();
    if fixed_pairing_code.is_some() && cloud_discovery_enabled {
        return Err("fixed pairing codes cannot be used with Cloudflare discovery enabled".into());
    }
    let pairing_state = local_pairing::PairingState::with_fixed_code(fixed_pairing_code, receiver_settings.local_pairing_code_required)?;
    local_pairing::spawn_local_pairing(pairing_state.clone());
    if cloud_discovery_enabled {
        cloud_discovery::spawn_registration(pairing_state.clone(), cert_hash_hex.clone());
    }

    let receiver_ready = Arc::new(AtomicBool::new(false));
    let ipc_pairing_state = pairing_state.clone();
    let ipc_management = management.clone();
    let ipc_commands = control_channel.cmd_tx.clone();
    let ipc_ready = receiver_ready.clone();
    tokio::spawn(async move {
        if let Err(error) = receiver_ipc::run(ipc_pairing_state, ipc_management, ipc_commands, ipc_ready).await {
            eprintln!("[RECEIVER IPC] Server stopped: {error}");
        }
    });

    let http_cert_hash = cert_hash_hex.clone();
    let http_control_channel = control_channel.clone();
    tokio::spawn(async move {
        if let Err(error) = http_server::run_server(http_cert_hash, http_control_channel).await {
            eprintln!("[HTTP SERVER ERROR] {error}");
        }
    });

    let webtransport_control_channel = control_channel.clone();
    let webtransport_pairing_state = pairing_state.clone();
    let webtransport_management = management.clone();
    tokio::spawn(async move {
        if let Err(error) = webtransport_server::run_server_with_identity(
            identity,
            tx,
            webtransport_control_channel,
            webtransport_pairing_state,
            webtransport_management,
        )
        .await
        {
            eprintln!("[SERVER ERROR] {error}");
        }
    });

    // 1. Auto-detect display geometry once before starting GStreamer
    let (screen_w, screen_h, vrefresh, connector_id, render_rect, edid_info) =
        playback::autodetect_display_info();
    management.set_health(management::HealthSnapshot {
        display_resolution: format!("{}x{}", screen_w, screen_h), display_fps: vrefresh,
        panel_resolution: edid_info.panel_res.clone(), edid_name: edid_info.name.clone(),
        edid_type: edid_info.conn_type.clone(), pairing_status: "READY".into(),
        cloud_status: "UNKNOWN".into(), playback_state: "idle_dashboard".into(), ..Default::default()
    });
    let signal_resolution = format!("{}x{}", screen_w, screen_h);
    let panel_resolution = edid_info.panel_res.clone();
    let idle_dashboard_codec = dashboard::configured_dashboard_codec();
    let idle_dashboard_enabled = config::env_bool_or("IDLE_DASHBOARD", true);
    let (idle_width, idle_height) = if idle_dashboard_codec == "raw" {
        dashboard::raw_dashboard_dimensions(screen_w, screen_h)
    } else {
        (screen_w, screen_h)
    };
    println!(
        "[IDLE DASHBOARD] Mode={} render={}x{} display={}x{}",
        idle_dashboard_codec, idle_width, idle_height, screen_w, screen_h
    );

    // 2. Start single persistent GStreamer pipeline
    let (playback_submission_tx, mut playback_submission_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut playback_engine = playback::start_persistent_playback(
        &idle_dashboard_codec,
        &connector_id,
        render_rect.as_deref(),
        "dashboard",
        idle_width,
        idle_height,
        playback_submission_tx,
    )?;

    let streaming_active = Arc::new(AtomicBool::new(false));
    let active_fps = Arc::new(AtomicU32::new(config::telemetry::DEFAULT_ACTIVE_FPS));
    let active_res = Arc::new(Mutex::new(config::telemetry::DEFAULT_ACTIVE_RESOLUTION.to_string()));
    let active_bitrate_mbps = Arc::new(Mutex::new(config::telemetry::DEFAULT_ACTIVE_BITRATE_MBPS));
    let active_latency_mode = Arc::new(Mutex::new(config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string()));
    let active_capture_resolution = Arc::new(Mutex::new(String::new()));
    let active_encoded_resolution = Arc::new(Mutex::new(String::new()));
    let active_aspect_mode = Arc::new(Mutex::new(String::new()));
    let active_content_rect = Arc::new(Mutex::new(String::new()));

    // 3. Background feeder thread for the idle clock/IP dashboard
    if idle_dashboard_enabled {
        dashboard::spawn_idle_dashboard_thread(
            idle_width,
            idle_height,
            vrefresh,
            Arc::clone(&streaming_active),
            playback_engine.dashboard_writer.clone(),
            pairing_state,
        );
    } else {
        println!("[IDLE DASHBOARD] Disabled by configuration");
    }

    println!(
        "[READY] Persistent GStreamer HDMI presenter running; waiting for UDP/WebTransport stream"
    );
    receiver_ready.store(true, Ordering::Release);
    let mut sent = 0u64;
    let idle_timeout_sec = config::env_or(
        "IDLE_TIMEOUT_SEC",
        config::server::DEFAULT_IDLE_TIMEOUT_SEC,
    );
    let idle_timeout = std::time::Duration::from_secs(idle_timeout_sec);
    let sender_liveness_timeout_sec = config::env_or(
        "SENDER_LIVENESS_TIMEOUT_SEC",
        config::server::DEFAULT_SENDER_LIVENESS_TIMEOUT_SEC,
    );
    let sender_liveness_timeout = std::time::Duration::from_secs(sender_liveness_timeout_sec);
    let mut media_stall_reported = false;
    let mut health_interval = tokio::time::interval(std::time::Duration::from_secs(2));
    health_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = health_interval.tick() => {
                let playback_state = match playback_engine.child.try_wait() {
                    Ok(Some(_)) => "error",
                    _ => match playback_engine.current_codec.as_str() {
                        "h264" => "h264",
                        "h265" => "h265",
                        _ => "idle_dashboard",
                    },
                };
                management.refresh_pipeline_health(playback_state, v4l2_decoder::reassembly_stats());
                management.refresh_system_health();
            }
            Some(submission) = playback_submission_rx.recv() => {
                if streaming_active.load(Ordering::Relaxed) {
                    let sample_interval = active_fps.load(Ordering::Relaxed).max(1);
                    if submission.seq == 1 || submission.seq % sample_interval == 0 {
                        control_channel.send_telemetry(control::TelemetryMessage::LatencySample {
                            seq: submission.seq,
                            capture_time_ms: submission.capture_time_ms,
                            encode_duration_ms: submission.encode_duration_ms,
                        });
                    }
                }
            }
            // High priority: Control socket JSON commands
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    control::ControlCommand::Stop => {
                        println!("[CONTROL WS] Received STOP command from independent control socket!");
                        management.stop("user_stop");
                        media_stall_reported = false;
                        streaming_active.store(false, Ordering::Relaxed);
                        if let Ok(mut l) = active_capture_resolution.lock() { l.clear(); }
                        if let Ok(mut l) = active_encoded_resolution.lock() { l.clear(); }
                        if let Ok(mut l) = active_aspect_mode.lock() { l.clear(); }
                        if let Ok(mut l) = active_content_rect.lock() { l.clear(); }
                        v4l2_decoder::reset_decoder_pipeline();
                        let _ = playback_engine.ensure_configuration(&idle_dashboard_codec, &connector_id, render_rect.as_deref(), "dashboard");
                        while rx.try_recv().is_ok() {}
                        let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(config::telemetry::DEFAULT_IDLE_BITRATE_MBPS);
                        let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string());
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: "IDLE".to_string(),
                            resolution: config::telemetry::DEFAULT_IDLE_RESOLUTION.to_string(),
                            fps: config::telemetry::DEFAULT_IDLE_FPS,
                            display_resolution: format!("{}x{}", screen_w, screen_h),
                            display_fps: vrefresh,
                            bitrate_mbps: bw,
                            latency_mode: lat_mode,
                            edid_name: edid_info.name.clone(),
                            edid_type: edid_info.conn_type.clone(),
                            edid_max_res: edid_info.max_res.clone(),
                            edid_max_fps: edid_info.max_fps,
                            display_max_fps: edid_info.max_fps,
                            capture_resolution: String::new(),
                            encoded_resolution: String::new(),
                            aspect_mode: String::new(),
                            content_rect: String::new(),
                            signal_resolution: signal_resolution.clone(),
                            panel_resolution: panel_resolution.clone(),
                        });
                    }
                    control::ControlCommand::AdminStop => {
                        println!("[ADMIN] Received administrative stop command");
                        management.stop("admin_stop");
                        media_stall_reported = false;
                        streaming_active.store(false, Ordering::Relaxed);
                        if let Ok(mut l) = active_capture_resolution.lock() { l.clear(); }
                        if let Ok(mut l) = active_encoded_resolution.lock() { l.clear(); }
                        if let Ok(mut l) = active_aspect_mode.lock() { l.clear(); }
                        if let Ok(mut l) = active_content_rect.lock() { l.clear(); }
                        v4l2_decoder::reset_decoder_pipeline();
                        let _ = playback_engine.ensure_configuration(&idle_dashboard_codec, &connector_id, render_rect.as_deref(), "dashboard");
                        while rx.try_recv().is_ok() {}
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: "IDLE".into(), resolution: config::telemetry::DEFAULT_IDLE_RESOLUTION.into(), fps: 0,
                            display_resolution: format!("{}x{}", screen_w, screen_h), display_fps: vrefresh,
                            bitrate_mbps: 0.0, latency_mode: String::new(), edid_name: edid_info.name.clone(), edid_type: edid_info.conn_type.clone(),
                            edid_max_res: edid_info.max_res.clone(), edid_max_fps: edid_info.max_fps, display_max_fps: edid_info.max_fps,
                            capture_resolution: String::new(), encoded_resolution: String::new(), aspect_mode: String::new(), content_rect: String::new(), signal_resolution: signal_resolution.clone(), panel_resolution: panel_resolution.clone(),
                        });
                    }
                    control::ControlCommand::RestartReceiver => {
                        println!("[ADMIN] Restart requested for a settings change");
                        management.stop("settings_restart");
                        return Ok(());
                    }
                    control::ControlCommand::Shutdown { reason } => {
                        println!("[WATCHDOG] Graceful receiver shutdown: {reason}");
                        management.stop(&reason);
                        return Ok(());
                    }
                    control::ControlCommand::ClientHello { device_id, user_agent, platform, language, page_session_id, connection_id, remote_ip } => {
                        management.hello(management::ClientMetadata { device_id, user_agent, platform, language, page_session_id, connection_id: connection_id.unwrap_or_else(|| "legacy".into()), remote_ip: remote_ip.unwrap_or_else(|| "unknown".into()) });
                    }
                    control::ControlCommand::Start { codec, resolution, fps, bitrate_mbps, latency_mode, aspect_mode, source_width, source_height, encoded_width, encoded_height, content_rect, signal_content_rect, panel_content_rect, signal_width, signal_height, panel_width, panel_height, connection_id, device_id } => {
                        let req_codec = codec.as_deref().unwrap_or(config::telemetry::DEFAULT_CODEC);
                        let requested_aspect_mode = aspect_mode.as_deref() == Some("stretch");
                        let aspect_mode = if requested_aspect_mode {
                            "stretch"
                        } else {
                            config::telemetry::DEFAULT_ASPECT_MODE
                        };
                        println!("[CONTROL WS] Received START command: codec={:?}, res={:?}, fps={:?}, bitrate={:?}, latency_mode={:?}, aspect_mode={:?}", req_codec, resolution, fps, bitrate_mbps, latency_mode, aspect_mode);
                        streaming_active.store(true, Ordering::Relaxed);
                        media_stall_reported = false;
                        let res_str = resolution.unwrap_or_else(|| config::telemetry::DEFAULT_ACTIVE_RESOLUTION.to_string());
                        let stream_fps = fps.unwrap_or(config::telemetry::DEFAULT_ACTIVE_FPS);
                        let bw = bitrate_mbps.unwrap_or(config::telemetry::DEFAULT_ACTIVE_BITRATE_MBPS);
                        let lat_mode = latency_mode.unwrap_or_else(|| config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string());
                        let capture_res = match (source_width, source_height) {
                            (Some(width), Some(height)) => format!("{width}x{height}"),
                            _ => String::new(),
                        };
                        let encoded_res = match (encoded_width, encoded_height) {
                            (Some(width), Some(height)) => format!("{width}x{height}"),
                            _ => res_str.clone(),
                        };
                        let sender = connection_id.as_ref().and_then(|id| management.snapshot().connections.into_iter().find(|c| &c.connection_id == id)).map(|c| management::ClientMetadata { device_id: if c.device_id.is_empty() { device_id.clone().unwrap_or_default() } else { c.device_id }, user_agent: c.user_agent, platform: c.platform, language: c.language, page_session_id: c.page_session_id, remote_ip: c.remote_ip, connection_id: c.connection_id });
                        management.start(management::StreamConfigSnapshot { codec: req_codec.to_string(), resolution: res_str.clone(), fps: stream_fps, bitrate_mbps: bw, latency_mode: lat_mode.clone(), aspect_mode: aspect_mode.to_string(), capture_resolution: capture_res.clone(), encoded_resolution: encoded_res.clone() }, sender);
                        let content_rect = content_rect.unwrap_or_default();
                        if let Ok(mut l) = active_capture_resolution.lock() { *l = capture_res.clone(); }
                        if let Ok(mut l) = active_encoded_resolution.lock() { *l = encoded_res.clone(); }
                        if let Ok(mut l) = active_aspect_mode.lock() { *l = aspect_mode.to_string(); }
                        if let Ok(mut l) = active_content_rect.lock() { *l = content_rect.clone(); }
                        if let (Some(visible_width), Some(visible_height), Some(encoded_width), Some(encoded_height)) = (source_width, source_height, encoded_width, encoded_height) {
                            println!("[SOURCE GEOMETRY] capture={}x{}, encoded={}x{}, content={}", visible_width, visible_height, encoded_width, encoded_height, content_rect);
                        }
                        if let (Some(signal_width), Some(signal_height), Some(panel_width), Some(panel_height)) = (signal_width, signal_height, panel_width, panel_height) {
                            println!("[DISPLAY GEOMETRY] signal={}x{}, panel={}x{}, signal_content={}, panel_content={}", signal_width, signal_height, panel_width, panel_height, signal_content_rect.as_deref().unwrap_or("<unknown>"), panel_content_rect.as_deref().unwrap_or("<unknown>"));
                        }
                        active_fps.store(stream_fps, Ordering::Relaxed);
                        if let Ok(mut l) = active_res.lock() { *l = res_str.clone(); }
                        if let Ok(mut l) = active_bitrate_mbps.lock() { *l = bw; }
                        if let Ok(mut l) = active_latency_mode.lock() { *l = lat_mode.clone(); }
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: "STREAMING".to_string(),
                            resolution: res_str,
                            fps: stream_fps,
                            display_resolution: format!("{}x{}", screen_w, screen_h),
                            display_fps: vrefresh,
                            bitrate_mbps: bw,
                            latency_mode: lat_mode,
                            edid_name: edid_info.name.clone(),
                            edid_type: edid_info.conn_type.clone(),
                            edid_max_res: edid_info.max_res.clone(),
                            edid_max_fps: edid_info.max_fps,
                            display_max_fps: edid_info.max_fps,
                            capture_resolution: capture_res,
                            encoded_resolution: encoded_res,
                            aspect_mode: aspect_mode.to_string(),
                            content_rect,
                            signal_resolution: signal_resolution.clone(),
                            panel_resolution: panel_resolution.clone(),
                        });
                    }
                    control::ControlCommand::Ping { id } => {
                        // WebTransport pings are answered directly on their
                        // originating control stream. This legacy command
                        // path remains for the older WebSocket endpoint.
                        control_channel.send_telemetry(control::TelemetryMessage::Pong { id });
                    }
                    control::ControlCommand::ClientDiagnostic { level, message } => {
                        let normalized_level = match level.to_ascii_lowercase().as_str() {
                            "error" => "error",
                            "warn" | "warning" => "warn",
                            "debug" => "debug",
                            _ => "info",
                        };
                        let bounded_message: String = message.chars().take(4096).collect();
                        if !bounded_message.is_empty() {
                            management.event(normalized_level, "client_diagnostic", format!("client=legacy: {bounded_message}"));
                        }
                    }
                    control::ControlCommand::LatencyReport { .. } => {
                        // Direct WebTransport reports are consumed at the
                        // authenticated connection boundary. Ignore any report
                        // reaching this legacy shared command path.
                    }
                    control::ControlCommand::GetStatus => {
                        let is_act = streaming_active.load(Ordering::Relaxed);
                        let state = if is_act { "STREAMING" } else { "IDLE" };
                        let cur_res = if is_act {
                            active_res.lock().map(|l| l.clone()).unwrap_or_else(|_| config::telemetry::DEFAULT_ACTIVE_RESOLUTION.to_string())
                        } else {
                            config::telemetry::DEFAULT_IDLE_RESOLUTION.to_string()
                        };
                        let cur_fps = if is_act { active_fps.load(Ordering::Relaxed) } else { 0 };
                        let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(config::telemetry::DEFAULT_IDLE_BITRATE_MBPS);
                        let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string());
                        let capture_res = active_capture_resolution.lock().map(|l| l.clone()).unwrap_or_default();
                        let encoded_res = active_encoded_resolution.lock().map(|l| l.clone()).unwrap_or_default();
                        let aspect_mode = active_aspect_mode.lock().map(|l| l.clone()).unwrap_or_default();
                        let content_rect = active_content_rect.lock().map(|l| l.clone()).unwrap_or_default();
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: state.to_string(),
                            resolution: cur_res,
                            fps: cur_fps,
                            display_resolution: format!("{}x{}", screen_w, screen_h),
                            display_fps: vrefresh,
                            bitrate_mbps: bw,
                            latency_mode: lat_mode,
                            edid_name: edid_info.name.clone(),
                            edid_type: edid_info.conn_type.clone(),
                            edid_max_res: edid_info.max_res.clone(),
                            edid_max_fps: edid_info.max_fps,
                            display_max_fps: edid_info.max_fps,
                            capture_resolution: capture_res,
                            encoded_resolution: encoded_res,
                            aspect_mode,
                            content_rect,
                            signal_resolution: signal_resolution.clone(),
                            panel_resolution: panel_resolution.clone(),
                        });
                    }
                }
            }

            // High throughput video frame processing
            recv_res = tokio::time::timeout(idle_timeout, rx.recv()) => {
                match recv_res {
                    Ok(Some(frame)) => {
                        if frame.codec == "stop" {
                            println!("[PLAYBACK] Received stop signal; restoring HDMI IP dashboard...");
                            management.stop("user_stop");
                            streaming_active.store(false, Ordering::Relaxed);
                            if let Ok(mut l) = active_capture_resolution.lock() { l.clear(); }
                            if let Ok(mut l) = active_encoded_resolution.lock() { l.clear(); }
                            if let Ok(mut l) = active_aspect_mode.lock() { l.clear(); }
                            if let Ok(mut l) = active_content_rect.lock() { l.clear(); }
                            v4l2_decoder::reset_decoder_pipeline();
                            let _ = playback_engine.ensure_configuration(&idle_dashboard_codec, &connector_id, render_rect.as_deref(), "dashboard");
                            while rx.try_recv().is_ok() {}
                            let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(config::telemetry::DEFAULT_IDLE_BITRATE_MBPS);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string());
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "IDLE".to_string(),
                                resolution: config::telemetry::DEFAULT_IDLE_RESOLUTION.to_string(),
                                fps: 0,
                                display_resolution: format!("{}x{}", screen_w, screen_h),
                                display_fps: vrefresh,
                                bitrate_mbps: bw,
                                latency_mode: lat_mode,
                                edid_name: edid_info.name.clone(),
                                edid_type: edid_info.conn_type.clone(),
                                 edid_max_res: edid_info.max_res.clone(),
                                 edid_max_fps: edid_info.max_fps,
                                 display_max_fps: edid_info.max_fps,
                                 capture_resolution: String::new(),
                                 encoded_resolution: String::new(),
                                 aspect_mode: String::new(),
                                 content_rect: String::new(),
                                 signal_resolution: signal_resolution.clone(),
                                 panel_resolution: panel_resolution.clone(),
                             });
                            continue;
                        }
                        if frame.codec != "hevc" && frame.codec != "h264" { continue; }

                        // Do not let trailing packets after an explicit stop switch the
                        // dashboard pipeline back into composed-stream mode.
                        if !streaming_active.load(Ordering::Relaxed) && frame.seq > 1 {
                            continue;
                        }

                        // Stop the idle feeder before replacing its raw KMS pipeline,
                        // otherwise a dashboard frame can block the stream handoff.
                        if frame.seq <= 1 {
                            streaming_active.store(true, Ordering::Relaxed);
                        }

                        // The client has already composed the encoded frame, including any
                        // preserve-mode bars. KMS only scales that frame across the display.
                        let _ = playback_engine.ensure_configuration(
                            &frame.codec,
                            &connector_id,
                            render_rect.as_deref(),
                            "composed",
                        );

                        // Allow auto-start if a new sequence frame (seq <= 1) arrives
                        if frame.seq <= 1 {
                            let was_active = streaming_active.swap(true, Ordering::Relaxed);
                            let frame_res = format!("{}x{}", frame.width, frame.height);
                            if let Ok(mut l) = active_res.lock() { *l = frame_res.clone(); }
                            let cur_fps = active_fps.load(Ordering::Relaxed);
                            let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(config::telemetry::DEFAULT_IDLE_BITRATE_MBPS);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string());
                            if !was_active {
                                control_channel.send_telemetry(control::TelemetryMessage::Status {
                                    state: "STREAMING".to_string(),
                                    resolution: frame_res,
                                    fps: cur_fps,
                                    display_resolution: format!("{}x{}", screen_w, screen_h),
                                    display_fps: vrefresh,
                                    bitrate_mbps: bw,
                                    latency_mode: lat_mode,
                                    edid_name: edid_info.name.clone(),
                                    edid_type: edid_info.conn_type.clone(),
                                     edid_max_res: edid_info.max_res.clone(),
                                     edid_max_fps: edid_info.max_fps,
                                     display_max_fps: edid_info.max_fps,
                                     capture_resolution: String::new(),
                                     encoded_resolution: String::new(),
                                     aspect_mode: String::new(),
                                     content_rect: String::new(),
                                      signal_resolution: signal_resolution.clone(),
                                      panel_resolution: panel_resolution.clone(),
                                  });
                            }
                        }

                        // If streaming was explicitly stopped, discard trailing/out-of-order frames
                        if !streaming_active.load(Ordering::Relaxed) {
                            continue;
                        }

                        let frame_seq = frame.seq;
                        let frame_bytes = frame.access_unit.len();
                        let timing = match (frame.capture_time_ms, frame.encode_duration_ms) {
                            (Some(capture_time_ms), Some(encode_duration_ms)) => Some(playback::PlaybackTiming {
                                seq: frame_seq,
                                capture_time_ms,
                                encode_duration_ms,
                            }),
                            _ => None,
                        };
                        if playback_engine.writer_tx.send(playback::PlaybackBuffer { bytes: frame.access_unit, timing }).is_err() {
                            eprintln!("[PLAYBACK ERROR] seq={} pipe write failed", frame.seq);
                            continue;
                        }

                        let latency_ms = frame.first_packet_at.elapsed().as_secs_f32() * 1000.0;
                        sent += 1;
                        media_stall_reported = false;
                        management.record_frame(frame_seq, frame_bytes, latency_ms as f64);
                        if sent == 1 || sent % 30 == 0 {
                            if config::codec_diagnostics_enabled() {
                                println!("[PLAYBACK] submitted_{}_access_units={sent} (latency={latency_ms:.1}ms)", frame.codec);
                            }
                            let frame_res = format!("{}x{}", frame.width, frame.height);
                            let cur_fps = active_fps.load(Ordering::Relaxed);
                            let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(config::telemetry::DEFAULT_IDLE_BITRATE_MBPS);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string());
                                control_channel.send_telemetry(control::TelemetryMessage::Status {
                                    state: "STREAMING".to_string(),
                                    resolution: frame_res,
                                    fps: cur_fps,
                                    display_resolution: format!("{}x{}", screen_w, screen_h),
                                    display_fps: vrefresh,
                                    bitrate_mbps: bw,
                                    latency_mode: lat_mode,
                                    edid_name: edid_info.name.clone(),
                                    edid_type: edid_info.conn_type.clone(),
                                     edid_max_res: edid_info.max_res.clone(),
                                     edid_max_fps: edid_info.max_fps,
                                     display_max_fps: edid_info.max_fps,
                                     capture_resolution: String::new(),
                                     encoded_resolution: String::new(),
                                     aspect_mode: String::new(),
                                     content_rect: String::new(),
                                      signal_resolution: signal_resolution.clone(),
                                      panel_resolution: panel_resolution.clone(),
                                   });
                        }
                    }
                    Ok(None) => break, // Channel closed
                    Err(_) => {
                        // Timeout waiting for frames (30s idle)
                        if streaming_active.load(Ordering::Relaxed) {
                            if management.active_sender_heartbeat_fresh(sender_liveness_timeout) {
                                if !media_stall_reported {
                                    println!("[PLAYBACK] No media frames for {}s; active sender heartbeat is fresh, keeping the last frame", idle_timeout_sec);
                                    management.event(
                                        "warn",
                                        "media_stalled_sender_alive",
                                        format!("no frames for {idle_timeout_sec}s while sender heartbeat remained fresh"),
                                    );
                                    media_stall_reported = true;
                                }
                                continue;
                            }
                            println!("[PLAYBACK] Stream idle timeout; restoring HDMI IP dashboard...");
                            management.stop("idle_timeout");
                            streaming_active.store(false, Ordering::Relaxed);
                            media_stall_reported = false;
                            if let Ok(mut l) = active_capture_resolution.lock() { l.clear(); }
                            if let Ok(mut l) = active_encoded_resolution.lock() { l.clear(); }
                            if let Ok(mut l) = active_aspect_mode.lock() { l.clear(); }
                             if let Ok(mut l) = active_content_rect.lock() { l.clear(); }
                            let _ = playback_engine.ensure_configuration(&idle_dashboard_codec, &connector_id, render_rect.as_deref(), "dashboard");
                            while rx.try_recv().is_ok() {}
                        let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(config::telemetry::DEFAULT_IDLE_BITRATE_MBPS);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| config::telemetry::DEFAULT_ACTIVE_LATENCY_MODE.to_string());
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "IDLE".to_string(),
                                resolution: config::telemetry::DEFAULT_IDLE_RESOLUTION.to_string(),
                                fps: 0,
                                display_resolution: format!("{}x{}", screen_w, screen_h),
                                display_fps: vrefresh,
                                bitrate_mbps: bw,
                                latency_mode: lat_mode,
                                edid_name: edid_info.name.clone(),
                                edid_type: edid_info.conn_type.clone(),
                                edid_max_res: edid_info.max_res.clone(),
                                edid_max_fps: edid_info.max_fps,
                                display_max_fps: edid_info.max_fps,
                                capture_resolution: String::new(),
                                encoded_resolution: String::new(),
                                aspect_mode: String::new(),
                                content_rect: String::new(),
                                 signal_resolution: signal_resolution.clone(),
                                 panel_resolution: panel_resolution.clone(),
                             });
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
