//! Verified RK3399 fallback: UDP HEVC -> V4L2 stateless decoder -> KMS.
//! The atomic two-plane presenter is developed separately; this keeps HDMI
//! playback on the proven pipeline while it is completed.
mod cert;
mod control;
mod drm_kms;
mod gfx;
mod http_server;
mod net;
mod text;
mod v4l2_decoder;
mod webtransport_server;

use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Stdio};
use tokio::sync::mpsc;
fn elevate_process_priority() {
    unsafe {
        let param = libc::sched_param { sched_priority: 20 };
        if libc::sched_setscheduler(0, libc::SCHED_FIFO, &param as *const _) == 0 {
            println!("[PRIORITY] Successfully elevated main process to SCHED_FIFO priority 20");
        } else {
            let err = *libc::__errno_location();
            println!("[PRIORITY] SCHED_FIFO elevation not permitted (errno={err}); setting niceness to -10...");
            libc::setpriority(libc::PRIO_PROCESS, 0, -10);
        }
    }
}

fn spawn_dmesg_kernel_monitor() {
    elevate_process_priority();
    std::thread::spawn(|| {
        if let Ok(mut child) = Command::new("dmesg").args(["-w"]).stdout(Stdio::piped()).spawn() {
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines().flatten() {
                    let lower = line.to_lowercase();
                    if lower.contains("rkvdec") || lower.contains("v4l2") || lower.contains("rockchip-drm") {
                        if lower.contains("error") || lower.contains("fault") || lower.contains("failed") || lower.contains("corrupt") || lower.contains("warn") {
                            println!("[LAYER 1 ALERT] Kernel Driver Event: {}", line);
                        }
                    }
                }
            }
        }
    });
}

struct PlaybackEngine {
    child: Child,
    writer_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
    codec: String,
}

fn stop_playback(playback: &mut Option<PlaybackEngine>) {
    if let Some(mut engine) = playback.take() {
        println!("[PLAYBACK STOPPING] Terminating previous {} GStreamer child process...", engine.codec);
        let _ = engine.child.kill();
        let _ = engine.child.wait();
    }
}

fn start_playback(codec: &str) -> Result<PlaybackEngine, Box<dyn std::error::Error>> {
    let (connector, render_rect) = match std::env::var("DRM_CONNECTOR_ID") {
        Ok(val) if !val.trim().is_empty() && val.trim() != "auto" => (val, None),
        _ => {
            if let Ok(card) = drm_kms::open_display_card() {
                if let Ok((screen_w, screen_h, _, conn_handle, _)) = drm_kms::autodetect_display_mode(&card) {
                    let id = u32::from(conn_handle).to_string();
                    println!("[PLAYBACK] Auto-detected active HDMI connector ID: {}", id);

                    let target_w = screen_w.min(screen_h * 16 / 9);
                    let target_h = screen_h.min(screen_w * 9 / 16);
                    let offset_x = (screen_w - target_w) / 2;
                    let offset_y = (screen_h - target_h) / 2;

                    let rect = format!("<{},{},{},{}>", offset_x, offset_y, target_w, target_h);
                    println!("[PLAYBACK] Display CRTC {}x{} | 16:9 render-rectangle={}", screen_w, screen_h, rect);
                    (id, Some(rect))
                } else {
                    ("54".into(), None)
                }
            } else {
                ("54".into(), None)
            }
        }
    };
    let plane = std::env::var("DRM_PLANE_ID").unwrap_or_else(|_| "33".into());
    let codec_lower = codec.to_lowercase();
    let (parser, decoder) = if codec_lower == "h264" {
        ("h264parse", "v4l2slh264dec")
    } else {
        ("h265parse", "v4l2slh265dec")
    };

    let mut gst_args = vec![
        "-q".to_string(), "fdsrc".to_string(), "fd=0".to_string(), "do-timestamp=true".to_string(), "!".to_string(),
        parser.to_string(), "config-interval=-1".to_string(), "!".to_string(),
        decoder.to_string(), "!".to_string(),
        "kmssink".to_string(), "driver-name=rockchip".to_string(),
        format!("connector-id={connector}"), format!("plane-id={plane}"),
    ];
    if let Some(rect) = render_rect {
        gst_args.push(format!("render-rectangle={rect}"));
    }
    gst_args.extend([
        "force-modesetting=false".to_string(), "can-scale=true".to_string(),
        "sync=false".to_string(), "skip-vsync=true".to_string(), "max-lateness=0".to_string(),
    ]);

    let mut child = Command::new("gst-launch-1.0")
        .env("GST_DEBUG", "v4l2slh265dec:4,h265parse:4,v4l2slh264dec:4,h264parse:4,kmssink:4")
        .args(&gst_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or("could not open GStreamer stdin")?;
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().flatten() {
                let lower = line.to_lowercase();
                if lower.contains("error") || lower.contains("warn") || lower.contains("corrupt") || lower.contains("missing") || lower.contains("drop") {
                    println!("[LAYER 2 ALERT] GStreamer Decoder Event: {}", line);
                } else if lower.contains("resolution changed") || lower.contains("colorimetry") {
                    println!("[DECODER CAPS CHANGE] GStreamer: {}", line);
                }
            }
        });
    }

    let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
    std::thread::spawn(move || {
        while let Ok(access_unit) = writer_rx.recv() {
            if stdin.write_all(&access_unit).and_then(|_| stdin.flush()).is_err() {
                break;
            }
        }
    });

    println!("[PLAYBACK READY] {codec} -> {parser} -> {decoder} -> HDMI connector {connector}, plane {plane}");
    Ok(PlaybackEngine {
        child,
        writer_tx,
        codec: codec_lower,
    })
}

