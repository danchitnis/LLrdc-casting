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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdidInfo {
    pub name: String,
    pub conn_type: String,
    pub max_res: String,
    pub max_fps: u32,
}

impl Default for EdidInfo {
    fn default() -> Self {
        Self {
            name: "HDMI Monitor".to_string(),
            conn_type: "HDMI-A".to_string(),
            max_res: "1920x1080".to_string(),
            max_fps: 60,
        }
    }
}

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

fn parse_edid_monitor_name(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 128 {
        return None;
    }
    if bytes[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }

    // Check descriptor blocks at offsets 54, 72, 90, 108
    for offset in [54, 72, 90, 108] {
        if offset + 18 <= bytes.len() {
            let block = &bytes[offset..offset + 18];
            if block[0] == 0x00 && block[1] == 0x00 && block[2] == 0x00 && block[3] == 0xFC {
                let name_bytes = &block[5..18];
                let name_str = String::from_utf8_lossy(name_bytes);
                let trimmed = name_str.trim_matches(|c: char| c == '\n' || c == '\r' || c == '\0' || c == ' ').to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }

    // Fallback: Manufacturer ID from bytes 8..9
    let b8 = bytes[8] as u16;
    let b9 = bytes[9] as u16;
    let c1 = (((b8 & 0x7C) >> 2) as u8 + b'A' - 1) as char;
    let c2 = ((((b8 & 0x03) << 3) | ((b9 & 0xE0) >> 5)) as u8 + b'A' - 1) as char;
    let c3 = ((b9 & 0x1F) as u8 + b'A' - 1) as char;
    if c1.is_ascii_alphabetic() && c2.is_ascii_alphabetic() && c3.is_ascii_alphabetic() {
        return Some(format!("{} Display", format!("{}{}{}", c1, c2, c3)));
    }

    None
}

fn parse_edid_max_resolution(bytes: &[u8]) -> Option<(u32, u32, u32)> {
    if bytes.len() < 128 {
        return None;
    }
    if bytes[0..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }

    let mut max_w = 0u32;
    let mut max_h = 0u32;
    let mut max_fps = 0u32;

    let mut check_mode = |w: u32, h: u32, fps: u32| {
        if w > max_w || (w == max_w && h > max_h) || (w == max_w && h == max_h && fps > max_fps) {
            max_w = w;
            max_h = h;
            max_fps = fps;
        }
    };

    // 1. Check DTDs in Base Block (offsets 54, 72, 90, 108)
    for offset in [54, 72, 90, 108] {
        if offset + 18 <= bytes.len() {
            let block = &bytes[offset..offset + 18];
            let pixel_clock = u16::from_le_bytes([block[0], block[1]]) as u32 * 10_000;
            if pixel_clock > 0 {
                let h_active = (block[2] as u32) | (((block[4] as u32) & 0xF0) << 4);
                let h_blank = (block[3] as u32) | (((block[4] as u32) & 0x0F) << 8);
                let v_active = (block[5] as u32) | (((block[7] as u32) & 0xF0) << 4);
                let v_blank = (block[6] as u32) | (((block[7] as u32) & 0x0F) << 8);
                let h_total = h_active + h_blank;
                let v_total = v_active + v_blank;
                if h_active > 0 && v_active > 0 && h_total > 0 && v_total > 0 {
                    let fps = (pixel_clock as f64 / (h_total * v_total) as f64).round() as u32;
                    check_mode(h_active, v_active, fps);
                }
            }
        }
    }

    // 2. Check CTA-861 Extension Block (Block 1 at offset 128)
    if bytes.len() >= 256 && bytes[128] == 0x02 {
        let ext = &bytes[128..256];
        let dtd_offset = ext[2] as usize;
        let dtd_end = if dtd_offset > 4 && dtd_offset <= 127 { dtd_offset } else { 127 };

        let mut idx = 4;
        while idx < dtd_end {
            let header = ext[idx];
            let tag = header >> 5;
            let len = (header & 0x1F) as usize;
            idx += 1;
            if idx + len > dtd_end {
                break;
            }
            let block_data = &ext[idx..idx + len];
            if tag == 2 { // Video Data Block (VICs)
                for &vic_raw in block_data {
                    let vic = vic_raw & 0x7F;
                    let (w, h, fps) = match vic {
                        93 => (3840, 2160, 24),
                        94 => (3840, 2160, 25),
                        95 => (3840, 2160, 30),
                        96 => (3840, 2160, 50),
                        97 => (3840, 2160, 60),
                        98 => (4096, 2160, 24),
                        99 => (4096, 2160, 25),
                        100 => (4096, 2160, 30),
                        101 => (4096, 2160, 50),
                        102 => (4096, 2160, 60),
                        103 => (4096, 2160, 100),
                        104 => (4096, 2160, 120),
                        105 => (4096, 2160, 24),
                        106 => (4096, 2160, 25),
                        107 => (4096, 2160, 30),
                        108..=110 => (3840, 2160, 60),
                        111..=113 => (4096, 2160, 60),
                        117..=118 => (3840, 2160, 120),
                        119..=120 => (4096, 2160, 120),
                        _ => (0, 0, 0),
                    };
                    if w > 0 && h > 0 {
                        check_mode(w, h, fps);
                    }
                }
            }
            idx += len;
        }

        let mut d_idx = dtd_offset;
        while d_idx + 18 <= 127 {
            let block = &ext[d_idx..d_idx + 18];
            let pixel_clock = u16::from_le_bytes([block[0], block[1]]) as u32 * 10_000;
            if pixel_clock > 0 {
                let h_active = (block[2] as u32) | (((block[4] as u32) & 0xF0) << 4);
                let h_blank = (block[3] as u32) | (((block[4] as u32) & 0x0F) << 8);
                let v_active = (block[5] as u32) | (((block[7] as u32) & 0xF0) << 4);
                let v_blank = (block[6] as u32) | (((block[7] as u32) & 0x0F) << 8);
                let h_total = h_active + h_blank;
                let v_total = v_active + v_blank;
                if h_active > 0 && v_active > 0 && h_total > 0 && v_total > 0 {
                    let fps = (pixel_clock as f64 / (h_total * v_total) as f64).round() as u32;
                    check_mode(h_active, v_active, fps);
                }
            }
            d_idx += 18;
        }
    }

    if max_w > 0 && max_h > 0 {
        Some((max_w, max_h, max_fps))
    } else {
        None
    }
}

pub fn extract_edid_info(card: &Card, conn_handle: connector::Handle, conn_info: &connector::Info) -> EdidInfo {
    let conn_type = match conn_info.interface() {
        connector::Interface::HDMIA => "HDMI-A".to_string(),
        connector::Interface::HDMIB => "HDMI-B".to_string(),
        connector::Interface::DisplayPort => "DisplayPort".to_string(),
        other => format!("{:?}", other),
    };

    let mut max_w = 0u32;
    let mut max_h = 0u32;
    let mut max_fps = 0u32;

    for mode in conn_info.modes() {
        let w = mode.size().0 as u32;
        let h = mode.size().1 as u32;
        let fps = mode.vrefresh() as u32;
        if (w * h > max_w * max_h) || (w * h == max_w * max_h && fps > max_fps) {
            max_w = w;
            max_h = h;
        }
        if fps > max_fps {
            max_fps = fps;
        }
    }

    let mut raw_edid_bytes: Option<Vec<u8>> = None;

    if let Ok(props) = card.get_properties(conn_handle) {
        let (handles, values) = props.as_props_and_values();
        for (&prop_handle, &prop_val) in handles.iter().zip(values.iter()) {
            if let Ok(prop_info) = card.get_property(prop_handle) {
                if prop_info.name().to_string_lossy() == "EDID" {
                    if let Ok(blob) = card.get_property_blob(prop_val) {
                        raw_edid_bytes = Some(blob);
                        break;
                    }
                }
            }
        }
    }

    if raw_edid_bytes.is_none() {
        if let Ok(entries) = std::fs::read_dir("/sys/class/drm") {
            for entry in entries.flatten() {
                let path = entry.path().join("edid");
                if path.exists() {
                    if let Ok(bytes) = std::fs::read(&path) {
                        if bytes.len() >= 128 && bytes[0..8] == [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
                            raw_edid_bytes = Some(bytes);
                            break;
                        }
                    }
                }
            }
        }
    }

    // Override max resolution if EDID binary explicitly specifies higher resolution (e.g. 4096x2160 CTA-861 VICs/DTDs)
    if let Some(bytes) = raw_edid_bytes.as_deref() {
        if let Some((e_w, e_h, e_fps)) = parse_edid_max_resolution(bytes) {
            if e_w > max_w || (e_w == max_w && e_h > max_h) || (e_w == max_w && e_h == max_h && e_fps > max_fps) {
                max_w = e_w;
                max_h = e_h;
            }
            if e_fps > max_fps {
                max_fps = e_fps;
            }
        }
    }

    let max_res = if max_w > 0 && max_h > 0 {
        format!("{}x{}", max_w, max_h)
    } else {
        "1920x1080".to_string()
    };
    let max_fps = if max_fps > 0 { max_fps } else { 60 };

    let name = raw_edid_bytes
        .as_deref()
        .and_then(parse_edid_monitor_name)
        .unwrap_or_else(|| "HDMI Monitor".to_string());

    EdidInfo {
        name,
        conn_type,
        max_res,
        max_fps,
    }
}

/// Autodetect active HDMI display connector and preferred mode resolution
pub fn autodetect_display_mode(card: &Card) -> Result<(u32, u32, Mode, connector::Handle, crtc::Handle, EdidInfo), Box<dyn std::error::Error>> {
    let resources = card.resource_handles()?;
    let mut target_connector = None;
    let mut selected_mode: Option<Mode> = None;
    let mut target_conn_info: Option<connector::Info> = None;

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
                        println!("[DRM AVAILABLE MODE] {}x{} @ {}Hz (flags={:?}, score={})", mode.size().0, mode.size().1, mode.vrefresh(), mode.mode_type(), score);
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
                    target_conn_info = Some(conn_info);
                    break;
                }
            }
        }
    }

    let conn_handle = target_connector.ok_or("No connected HDMI connector found")?;
    let conn_info = target_conn_info.ok_or("No info for connected HDMI connector")?;
    let mode = selected_mode.ok_or("No mode found for connected HDMI display")?;

    let edid_info = extract_edid_info(card, conn_handle, &conn_info);

    let (screen_w, screen_h) = (mode.size().0 as u32, mode.size().1 as u32);
    let crtc_handle = resources.crtcs()[0];

    println!("[DRM AUTODETECT SUCCESS] Screen Resolution: {}x{} @ {}Hz (Name: '{}', Max: {}@{}Hz, Connector: {:?}, CRTC: {:?})",
             screen_w, screen_h, mode.vrefresh(), edid_info.name, edid_info.max_res, edid_info.max_fps, conn_handle, crtc_handle);

    Ok((screen_w, screen_h, mode, conn_handle, crtc_handle, edid_info))
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
