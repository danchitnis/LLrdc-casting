/*
 * Radxa Rock 5C+ WebTransport QUIC UDP Remote Screen Sharing Server
 * Safe Rust V4L2 DMA-BUF DRM Atomic Display Pipeline (Real-Time Clock & Double-Buffered Page Flip)
 */

mod drm_kms;
mod gfx;
mod net;
mod text;
mod v4l2_decoder;
mod webtransport_server;

use drm::control::Device as ControlDevice;
use std::os::fd::AsFd;
use std::os::unix::io::AsRawFd;
use std::time::Duration;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=====================================================");
    println!(" Safe Rust Pipeline: WebTransport -> V4L2 -> DRM");
    println!(" Radxa Rock 5C+ / Rockchip RK3588 DRM Display");
    println!("=====================================================\n");

    // -------------------------------------------------------------
    // Step 1: Open DRM Card Device & Autodetect HDMI Screen Resolution
    // -------------------------------------------------------------
    println!("[STEP 1] Opening DRM device & autodetecting display mode...");
    let card = drm_kms::open_display_card()?;
    let (screen_w, screen_h, mode, conn_handle, crtc_handle) =
        drm_kms::autodetect_display_mode(&card)?;

    // Initialize V4L2 Hardware Decoder Node (/dev/video2 / rkvdec)
    v4l2_decoder::init_hardware_decoder()?;

    let drm_raw_fd = card.as_fd().as_raw_fd();

    // -------------------------------------------------------------
    // Step 2: Allocate DOUBLE-BUFFERED Native DRM PRIME DMA-BUF Buffers
    // -------------------------------------------------------------
    println!("\n[STEP 2] Allocating Double-Buffered DRM PRIME frame memory ({}x{})...", screen_w, screen_h);

    let (fd_0, pitch_0, size_0, ptr_0) = drm_kms::allocate_prime_dmabuf(drm_raw_fd, screen_w, screen_h)?;
    let fb_handle_0 = drm_kms::import_dmabuf_and_add_fb(drm_raw_fd, fd_0, screen_w, screen_h, pitch_0, drm_kms::DRM_FORMAT_XRGB8888)?;
    println!("[DMA-BUF 0] Buffer 0 ready: fd={}, FB={:?}", fd_0, fb_handle_0);

    let (fd_1, _pitch_1, size_1, ptr_1) = drm_kms::allocate_prime_dmabuf(drm_raw_fd, screen_w, screen_h)?;
    let fb_handle_1 = drm_kms::import_dmabuf_and_add_fb(drm_raw_fd, fd_1, screen_w, screen_h, pitch_0, drm_kms::DRM_FORMAT_XRGB8888)?;
    println!("[DMA-BUF 1] Buffer 1 ready: fd={}, FB={:?}", fd_1, fb_handle_1);

    // -------------------------------------------------------------
    // Step 2b: Query Active Network IPv4 Addresses
    // -------------------------------------------------------------
    let active_ips = net::get_active_ipv4_addresses();
    println!("\n[NETWORK] Active IPv4 Addresses detected on device:");
    for (iface, ip) in &active_ips {
        println!("  - {:<10} : {}", iface, ip);
    }

    if !ptr_0.is_null() && size_0 > 0 {
        let slice_0 = unsafe { std::slice::from_raw_parts_mut(ptr_0 as *mut u32, size_0 / 4) };
        text::draw_ip_dashboard_argb(slice_0, screen_w, screen_h, &active_ips);
    }
    if !ptr_1.is_null() && size_1 > 0 {
        let slice_1 = unsafe { std::slice::from_raw_parts_mut(ptr_1 as *mut u32, size_1 / 4) };
        text::draw_ip_dashboard_argb(slice_1, screen_w, screen_h, &active_ips);
    }

    // -------------------------------------------------------------
    // Step 3: Commit Initial CRTC Mode
    // -------------------------------------------------------------
    println!("\n[STEP 3] Executing Initial DRM KMS Modeset on CRTC {:?}...", crtc_handle);
    drm_kms::set_display_mode(&card, crtc_handle, fb_handle_0, conn_handle, mode)?;

    // -------------------------------------------------------------
    // Step 4: Start WebTransport Server & Video Stream Receiver
    // -------------------------------------------------------------
    let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(32);

    tokio::spawn(async move {
        if let Err(e) = webtransport_server::run_server(frame_tx).await {
            eprintln!("[SERVER ERROR] WebTransport QUIC server error: {}", e);
        }
    });

    println!("\n[SERVER READY] WebTransport QUIC UDP Server running on port 4433/4434.");
    println!(" Displaying IPv4 Dashboard with Real-Time Clock on HDMI.");
    println!(" Waiting for incoming H.264 video streams from remote client...");

    let mut clock_interval = tokio::time::interval(Duration::from_secs(1));
    let mut current_buf_idx = 0;
    let mut is_streaming_active = false;

    loop {
        tokio::select! {
            // Update real-time clock on IP screen every second when idle
            _ = clock_interval.tick() => {
                if !is_streaming_active {
                    let (back_ptr, back_size, back_fb) = if current_buf_idx == 0 {
                        (ptr_1, size_1, fb_handle_1)
                    } else {
                        (ptr_0, size_0, fb_handle_0)
                    };

                    if !back_ptr.is_null() && back_size > 0 {
                        let slice = unsafe { std::slice::from_raw_parts_mut(back_ptr as *mut u32, back_size / 4) };
                        text::draw_ip_dashboard_argb(slice, screen_w, screen_h, &active_ips);
                        let _ = card.page_flip(crtc_handle, back_fb, drm::control::PageFlipFlags::EVENT, None);
                        current_buf_idx = 1 - current_buf_idx;
                    }
                }
            }

            // Handle incoming video stream frames
            Some(h264_payload) = frame_rx.recv() => {
                if !is_streaming_active {
                    if !ptr_0.is_null() && size_0 > 0 {
                        let slice_0 = unsafe { std::slice::from_raw_parts_mut(ptr_0 as *mut u32, size_0 / 4) };
                        v4l2_decoder::init_player_ui(slice_0, screen_w, screen_h);
                    }
                    if !ptr_1.is_null() && size_1 > 0 {
                        let slice_1 = unsafe { std::slice::from_raw_parts_mut(ptr_1 as *mut u32, size_1 / 4) };
                        v4l2_decoder::init_player_ui(slice_1, screen_w, screen_h);
                    }
                    is_streaming_active = true;
                }

                let (back_ptr, back_size, back_fb) = if current_buf_idx == 0 {
                    (ptr_1, size_1, fb_handle_1)
                } else {
                    (ptr_0, size_0, fb_handle_0)
                };

                if let Ok(rendered) = v4l2_decoder::process_and_render_h264_frame(
                    &h264_payload,
                    back_ptr,
                    back_size,
                    screen_w,
                    screen_h,
                    drm_kms::DRM_FORMAT_XRGB8888,
                ) {
                    if rendered {
                        let _ = card.page_flip(
                            crtc_handle,
                            back_fb,
                            drm::control::PageFlipFlags::EVENT,
                            None,
                        );
                        current_buf_idx = 1 - current_buf_idx;
                    }
                }
            }
        }
    }
}