/// Holds the DRM file and scanout allocation while the receiver is idle. It
/// must be dropped before gst-launch starts so the playback process can become
/// DRM master and take over the same HDMI plane.
struct IdleDashboard {
    _card: drm_kms::Card,
    _fb: drm::control::framebuffer::Handle,
    _prime_fd: i32,
    _stop_tx: Option<std::sync::mpsc::Sender<()>>,
    _thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl IdleDashboard {
    fn release(mut self) {
        if let Some(tx) = self._stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self._thread_handle.take() {
            let _ = handle.join();
        }
        drm_kms::drop_master(&self._card);
        println!("[IDLE DASHBOARD] released DRM master for video playback.");
    }
}

impl Drop for IdleDashboard {
    fn drop(&mut self) {
        if let Some(tx) = self._stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self._thread_handle.take() {
            let _ = handle.join();
        }
    }
}

fn show_idle_dashboard() -> Result<IdleDashboard, Box<dyn std::error::Error>> {
    let card = drm_kms::open_display_card()?;
    let (width, height, mode, connector, crtc) = drm_kms::autodetect_display_mode(&card)?;
    let (prime_fd, pitch, size, ptr) = drm_kms::allocate_prime_dmabuf(card.0.as_raw_fd(), width, height)?;
    let fb = drm_kms::import_dmabuf_and_add_fb(card.0.as_raw_fd(), prime_fd, width, height, pitch, drm_kms::DRM_FORMAT_XRGB8888)?;
    let ips = net::get_active_ipv4_addresses();
    unsafe {
        let pixels = std::slice::from_raw_parts_mut(ptr as *mut u32, size / 4);
        text::draw_ip_dashboard_argb(pixels, width, height, mode.vrefresh(), &ips);
    }
    drm_kms::set_display_mode(&card, crtc, fb, connector, mode)?;
    println!("[IDLE DASHBOARD] HDMI IP screen active; waiting for HEVC stream.");

    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let ptr_addr = ptr as usize;

    let thread_handle = std::thread::spawn(move || {
        let mut last_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        loop {
            match stop_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            }
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now_secs != last_secs {
                last_secs = now_secs;
                unsafe {
                    let pixels = std::slice::from_raw_parts_mut(ptr_addr as *mut u32, size / 4);
                    text::update_clock_argb(pixels, width, height);
                }
            }
        }
    });

