//! Verified RK3399 fallback: UDP HEVC -> V4L2 stateless decoder -> KMS.
//! The atomic two-plane presenter is developed separately; this keeps HDMI
//! playback on the proven pipeline while it is completed.
mod cert;
mod control;
mod drm_kms;
mod http_server;
mod net;
mod text;
mod v4l2_decoder;
mod webtransport_server;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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
    _child: Child,
    writer_tx: std::sync::mpsc::SyncSender<Vec<u8>>,
}

fn autodetect_display_info() -> (u32, u32, u32, String, Option<String>) {
    if let Ok(card) = drm_kms::open_display_card() {
        if let Ok((screen_w, screen_h, mode, conn_handle, _)) = drm_kms::autodetect_display_mode(&card) {
            let conn_id = u32::from(conn_handle).to_string();
            let target_w = screen_w.min(screen_h * 16 / 9);
            let target_h = screen_h.min(screen_w * 9 / 16);
            let offset_x = (screen_w - target_w) / 2;
            let offset_y = (screen_h - target_h) / 2;
            let rect = format!("<{},{},{},{}>", offset_x, offset_y, target_w, target_h);
            let refresh = mode.vrefresh() as u32;
            drm_kms::drop_master(&card);
            drop(card);
            println!("[DISPLAY INFO] Auto-detected HDMI Connector {}, {}x{}@{}Hz, rect={}", conn_id, screen_w, screen_h, refresh, rect);
            return (screen_w, screen_h, refresh, conn_id, Some(rect));
        }
        drm_kms::drop_master(&card);
    }
    (1920, 1080, 60, "54".into(), None)
}

struct PersistentDashboardEncoder {
    child: Child,
    stdin: std::process::ChildStdin,
    width: u32,
    height: u32,
}

impl PersistentDashboardEncoder {
    fn spawn(width: u32, height: u32, writer_tx: std::sync::mpsc::SyncSender<Vec<u8>>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut child = Command::new("ffmpeg")
            .args([
                "-y",
                "-r", "1",
                "-f", "rawvideo",
                "-pixel_format", "bgra",
                "-video_size", &format!("{}x{}", width, height),
                "-i", "-",
                "-c:v", "libx265",
                "-preset", "ultrafast",
                "-tune", "zerolatency",
                "-flush_packets", "1",
                "-x265-params", "keyint=3600:bframes=0:no-scenecut=1",
                "-pix_fmt", "yuv420p",
                "-f", "hevc",
                "-",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().ok_or("ffmpeg stdin failed")?;
        let mut stdout = child.stdout.take().ok_or("ffmpeg stdout failed")?;

        // Parallel reader thread: forwards stdout stream chunks directly to GStreamer writer_tx
        std::thread::spawn(move || {
            let mut buf = [0u8; 131072];
            use std::io::Read;
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 { break; }
                if writer_tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            width,
            height,
        })
    }

    fn push_frame(&mut self, vrefresh: u32) {
        let ips = net::get_active_ipv4_addresses();
        let mut pixels = vec![0u32; (self.width * self.height) as usize];
        text::draw_ip_dashboard_argb(&mut pixels, self.width, self.height, vrefresh, &ips);

        let raw_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(pixels.as_ptr() as *const u8, pixels.len() * 4)
        };

        if self.stdin.write_all(raw_bytes).and_then(|_| self.stdin.flush()).is_err() {
            eprintln!("[IDLE ENCODER] Pipe write to ffmpeg stdin failed");
        }
    }
}

impl Drop for PersistentDashboardEncoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_persistent_playback(codec: &str, connector: &str, render_rect: Option<&str>) -> Result<PlaybackEngine, Box<dyn std::error::Error>> {
    let t_start = std::time::Instant::now();
    println!("[PLAYBACK STARTUP] Initializing persistent GStreamer pipeline at t=0ms");
    let plane = std::env::var("DRM_PLANE_ID").unwrap_or_else(|_| "33".into());
    let codec_lower = codec.to_lowercase();
    let (parser, decoder) = if codec_lower == "h264" {
        ("h264parse", "v4l2slh264dec")
    } else {
        ("h265parse", "v4l2slh265dec")
    };

    let mut gst_args = vec![
        "-q".to_string(), "fdsrc".to_string(), "fd=0".to_string(), "do-timestamp=true".to_string(), "blocksize=65536".to_string(), "!".to_string(),
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
        "sync=false".to_string(), "async=false".to_string(), "skip-vsync=true".to_string(), "max-lateness=0".to_string(),
    ]);

