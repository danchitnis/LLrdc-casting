//! Verified RK3399 fallback: UDP HEVC -> V4L2 stateless decoder -> KMS.
//! The atomic two-plane presenter is developed separately; this keeps HDMI
//! playback on the proven pipeline while it is completed.
mod cert;
mod control;
mod dashboard;
mod drm_kms;
mod http_server;
mod net;
mod playback;
mod sys_monitor;
mod text;
mod v4l2_decoder;
mod webtransport_server;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    sys_monitor::spawn_dmesg_kernel_monitor();
    let _ = drm_kms::inspect_live_scanout_status();

    let (tx, mut rx) = mpsc::channel::<v4l2_decoder::VideoFrame>(64);
    let (cmd_tx, mut cmd_rx) = mpsc::channel::<control::ControlCommand>(32);
    let control_channel = control::ControlChannel::new(cmd_tx);

    let identity = webtransport_server::get_or_create_identity().await.map_err(|e| e as Box<dyn std::error::Error>)?;
    let cert_hash_hex = webtransport_server::extract_cert_hash_hex(&identity);

    let http_cert_hash = cert_hash_hex.clone();
    let http_control_channel = control_channel.clone();
    tokio::spawn(async move {
        if let Err(error) = http_server::run_server(http_cert_hash, http_control_channel).await {
            eprintln!("[HTTP SERVER ERROR] {error}");
        }
    });

    tokio::spawn(async move {
        if let Err(error) = webtransport_server::run_server_with_identity(identity, tx).await {
            eprintln!("[SERVER ERROR] {error}");
        }
    });

    // 1. Auto-detect display geometry once before starting GStreamer
    let (screen_w, screen_h, vrefresh, connector_id, render_rect) = playback::autodetect_display_info();

    // 2. Start single persistent GStreamer pipeline
    let playback_engine = playback::start_persistent_playback("hevc", &connector_id, render_rect.as_deref())?;

    let streaming_active = Arc::new(AtomicBool::new(false));
    let active_fps = Arc::new(AtomicU32::new(30));
    let active_res = Arc::new(Mutex::new("1920x1080".to_string()));
    let active_bitrate_mbps = Arc::new(Mutex::new(10.0f32));
    let active_latency_mode = Arc::new(Mutex::new("ULL".to_string()));

    // 3. Background feeder thread for native HEVC clock/IP dashboard
    dashboard::spawn_idle_dashboard_thread(
        screen_w,
        screen_h,
        vrefresh,
        Arc::clone(&streaming_active),
        playback_engine.writer_tx.clone(),
    );

    println!("[READY] Persistent GStreamer HDMI presenter running; waiting for UDP/WebTransport HEVC stream");
    let mut sent = 0u64;
    let idle_timeout_sec: u64 = std::env::var("IDLE_TIMEOUT_SEC")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(30);
    let idle_timeout = std::time::Duration::from_secs(idle_timeout_sec);

    loop {
        tokio::select! {
            // High priority: Control socket JSON commands
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    control::ControlCommand::Stop => {
                        println!("[CONTROL WS] Received STOP command from independent control socket!");
                        streaming_active.store(false, Ordering::Relaxed);
                        v4l2_decoder::reset_decoder_pipeline();
                        while rx.try_recv().is_ok() {}
                        let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(0.0);
                        let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| "ULL".to_string());
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: "IDLE".to_string(),
                            resolution: "0x0".to_string(),
                            fps: 0,
                            delivery_rate: 100.0,
                            frames_submitted: sent,
                            latency_ms: 0.0,
                            display_resolution: format!("{}x{}", screen_w, screen_h),
                            display_fps: vrefresh,
                            bitrate_mbps: bw,
                            latency_mode: lat_mode,
                        });
                    }
                    control::ControlCommand::Start { codec, resolution, fps, bitrate_mbps, latency_mode } => {
                        println!("[CONTROL WS] Received START command: codec={:?}, res={:?}, fps={:?}, bitrate={:?}, latency_mode={:?}", codec, resolution, fps, bitrate_mbps, latency_mode);
                        streaming_active.store(true, Ordering::Relaxed);
                        let res_str = resolution.unwrap_or_else(|| "1920x1080".to_string());
                        let stream_fps = fps.unwrap_or(30);
                        let bw = bitrate_mbps.unwrap_or(10.0);
                        let lat_mode = latency_mode.unwrap_or_else(|| "ULL".to_string());
                        active_fps.store(stream_fps, Ordering::Relaxed);
                        if let Ok(mut l) = active_res.lock() { *l = res_str.clone(); }
                        if let Ok(mut l) = active_bitrate_mbps.lock() { *l = bw; }
                        if let Ok(mut l) = active_latency_mode.lock() { *l = lat_mode.clone(); }
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: "STREAMING".to_string(),
                            resolution: res_str,
                            fps: stream_fps,
                            delivery_rate: 100.0,
                            frames_submitted: sent,
                            latency_ms: 0.0,
                            display_resolution: format!("{}x{}", screen_w, screen_h),
                            display_fps: vrefresh,
                            bitrate_mbps: bw,
                            latency_mode: lat_mode,
                        });
                    }
                    control::ControlCommand::Ping => {
                        control_channel.send_telemetry(control::TelemetryMessage::Pong);
                    }
                    control::ControlCommand::GetStatus => {
                        let is_act = streaming_active.load(Ordering::Relaxed);
                        let state = if is_act { "STREAMING" } else { "IDLE" };
                        let cur_res = if is_act {
                            active_res.lock().map(|l| l.clone()).unwrap_or_else(|_| "1920x1080".to_string())
                        } else {
                            "0x0".to_string()
                        };
                        let cur_fps = if is_act { active_fps.load(Ordering::Relaxed) } else { 0 };
                        let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(0.0);
                        let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| "ULL".to_string());
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: state.to_string(),
                            resolution: cur_res,
                            fps: cur_fps,
                            delivery_rate: 100.0,
                            frames_submitted: sent,
                            latency_ms: 0.0,
                            display_resolution: format!("{}x{}", screen_w, screen_h),
                            display_fps: vrefresh,
                            bitrate_mbps: bw,
                            latency_mode: lat_mode,
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
                            streaming_active.store(false, Ordering::Relaxed);
                            v4l2_decoder::reset_decoder_pipeline();
                            while rx.try_recv().is_ok() {}
                            let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(0.0);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| "ULL".to_string());
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "IDLE".to_string(),
                                resolution: "0x0".to_string(),
                                fps: 0,
                                delivery_rate: 100.0,
                                frames_submitted: sent,
                                latency_ms: 0.0,
                                display_resolution: format!("{}x{}", screen_w, screen_h),
                                display_fps: vrefresh,
                                bitrate_mbps: bw,
                                latency_mode: lat_mode,
                            });
                            continue;
                        }
                        if frame.codec != "hevc" && frame.codec != "h264" { continue; }

                        // Allow auto-start if a new sequence frame (seq <= 1) arrives
                        if frame.seq <= 1 {
                            let was_active = streaming_active.swap(true, Ordering::Relaxed);
                            let frame_res = format!("{}x{}", frame.width, frame.height);
                            if let Ok(mut l) = active_res.lock() { *l = frame_res.clone(); }
                            let cur_fps = active_fps.load(Ordering::Relaxed);
                            let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(0.0);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| "ULL".to_string());
                            if !was_active {
                                control_channel.send_telemetry(control::TelemetryMessage::Status {
                                    state: "STREAMING".to_string(),
                                    resolution: frame_res,
                                    fps: cur_fps,
                                    delivery_rate: 100.0,
                                    frames_submitted: sent,
                                    latency_ms: 0.0,
                                    display_resolution: format!("{}x{}", screen_w, screen_h),
                                    display_fps: vrefresh,
                                    bitrate_mbps: bw,
                                    latency_mode: lat_mode,
                                });
                            }
                        }

                        // If streaming was explicitly stopped, discard trailing/out-of-order frames
                        if !streaming_active.load(Ordering::Relaxed) {
                            continue;
                        }

                        if playback_engine.writer_tx.send(frame.access_unit).is_err() {
                            eprintln!("[PLAYBACK ERROR] seq={} pipe write failed", frame.seq);
                            continue;
                        }

                        sent += 1;
                        let latency_ms = frame.first_packet_at.elapsed().as_secs_f32() * 1000.0;
                        if sent == 1 || sent % 30 == 0 {
                            println!("[PLAYBACK] submitted_{}_access_units={sent} (latency={latency_ms:.1}ms)", frame.codec);
                            let frame_res = format!("{}x{}", frame.width, frame.height);
                            let cur_fps = active_fps.load(Ordering::Relaxed);
                            let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(0.0);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| "ULL".to_string());
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "STREAMING".to_string(),
                                resolution: frame_res,
                                fps: cur_fps,
                                delivery_rate: 100.0,
                                frames_submitted: sent,
                                latency_ms,
                                display_resolution: format!("{}x{}", screen_w, screen_h),
                                display_fps: vrefresh,
                                bitrate_mbps: bw,
                                latency_mode: lat_mode,
                            });
                        }
                    }
                    Ok(None) => break, // Channel closed
                    Err(_) => {
                        // Timeout waiting for frames (30s idle)
                        if streaming_active.load(Ordering::Relaxed) {
                            println!("[PLAYBACK] Stream idle timeout; restoring HDMI IP dashboard...");
                            streaming_active.store(false, Ordering::Relaxed);
                            while rx.try_recv().is_ok() {}
                            let bw = active_bitrate_mbps.lock().map(|l| *l).unwrap_or(0.0);
                            let lat_mode = active_latency_mode.lock().map(|l| l.clone()).unwrap_or_else(|_| "ULL".to_string());
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "IDLE".to_string(),
                                resolution: "0x0".to_string(),
                                fps: 0,
                                delivery_rate: 100.0,
                                frames_submitted: sent,
                                latency_ms: 0.0,
                                display_resolution: format!("{}x{}", screen_w, screen_h),
                                display_fps: vrefresh,
                                bitrate_mbps: bw,
                                latency_mode: lat_mode,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