    Ok(IdleDashboard {
        _card: card,
        _fb: fb,
        _prime_fd: prime_fd,
        _stop_tx: Some(stop_tx),
        _thread_handle: Some(thread_handle),
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    spawn_dmesg_kernel_monitor();
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
    let mut dashboard = if std::env::var("IDLE_DASHBOARD").map_or(true, |v| v != "0") {
        Some(show_idle_dashboard()?)
    } else { None };
    println!("[READY] waiting for video stream access units on UDP/WebTransport");
    let mut playback: Option<PlaybackEngine> = None;
    let mut sent = 0u64;
    let mut streaming_enabled = false;
    let idle_timeout = std::time::Duration::from_millis(30000);

    loop {
        tokio::select! {
            // High priority: Control socket JSON commands
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    control::ControlCommand::Stop => {
                        println!("[CONTROL WS] Received STOP command from independent control socket!");
                        streaming_enabled = false;
                        stop_playback(&mut playback);
                        while rx.try_recv().is_ok() {}
                        if dashboard.is_none() && std::env::var("IDLE_DASHBOARD").map_or(true, |v| v != "0") {
                            if let Ok(d) = show_idle_dashboard() {
                                dashboard = Some(d);
                            }
                        }
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: "IDLE".to_string(),
                            resolution: "0x0".to_string(),
                            fps: 0,
                            delivery_rate: 100.0,
                            frames_submitted: sent,
                            latency_ms: 0.0,
                        });
                    }
                    control::ControlCommand::Start { codec, resolution } => {
                        println!("[CONTROL WS] Received START command: codec={:?}, res={:?}", codec, resolution);
                        streaming_enabled = true;
                    }
                    control::ControlCommand::Ping => {
                        control_channel.send_telemetry(control::TelemetryMessage::Pong);
                    }
                    control::ControlCommand::GetStatus => {
                        let state = if playback.is_some() { "STREAMING" } else { "IDLE" };
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: state.to_string(),
                            resolution: "1920x1080".to_string(),
                            fps: 60,
                            delivery_rate: 100.0,
                            frames_submitted: sent,
                            latency_ms: 0.0,
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
                            streaming_enabled = false;
                            stop_playback(&mut playback);
                            while rx.try_recv().is_ok() {}
                            if dashboard.is_none() && std::env::var("IDLE_DASHBOARD").map_or(true, |v| v != "0") {
                                if let Ok(d) = show_idle_dashboard() {
                                    dashboard = Some(d);
                                }
                            }
                            continue;
                        }
                        if frame.codec != "hevc" && frame.codec != "h264" { continue; }

                        // Allow auto-start if a new sequence frame (seq <= 1) arrives
                        if frame.seq <= 1 {
                            streaming_enabled = true;
                        }

                        // If streaming was explicitly stopped, discard trailing/out-of-order frames
                        if !streaming_enabled {
                            continue;
                        }

                        let need_restart = match &playback {
                            Some(engine) => engine.codec != frame.codec.to_lowercase(),
                            None => true,
                        };

                        if need_restart {
                            stop_playback(&mut playback);
                            // Release the dashboard's DRM master immediately before hand-off.
                            if let Some(dashboard) = dashboard.take() { dashboard.release(); }
                            playback = Some(start_playback(&frame.codec)?);
                        }

                        if let Some(engine) = playback.as_ref() {
                            if engine.writer_tx.send(frame.access_unit).is_err() {
                                eprintln!("[PLAYBACK ERROR] seq={} pipe closed; restarting pipeline", frame.seq);
                                stop_playback(&mut playback);
                                continue;
                            }
                        }
                        sent += 1;
                        let latency_ms = frame.first_packet_at.elapsed().as_secs_f32() * 1000.0;
                        if sent == 1 || sent % 30 == 0 {
                            println!("[PLAYBACK] submitted_{}_access_units={sent} (latency={latency_ms:.1}ms)", frame.codec);
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "STREAMING".to_string(),
                                resolution: format!("{}x{}", frame.width, frame.height),
                                fps: 30,
                                delivery_rate: 100.0,
                                frames_submitted: sent,
                                latency_ms,
                            });
                        }
                    }
                    Ok(None) => break, // Channel closed
                    Err(_) => {
                        // Timeout waiting for frames (1.5s idle)
                        if playback.is_some() {
                            println!("[PLAYBACK] Stream idle timeout; restoring HDMI IP dashboard...");
                            streaming_enabled = false;
                            stop_playback(&mut playback);
                            while rx.try_recv().is_ok() {}
                            if dashboard.is_none() && std::env::var("IDLE_DASHBOARD").map_or(true, |v| v != "0") {
                                if let Ok(d) = show_idle_dashboard() {
                                    dashboard = Some(d);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
