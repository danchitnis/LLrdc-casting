//! Verified RK3399 fallback: UDP HEVC -> V4L2 stateless decoder -> KMS.
//! The atomic two-plane presenter is developed separately; this keeps HDMI
//! playback on the proven pipeline while it is completed.
mod drm_kms;
mod gfx;
mod net;
mod text;
mod v4l2_decoder;
mod webtransport_server;

use std::io::Write;
use std::os::fd::AsRawFd;
use std::process::{Child, ChildStdin, Command, Stdio};
use tokio::sync::mpsc;

fn start_playback(codec: &str) -> Result<(Child, ChildStdin, String), Box<dyn std::error::Error>> {
    let connector = std::env::var("DRM_CONNECTOR_ID").unwrap_or_else(|_| "54".into());
    let plane = std::env::var("DRM_PLANE_ID").unwrap_or_else(|_| "33".into());
    let (parser, decoder) = if codec == "h264" {
        ("h264parse", "v4l2slh264dec")
    } else {
        ("h265parse", "v4l2slh265dec")
    };
    let mut child = Command::new("gst-launch-1.0")
        .args([
            "-q", "fdsrc", "fd=0", "blocksize=262144", "!", parser, "!",
            decoder, "!", "kmssink", "driver-name=rockchip",
            &format!("connector-id={connector}"), &format!("plane-id={plane}"),
            "force-modesetting=false", "sync=false", "skip-vsync=true", "max-lateness=0",
        ])
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn()?;
    let stdin = child.stdin.take().ok_or("could not open GStreamer stdin")?;
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
    let (tx, mut rx) = mpsc::channel::<v4l2_decoder::VideoFrame>(2);
    tokio::spawn(async move { if let Err(error) = webtransport_server::run_server(tx).await { eprintln!("[SERVER ERROR] {error}"); } });
    let mut dashboard = if std::env::var("IDLE_DASHBOARD").map_or(true, |v| v != "0") {
        Some(show_idle_dashboard()?)
    } else { None };
    println!("[READY] waiting for video stream access units on UDP/WebTransport");
    let mut playback: Option<(Child, ChildStdin, String)> = None;
    let mut sent = 0u64;
    while let Some(mut frame) = rx.recv().await {
        while let Ok(newer) = rx.try_recv() { frame = newer; }
        if frame.codec != "hevc" && frame.codec != "h264" { continue; }
        
        let need_restart = match &playback {
            Some((_, _, current_codec)) => current_codec != &frame.codec,
            None => true,
        };

        if need_restart {
            playback = None;
            // Release the dashboard's DRM master immediately before hand-off.
            if let Some(dashboard) = dashboard.take() { dashboard.release(); }
            playback = Some(start_playback(&frame.codec)?);
        }

        let (_, stdin, _) = playback.as_mut().expect("playback initialized");
        if let Err(error) = stdin.write_all(&frame.access_unit).and_then(|_| stdin.flush()) {
            eprintln!("[PLAYBACK ERROR] seq={} {error}; restarting pipeline", frame.seq);
            playback = None;
            continue;
        }
        sent += 1;
        if sent == 1 || sent % 60 == 0 { println!("[PLAYBACK] submitted_{}_access_units={sent}", frame.codec); }
    }
    Ok(())
}
