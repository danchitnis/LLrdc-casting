//! Verified RK3399 fallback: UDP HEVC -> V4L2 stateless decoder -> KMS.
//! The atomic two-plane presenter is developed separately; this keeps HDMI
//! playback on the proven pipeline while it is completed.
mod cert;
mod drm_kms;
mod gfx;
mod http_server;
mod net;
mod text;
mod v4l2_decoder;
mod webtransport_server;

use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStdin, Command, Stdio};
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

fn stop_playback(playback: &mut Option<(Child, ChildStdin, String)>) {
    if let Some((mut child, _, codec)) = playback.take() {
        println!("[PLAYBACK STOPPING] Terminating previous {codec} GStreamer child process...");
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn start_playback(codec: &str) -> Result<(Child, ChildStdin, String), Box<dyn std::error::Error>> {
    let connector = std::env::var("DRM_CONNECTOR_ID").unwrap_or_else(|_| "54".into());
    let plane = std::env::var("DRM_PLANE_ID").unwrap_or_else(|_| "33".into());
    let (parser, decoder) = if codec == "h264" {
        ("h264parse", "v4l2slh264dec")
    } else {
        ("h265parse", "v4l2slh265dec")
    };
    let mut child = Command::new("gst-launch-1.0")
        .env("GST_DEBUG", "v4l2slh265dec:4,h265parse:4,v4l2slh264dec:4,h264parse:4,kmssink:4")
        .args([
            "-q", "fdsrc", "fd=0", "blocksize=4096", "do-timestamp=true", "!",
            parser, "config-interval=-1", "!",
            decoder, "!",
            "kmssink", "driver-name=rockchip",
            &format!("connector-id={connector}"), &format!("plane-id={plane}"),
            "force-modesetting=false", "sync=false", "skip-vsync=true", "max-lateness=0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdin = child.stdin.take().ok_or("could not open GStreamer stdin")?;
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

    println!("[PLAYBACK READY] {codec} -> {parser} -> {decoder} -> HDMI connector {connector}, plane {plane}");
    Ok((child, stdin, codec.to_string()))
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
    spawn_dmesg_kernel_monitor();
    let _ = drm_kms::inspect_live_scanout_status();
    let (tx, mut rx) = mpsc::channel::<v4l2_decoder::VideoFrame>(64);

    let identity = webtransport_server::get_or_create_identity().await.map_err(|e| e as Box<dyn std::error::Error>)?;
    let cert_hash_hex = webtransport_server::extract_cert_hash_hex(&identity);

    let http_cert_hash = cert_hash_hex.clone();
    tokio::spawn(async move {
        if let Err(error) = http_server::run_server(http_cert_hash).await {
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
    let mut playback: Option<(Child, ChildStdin, String)> = None;
    let mut sent = 0u64;
    while let Some(frame) = rx.recv().await {
        if frame.codec != "hevc" && frame.codec != "h264" { continue; }
        
        let need_restart = match &playback {
            Some((_, _, current_codec)) => current_codec != &frame.codec,
            None => true,
        };

        if need_restart {
            stop_playback(&mut playback);
            // Release the dashboard's DRM master immediately before hand-off.
            if let Some(dashboard) = dashboard.take() { dashboard.release(); }
            playback = Some(start_playback(&frame.codec)?);
        }

        let (_, stdin, _) = playback.as_mut().expect("playback initialized");
        if let Err(error) = stdin.write_all(&frame.access_unit).and_then(|_| stdin.flush()) {
            eprintln!("[PLAYBACK ERROR] seq={} {error}; restarting pipeline", frame.seq);
            stop_playback(&mut playback);
            continue;
        }
        sent += 1;
        if sent == 1 || sent % 60 == 0 { println!("[PLAYBACK] submitted_{}_access_units={sent}", frame.codec); }
    }
    Ok(())
}
