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

fn start_playback() -> Result<(Child, ChildStdin), Box<dyn std::error::Error>> {
    let connector = std::env::var("DRM_CONNECTOR_ID").unwrap_or_else(|_| "54".into());
    let plane = std::env::var("DRM_PLANE_ID").unwrap_or_else(|_| "33".into());
    let mut child = Command::new("gst-launch-1.0")
        .args([
            "-q", "fdsrc", "fd=0", "blocksize=262144", "!", "h265parse", "!",
            "v4l2slh265dec", "!", "kmssink", "driver-name=rockchip",
            &format!("connector-id={connector}"), &format!("plane-id={plane}"),
            "force-modesetting=false", "sync=false", "skip-vsync=true", "max-lateness=0",
        ])
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn()?;
    let stdin = child.stdin.take().ok_or("could not open GStreamer stdin")?;
    println!("[PLAYBACK READY] HEVC -> v4l2slh265dec -> HDMI connector {connector}, plane {plane}");
    Ok((child, stdin))
}

/// Holds the DRM file and scanout allocation while the receiver is idle. It
/// must be dropped before gst-launch starts so the playback process can become
/// DRM master and take over the same HDMI plane.
struct IdleDashboard {
    _card: drm_kms::Card,
    _fb: drm::control::framebuffer::Handle,
    _prime_fd: i32,
}

impl IdleDashboard {
    fn release(self) {
        drm_kms::drop_master(&self._card);
        println!("[IDLE DASHBOARD] released DRM master for video playback.");
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
    Ok(IdleDashboard { _card: card, _fb: fb, _prime_fd: prime_fd })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, mut rx) = mpsc::channel::<v4l2_decoder::VideoFrame>(2);
    tokio::spawn(async move { if let Err(error) = webtransport_server::run_server(tx).await { eprintln!("[SERVER ERROR] {error}"); } });
    let mut dashboard = if std::env::var("IDLE_DASHBOARD").map_or(true, |v| v != "0") {
        Some(show_idle_dashboard()?)
    } else { None };
    println!("[READY] waiting for H.265 UDP access units on port 4434");
    let mut playback: Option<(Child, ChildStdin)> = None;
    let mut sent = 0u64;
    while let Some(mut frame) = rx.recv().await {
        while let Ok(newer) = rx.try_recv() { frame = newer; }
        if frame.codec != "hevc" { continue; }
        if playback.is_none() {
            // Release the dashboard's DRM master immediately before hand-off.
            if let Some(dashboard) = dashboard.take() { dashboard.release(); }
            playback = Some(start_playback()?);
        }
        let (_, stdin) = playback.as_mut().expect("playback initialized");
        if let Err(error) = stdin.write_all(&frame.access_unit).and_then(|_| stdin.flush()) {
            eprintln!("[PLAYBACK ERROR] seq={} {error}; restarting pipeline", frame.seq);
            playback = None;
            continue;
        }
        sent += 1;
        if sent == 1 || sent % 60 == 0 { println!("[PLAYBACK] submitted_hevc_access_units={sent}"); }
    }
    Ok(())
}
