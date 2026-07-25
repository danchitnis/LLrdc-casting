/*
 * Radxa Rock 5C+ WebTransport QUIC UDP Remote Screen Sharing Server
 * Safe Rust V4L2 DMA-BUF DRM Atomic Display Pipeline
 */

mod drm_kms;
mod gfx;
mod net;
mod text;
mod v4l2;
mod v4l2_decoder;
mod webtransport_server;

use std::os::fd::AsFd;
use std::os::unix::io::AsRawFd;
use std::thread;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=====================================================");
    println!(" Safe Rust Pipeline: WebTransport -> V4L2 -> DRM");
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
    // Step 2b: Query Active Network IPv4 Addresses & Render to Framebuffer Memory
    // -------------------------------------------------------------
    let active_ips = net::get_active_ipv4_addresses();
    println!("\n[NETWORK] Active IPv4 Addresses detected on device:");
    for (iface, ip) in &active_ips {
        println!("  - {:<10} : {}", iface, ip);
    }

    if !_buf_map.is_null() && _buf_size > 0 {
        if pixel_format == drm_kms::DRM_FORMAT_XRGB8888 || pixel_format == drm_kms::DRM_FORMAT_ARGB8888 {
            let slice = unsafe { std::slice::from_raw_parts_mut(_buf_map as *mut u32, _buf_size / 4) };
            text::draw_ip_dashboard_argb(slice, fb_w, fb_h, &active_ips);
        } else if pixel_format == drm_kms::DRM_FORMAT_NV12 {
            let slice = unsafe { std::slice::from_raw_parts_mut(_buf_map as *mut u8, _buf_size) };
            text::draw_ip_dashboard_nv12(slice, fb_w, fb_h, &active_ips);
        }
    }

    // -------------------------------------------------------------
    // Step 3: Import DMA-BUF fd into DRM Framebuffer & Commit Modeset
    // -------------------------------------------------------------
    println!("\n[STEP 3] Importing DMA-BUF fd ({}) into DRM Framebuffer...", dmabuf_fd);
    let fb_handle = drm_kms::import_dmabuf_and_add_fb(drm_raw_fd, dmabuf_fd, fb_w, fb_h, pitch, pixel_format)?;

    println!("\n[STEP 4] Executing DRM KMS Modeset & Display on CRTC {:?}...", crtc_handle);
    drm_kms::set_display_mode(&card, crtc_handle, fb_handle, conn_handle, mode)?;

    // -------------------------------------------------------------
    // Step 5: Display IP Dashboard for EXACTLY 1 second
    // -------------------------------------------------------------
    println!("\n=====================================================");
    println!(" [SUCCESS] DRM KMS Display Active!");
    println!(" [TIMING] Displaying IPv4 Address Dashboard on HDMI for 1 second...");
    println!("=====================================================");
    thread::sleep(Duration::from_secs(1));

    // -------------------------------------------------------------
    // Step 6: Start WebTransport Server & Video Stream Decoder Loop
    // -------------------------------------------------------------
    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(32);

    // Spawn WebTransport QUIC UDP server on 0.0.0.0:4433
    tokio::spawn(async move {
        if let Err(e) = webtransport_server::run_server(frame_tx).await {
            eprintln!("[SERVER ERROR] WebTransport QUIC server error: {}", e);
        }
    });

    println!("\n[SERVER READY] WebTransport QUIC UDP Server running on port 4433.");
    println!(" Waiting for incoming H.264 video streams from remote client...");

    // Continuously process and display incoming H.264 video frames
    while let Some(h264_payload) = frame_rx.recv().await {
        if let Err(e) = v4l2_decoder::process_and_render_h264_frame(
            &h264_payload,
            _buf_map,
            _buf_size,
            fb_w,
            fb_h,
            pixel_format,
        ) {
            eprintln!("[DECODER ERROR] Failed to decode/render H.264 frame: {}", e);
        } else {
            // Refresh DRM KMS display commit
            let _ = drm_kms::set_display_mode(&card, crtc_handle, fb_handle, conn_handle, mode);
        }
    }

    Ok(())
}