    println!("[PLAYBACK STARTUP] Spawning continuous gst-launch-1.0 process...");
    let mut child = Command::new("gst-launch-1.0")
        .env("GST_DEBUG", "v4l2slh265dec:4,h265parse:4,v4l2slh264dec:4,h264parse:4,kmssink:4")
        .args(&gst_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or("could not open GStreamer stdin")?;
    if let Some(stderr) = child.stderr.take() {
        let t_stderr = t_start;
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().flatten() {
                let lower = line.to_lowercase();
                if lower.contains("error") || lower.contains("warn") || lower.contains("corrupt") || lower.contains("missing") || lower.contains("drop") {
                    println!("[LAYER 2 ALERT] GStreamer Decoder Event (at {:.1}ms): {}", t_stderr.elapsed().as_secs_f32() * 1000.0, line);
                } else if lower.contains("resolution changed") || lower.contains("colorimetry") {
                    println!("[DECODER CAPS CHANGE] GStreamer (at {:.1}ms): {}", t_stderr.elapsed().as_secs_f32() * 1000.0, line);
                }
            }
        });
    }

    let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(16);
    std::thread::spawn(move || {
        while let Ok(access_unit) = writer_rx.recv() {
            if stdin.write_all(&access_unit).and_then(|_| stdin.flush()).is_err() {
                eprintln!("[PLAYBACK] GStreamer stdin write failed");
                break;
            }
        }
    });

    println!("[PLAYBACK READY] Persistent GStreamer pipeline active on HDMI connector {connector}, plane {plane}");
    Ok(PlaybackEngine {
        _child: child,
        writer_tx,
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

    // 1. Auto-detect display geometry once before starting GStreamer
    let (screen_w, screen_h, vrefresh, connector_id, render_rect) = autodetect_display_info();

    // 2. Start single persistent GStreamer pipeline
    let playback = start_persistent_playback("hevc", &connector_id, render_rect.as_deref())?;

    let streaming_active = Arc::new(AtomicBool::new(false));

    // 3. Background feeder thread: uses persistent HEVC x265 encoder at native screen resolution to encode live clock dashboard frames every 1.0s
    let idle_active = Arc::clone(&streaming_active);
    let idle_tx = playback.writer_tx.clone();
    std::thread::spawn(move || {
        let mut last_secs = 0u64;
        let mut encoder: Option<PersistentDashboardEncoder> = None;
        loop {
            if !idle_active.load(Ordering::Relaxed) {
                if encoder.is_none() {
                    println!("[IDLE THREAD] Spawning persistent native {}x{} HEVC dashboard encoder...", screen_w, screen_h);
                    encoder = PersistentDashboardEncoder::spawn(screen_w, screen_h, idle_tx.clone()).ok();
                }

                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                if now_secs != last_secs {
                    last_secs = now_secs;
                    if let Some(enc) = encoder.as_mut() {
                        enc.push_frame(vrefresh);
                    }
                }
            } else if encoder.is_some() {
                println!("[IDLE THREAD] Streaming active; terminating idle HEVC dashboard encoder process.");
                encoder = None;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });

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
                        streaming_active.store(true, Ordering::Relaxed);
                        let res_str = resolution.unwrap_or_else(|| "1920x1080".to_string());
                        control_channel.send_telemetry(control::TelemetryMessage::Status {
                            state: "STREAMING".to_string(),
                            resolution: res_str,
                            fps: 30,
                            delivery_rate: 100.0,
                            frames_submitted: sent,
                            latency_ms: 0.0,
                        });
                    }
                    control::ControlCommand::Ping => {
                        control_channel.send_telemetry(control::TelemetryMessage::Pong);
                    }
                    control::ControlCommand::GetStatus => {
                        let state = if streaming_active.load(Ordering::Relaxed) { "STREAMING" } else { "IDLE" };
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
                            streaming_active.store(false, Ordering::Relaxed);
                            v4l2_decoder::reset_decoder_pipeline();
                            while rx.try_recv().is_ok() {}
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "IDLE".to_string(),
                                resolution: "0x0".to_string(),
                                fps: 0,
                                delivery_rate: 100.0,
                                frames_submitted: sent,
                                latency_ms: 0.0,
                            });
                            continue;
                        }
                        if frame.codec != "hevc" && frame.codec != "h264" { continue; }

                        // Allow auto-start if a new sequence frame (seq <= 1) arrives
                        if frame.seq <= 1 {
                            let was_active = streaming_active.swap(true, Ordering::Relaxed);
                            if !was_active {
                                control_channel.send_telemetry(control::TelemetryMessage::Status {
                                    state: "STREAMING".to_string(),
                                    resolution: format!("{}x{}", frame.width, frame.height),
                                    fps: 30,
                                    delivery_rate: 100.0,
                                    frames_submitted: sent,
                                    latency_ms: 0.0,
                                });
                            }
                        }

                        // If streaming was explicitly stopped, discard trailing/out-of-order frames
                        if !streaming_active.load(Ordering::Relaxed) {
                            continue;
                        }

                        if playback.writer_tx.send(frame.access_unit).is_err() {
                            eprintln!("[PLAYBACK ERROR] seq={} pipe write failed", frame.seq);
                            continue;
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
                        // Timeout waiting for frames (30s idle)
                        if streaming_active.load(Ordering::Relaxed) {
                            println!("[PLAYBACK] Stream idle timeout; restoring HDMI IP dashboard...");
                            streaming_active.store(false, Ordering::Relaxed);
                            while rx.try_recv().is_ok() {}
                            control_channel.send_telemetry(control::TelemetryMessage::Status {
                                state: "IDLE".to_string(),
                                resolution: "0x0".to_string(),
                                fps: 0,
                                delivery_rate: 100.0,
                                frames_submitted: sent,
                                latency_ms: 0.0,
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
