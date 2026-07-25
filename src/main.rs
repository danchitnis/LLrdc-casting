/*
 * Radxa Rock 5C+ V4L2 DMA-BUF DRM Atomic Display Pipeline
 * Modular Safe Rust Entry Point
 */

mod drm_kms;
mod gfx;
mod v4l2;

use std::os::fd::AsFd;
use std::os::unix::io::AsRawFd;
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=====================================================");
    println!(" Safe Rust Pipeline: V4L2 -> DMA-BUF -> DRM");
    println!(" Radxa Rock 5C+ / Rockchip RK3588 DRM Display");
    println!("=====================================================\n");

    let args: Vec<String> = std::env::args().collect();
    let requested_dev = args.get(1).map(|s| s.as_str()).unwrap_or("/dev/video0");

    // -------------------------------------------------------------
    // Step 1: Open DRM Card Device & Autodetect HDMI Screen Resolution
    // -------------------------------------------------------------
    println!("[STEP 1] Opening DRM device & autodetecting display mode...");
    let card = drm_kms::open_display_card()?;
    let (screen_w, screen_h, mode, conn_handle, crtc_handle) =
        drm_kms::autodetect_display_mode(&card)?;

    let drm_raw_fd = card.as_fd().as_raw_fd();

    // -------------------------------------------------------------
    // Step 2: Allocate & Export V4L2 DMA-BUF Buffer
    // -------------------------------------------------------------
    println!("\n[STEP 2] Allocating & exporting V4L2 DMA-BUF frame memory...");
    let mut fb_w = screen_w;
    let mut fb_h = screen_h;
    let mut pitch = fb_w;
    let mut pixel_format = drm_kms::DRM_FORMAT_NV12;
    let mut dmabuf_fd = -1;
    let mut _buf_map: *mut libc::c_void = std::ptr::null_mut();
    let mut _buf_size: usize = 0;

    match v4l2::allocate_and_export_v4l2_buffer(requested_dev, screen_w, screen_h) {
        Ok(v4l2_buf) if v4l2_buf.fb_w >= screen_w => {
            dmabuf_fd = v4l2_buf.dmabuf_fd;
            fb_w = v4l2_buf.fb_w;
            fb_h = v4l2_buf.fb_h;
            pitch = v4l2_buf.pitch;
            pixel_format = v4l2_buf.pixel_format;
            _buf_map = v4l2_buf.buf_map;
            _buf_size = v4l2_buf.buf_size;
        }
        Ok(v4l2_buf) => {
            println!("[INFO] V4L2 hardware clamped buffer to {}x{}; upgrading to 1:1 screen resolution ({}x{}) DMA-BUF...",
                     v4l2_buf.fb_w, v4l2_buf.fb_h, screen_w, screen_h);
            unsafe {
                if !v4l2_buf.buf_map.is_null() && v4l2_buf.buf_size > 0 {
                    libc::munmap(v4l2_buf.buf_map, v4l2_buf.buf_size);
                }
                if v4l2_buf.dmabuf_fd >= 0 { libc::close(v4l2_buf.dmabuf_fd); }
            }
        }
        Err(e) => {
            println!("[INFO] V4L2 allocation note: {}; using DRM PRIME DMA-BUF export...", e);
        }
    }

    // Fallback: Allocate 1:1 screen resolution DMA-BUF via DRM PRIME export
    if dmabuf_fd < 0 {
        println!("[INFO] Allocating native DRM PRIME DMA-BUF buffer ({}x{})...", screen_w, screen_h);
        let (fd, pitch_val, size, ptr) = drm_kms::allocate_prime_dmabuf(drm_raw_fd, screen_w, screen_h)?;
        dmabuf_fd = fd;
        fb_w = screen_w;
        fb_h = screen_h;
        pitch = pitch_val;
        _buf_size = size;
        _buf_map = ptr;
        pixel_format = drm_kms::DRM_FORMAT_XRGB8888;
        println!("[DMA-BUF SUCCESS] Created native DMA-BUF fd = {} ({}x{}) via PRIME export", dmabuf_fd, fb_w, fb_h);
    }

    // -------------------------------------------------------------
    // Step 3: Import DMA-BUF fd into DRM Framebuffer
    // -------------------------------------------------------------
    println!("\n[STEP 3] Importing DMA-BUF fd ({}) into DRM Framebuffer...", dmabuf_fd);
    let fb_handle = drm_kms::import_dmabuf_and_add_fb(drm_raw_fd, dmabuf_fd, fb_w, fb_h, pitch, pixel_format)?;

    // -------------------------------------------------------------
    // Step 4: Display Framebuffer directly via DRM CRTC & Modeset
    // -------------------------------------------------------------
    println!("\n[STEP 4] Executing DRM KMS Modeset & Display on CRTC {:?}...", crtc_handle);
    drm_kms::set_display_mode(&card, crtc_handle, fb_handle, conn_handle, mode)?;

    println!("\n=====================================================");
    println!(" [SUCCESS] DRM KMS Display Commit Successful!");
    println!(" Screen Resolution: {}x{} @ {}Hz", screen_w, screen_h, mode.vrefresh());
    println!(" Frame Buffer Size: {}x{}", fb_w, fb_h);
    println!("=====================================================");

    println!("\nDisplaying rectangle on HDMI screen for 10 seconds...");
    thread::sleep(Duration::from_secs(10));

    println!("Done.");
    Ok(())
}
