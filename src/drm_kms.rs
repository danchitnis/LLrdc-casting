/*
 * DRM KMS Display Module: Card opening, mode autodetection & display inspection
 */

use std::fs::{File, OpenOptions};
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd};

use drm::control::{
    connector, crtc,
    Device as ControlDevice, Mode,
};
use drm::Device as DrmDevice;
use libc::c_int;

// Wrapper struct for DRM card device
#[derive(Debug)]
pub struct Card(pub File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl DrmDevice for Card {}
impl ControlDevice for Card {}

#[link(name = "drm")]
extern "C" {
    fn drmDropMaster(fd: c_int) -> c_int;
}

/// Explicitly relinquish DRM master before another process takes over KMS.
pub fn drop_master(card: &Card) {
    unsafe { let _ = drmDropMaster(card.0.as_raw_fd()); }
}

/// Open active DRM display card (`/dev/dri/card0`)
pub fn open_display_card() -> Result<Card, Box<dyn std::error::Error>> {
    for i in 0..4 {
        let card_path = format!("/dev/dri/card{}", i);
        if let Ok(file) = OpenOptions::new().read(true).write(true).open(&card_path) {
            let card = Card(file);
            if let Ok(handles) = card.resource_handles() {
                if !handles.connectors().is_empty() && !handles.crtcs().is_empty() {
                    println!("[DRM SUCCESS] Opened display card: {}", card_path);

                    let mut last_err = None;
                    for _attempt in 1..=10 {
                        let res1 = card.set_client_capability(drm::ClientCapability::UniversalPlanes, true);
                        let res2 = card.set_client_capability(drm::ClientCapability::Atomic, true);
                        if res1.is_ok() && res2.is_ok() {
                            return Ok(card);
                        }
                        if let Err(e) = res1.or(res2) {
                            last_err = Some(e);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if let Some(e) = last_err {
                        return Err(Box::new(e));
                    }
                    return Ok(card);
                }
            }
        }
    }
    Err("Could not find an active DRM display card".into())
}

fn score_mode(mode: &Mode) -> u64 {
    let (w, h) = (mode.size().0 as u64, mode.size().1 as u64);
    let fps = mode.vrefresh() as u64;
    let is_preferred = mode.mode_type().contains(drm::control::ModeTypeFlags::PREFERRED);

    let area = w * h;
    let fps_bonus = if fps >= 50 { 100_000 } else if fps >= 30 { 50_000 } else { 0 };
    let preferred_bonus = if is_preferred { 100_000 } else { 0 };

    area + fps_bonus + preferred_bonus
}

/// Autodetect active HDMI display connector and preferred mode resolution
pub fn autodetect_display_mode(card: &Card) -> Result<(u32, u32, Mode, connector::Handle, crtc::Handle), Box<dyn std::error::Error>> {
    let resources = card.resource_handles()?;
    let mut target_connector = None;
    let mut selected_mode: Option<Mode> = None;

    for &conn_handle in resources.connectors() {
        if let Ok(conn_info) = card.get_connector(conn_handle, true) {
            if conn_info.state() == connector::State::Connected {
                let conn_type = conn_info.interface();
                if conn_type == connector::Interface::HDMIA || conn_type == connector::Interface::HDMIB {
                    println!("[DRM] Found connected HDMI connector: {:?}", conn_handle);
                    // Select optimal HDMI display mode prioritizing standard 16:9 60Hz and preferred EDID modes
                    let mut best_mode: Option<Mode> = None;
                    let mut best_score: u64 = 0;
                    for mode in conn_info.modes() {
                        let score = score_mode(mode);
                        if best_mode.is_none() || score > best_score {
                            best_mode = Some(*mode);
                            best_score = score;
                        }
                    }

                    if let Some(mode) = best_mode {
                        println!("[DRM] Selected optimal HDMI mode: {}x{} @ {}Hz (score={})", mode.size().0, mode.size().1, mode.vrefresh(), best_score);
                        selected_mode = Some(mode);
                    }
                    target_connector = Some(conn_handle);
                    break;
                }
            }
        }
    }

    let conn_handle = target_connector.ok_or("No connected HDMI connector found")?;
    let mode = selected_mode.ok_or("No mode found for connected HDMI display")?;

    let (screen_w, screen_h) = (mode.size().0 as u32, mode.size().1 as u32);
    let crtc_handle = resources.crtcs()[0];

    println!("[DRM AUTODETECT SUCCESS] Screen Resolution: {}x{} @ {}Hz (Connector: {:?}, CRTC: {:?})",
             screen_w, screen_h, mode.vrefresh(), conn_handle, crtc_handle);

    Ok((screen_w, screen_h, mode, conn_handle, crtc_handle))
}

/// Inspect active DRM CRTC and HDMI scanout state (Layer 3 Inspector)
pub fn inspect_live_scanout_status() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(card) = open_display_card() {
        let resources = card.resource_handles()?;
        for &crtc in resources.crtcs() {
            if let Ok(info) = card.get_crtc(crtc) {
                if let Some(mode) = info.mode() {
                    let fb = info.framebuffer();
                    println!(
                        "[LAYER 3 INSPECTOR] Active DRM CRTC {:?} | Resolution: {}x{} @ {}Hz | FB Handle: {:?}",
                        crtc, mode.size().0, mode.size().1, mode.vrefresh(), fb
                    );
                }
            }
        }
        drop_master(&card);
    }
    Ok(())
}
