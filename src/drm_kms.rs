/*
 * DRM KMS Display Module: Card opening, mode autodetection, GEM import & CRTC modeset
 */

use std::fs::{File, OpenOptions};
use std::num::NonZeroU32;
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::ptr;

use drm::control::{
    connector, crtc, framebuffer,
    Device as ControlDevice, Mode,
};
use drm::Device as DrmDevice;
use libc::{c_int, O_CLOEXEC, O_RDWR, MAP_SHARED, PROT_READ, PROT_WRITE, MAP_FAILED};

pub const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
pub const DRM_FORMAT_NV12: u32 = u32::from_le_bytes(*b"NV12");

const DRM_IOCTL_MODE_CREATE_DUMB: u64 = 0xc02064b2;
const DRM_IOCTL_MODE_MAP_DUMB: u64 = 0xc01064b3;
const DRM_IOCTL_PRIME_HANDLE_TO_FD: u64 = 0xc00c642d;

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

#[repr(C)]
struct DrmModeCreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}

#[repr(C)]
struct DrmModeMapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}

#[repr(C)]
struct DrmPrimeHandle {
    handle: u32,
    flags: u32,
    fd: c_int,
}

#[link(name = "drm")]
extern "C" {
    fn drmDropMaster(fd: c_int) -> c_int;
    fn drmPrimeFDToHandle(fd: c_int, prime_fd: u32, handle: *mut u32) -> c_int;
    fn drmModeAddFB2(
        fd: c_int, width: u32, height: u32, pixel_format: u32,
        bo_handles: *const u32, pitches: *const u32, offsets: *const u32,
        buf_id: *mut u32, flags: u32,
    ) -> c_int;
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
                        println!("[DRM MODE OPTION] {}x{} @ {}Hz (pref={}) -> score={}",
                                 mode.size().0, mode.size().1, mode.vrefresh(),
                                 mode.mode_type().contains(drm::control::ModeTypeFlags::PREFERRED), score);
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

/// Import DMA-BUF fd into DRM GEM handle and create Framebuffer
pub fn import_dmabuf_and_add_fb(
    drm_raw_fd: RawFd,
    dmabuf_fd: RawFd,
    fb_w: u32,
    fb_h: u32,
    pitch: u32,
    pixel_format: u32,
) -> Result<framebuffer::Handle, Box<dyn std::error::Error>> {
    let mut gem_handle: u32 = 0;
    let mut fb_id: u32 = 0;

    unsafe {
        if drmPrimeFDToHandle(drm_raw_fd, dmabuf_fd as u32, &mut gem_handle) < 0 {
            return Err("drmPrimeFDToHandle failed".into());
        }
        println!("[DRM SUCCESS] Converted DMA-BUF fd ({}) -> GEM Handle ({})", dmabuf_fd, gem_handle);

        let handles: [u32; 4] = [
            gem_handle,
            if pixel_format == DRM_FORMAT_NV12 { gem_handle } else { 0 },
            0, 0,
        ];
        let pitches: [u32; 4] = [
            if pixel_format == DRM_FORMAT_NV12 { fb_w } else { pitch },
            if pixel_format == DRM_FORMAT_NV12 { fb_w } else { 0 },
            0, 0,
        ];
        let offsets: [u32; 4] = [
            0,
            if pixel_format == DRM_FORMAT_NV12 { fb_w * fb_h } else { 0 },
            0, 0,
        ];

        if drmModeAddFB2(drm_raw_fd, fb_w, fb_h, pixel_format, handles.as_ptr(), pitches.as_ptr(), offsets.as_ptr(), &mut fb_id, 0) < 0 {
            return Err("drmModeAddFB2 failed".into());
        }
    }

    let fb_handle = framebuffer::Handle::from(
        NonZeroU32::new(fb_id).ok_or("Invalid Framebuffer ID")?
    );

    println!("[DRM SUCCESS] Created DRM Framebuffer Handle = {:?} ({}x{})", fb_handle, fb_w, fb_h);
    Ok(fb_handle)
}

/// Set CRTC mode and display the framebuffer directly on the HDMI screen
pub fn set_display_mode(
    card: &Card,
    crtc_handle: crtc::Handle,
    fb_handle: framebuffer::Handle,
    conn_handle: connector::Handle,
    mode: Mode,
) -> Result<(), Box<dyn std::error::Error>> {
    card.set_crtc(crtc_handle, Some(fb_handle), (0, 0), &[conn_handle], Some(mode))?;
    Ok(())
}

/// Create native 1:1 screen resolution DRM PRIME DMA-BUF buffer
pub fn allocate_prime_dmabuf(
    drm_fd: RawFd,
    width: u32,
    height: u32,
) -> Result<(RawFd, u32, usize, *mut std::ffi::c_void), Box<dyn std::error::Error>> {
    unsafe {
        let mut create_dumb = DrmModeCreateDumb {
            width,
            height,
            bpp: 32,
            flags: 0,
            handle: 0,
            pitch: 0,
            size: 0,
        };

        if libc::ioctl(drm_fd, DRM_IOCTL_MODE_CREATE_DUMB, &mut create_dumb) < 0 {
            return Err("DRM_IOCTL_MODE_CREATE_DUMB failed".into());
        }

        let mut prime = DrmPrimeHandle {
            handle: create_dumb.handle,
            flags: (O_CLOEXEC | O_RDWR) as u32,
            fd: -1,
        };

        if libc::ioctl(drm_fd, DRM_IOCTL_PRIME_HANDLE_TO_FD, &mut prime) < 0 {
            return Err("DRM_IOCTL_PRIME_HANDLE_TO_FD failed".into());
        }

        let mut map_dumb = DrmModeMapDumb {
            handle: create_dumb.handle,
            pad: 0,
            offset: 0,
        };

        if libc::ioctl(drm_fd, DRM_IOCTL_MODE_MAP_DUMB, &mut map_dumb) < 0 {
            return Err("DRM_IOCTL_MODE_MAP_DUMB failed".into());
        }

        let ptr = libc::mmap(
            ptr::null_mut(),
            create_dumb.size as usize,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            drm_fd,
            map_dumb.offset as libc::off_t,
        );

        if ptr == MAP_FAILED {
            return Err("mmap failed".into());
        }

        let slice = std::slice::from_raw_parts_mut(ptr as *mut u32, create_dumb.size as usize / 4);
        crate::gfx::draw_rectangles_argb(slice, width, height);

        Ok((prime.fd, create_dumb.pitch, create_dumb.size as usize, ptr))
    }
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
    }
    Ok(())
}
