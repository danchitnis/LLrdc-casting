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
    let active_format = if screen_w >= 3840 {
        drm_kms::DRM_FORMAT_NV12
    } else {
        drm_kms::DRM_FORMAT_XRGB8888
    };

    println!("\n[STEP 2] Allocating Double-Buffered DRM PRIME frame memory ({}x{})...", screen_w, screen_h);

    let (fd_0, pitch_0, size_0, ptr_0) = drm_kms::allocate_prime_dmabuf(drm_raw_fd, screen_w, screen_h)?;
    let fb_handle_0 = drm_kms::import_dmabuf_and_add_fb(drm_raw_fd, fd_0, screen_w, screen_h, pitch_0, active_format)?;
    println!("[DMA-BUF 0] Buffer 0 ready: fd={}, FB={:?}", fd_0, fb_handle_0);

    let (fd_1, pitch_1, size_1, ptr_1) = drm_kms::allocate_prime_dmabuf(drm_raw_fd, screen_w, screen_h)?;
    let fb_handle_1 = drm_kms::import_dmabuf_and_add_fb(drm_raw_fd, fd_1, screen_w, screen_h, pitch_1, active_format)?;
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
        if active_format == drm_kms::DRM_FORMAT_NV12 {
            let slice_0 = unsafe { std::slice::from_raw_parts_mut(ptr_0 as *mut u8, size_0) };
            text::draw_ip_dashboard_nv12(slice_0, screen_w, screen_h, &active_ips);
        } else {
            let slice_0 = unsafe { std::slice::from_raw_parts_mut(ptr_0 as *mut u32, size_0 / 4) };
            text::draw_ip_dashboard_argb(slice_0, screen_w, screen_h, &active_ips);
        }
    }
    if !ptr_1.is_null() && size_1 > 0 {
        if active_format == drm_kms::DRM_FORMAT_NV12 {
            let slice_1 = unsafe { std::slice::from_raw_parts_mut(ptr_1 as *mut u8, size_1) };
            text::draw_ip_dashboard_nv12(slice_1, screen_w, screen_h, &active_ips);
        } else {
            let slice_1 = unsafe { std::slice::from_raw_parts_mut(ptr_1 as *mut u32, size_1 / 4) };
            text::draw_ip_dashboard_argb(slice_1, screen_w, screen_h, &active_ips);
        }
    }

    // -------------------------------------------------------------
    // Step 3: Commit Initial CRTC Mode
    // -------------------------------------------------------------
    println!("\n[STEP 3] Executing Initial DRM KMS Modeset on CRTC {:?}...", crtc_handle);
    drm_kms::set_display_mode(&card, crtc_handle, fb_handle_0, conn_handle, mode)?;

    // -------------------------------------------------------------
    // Step 4: Start WebTransport Server & Video Stream Receiver
    // -------------------------------------------------------------
    let (frame_tx, mut frame_rx) = mpsc::channel::<v4l2_decoder::VideoFrame>(32);

    tokio::spawn(async move {
        if let Err(e) = webtransport_server::run_server(frame_tx).await {
            eprintln!("[SERVER ERROR] WebTransport QUIC server error: {}", e);
        }
    });

    println!("\n[SERVER READY] WebTransport QUIC UDP Server running on port 4433/4434.");
    println!(" Displaying IPv4 Dashboard with Real-Time Clock on HDMI.");
    println!(" Waiting for incoming video streams from remote client...");

    use std::time::Instant;

    let mut clock_interval = tokio::time::interval(Duration::from_secs(1));
    let mut last_render_time = Instant::now();
    let mut frame_time_deltas: Vec<f32> = Vec::with_capacity(30);

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
                        if active_format == drm_kms::DRM_FORMAT_NV12 {
                            let slice = unsafe { std::slice::from_raw_parts_mut(back_ptr as *mut u8, back_size) };
                            text::draw_ip_dashboard_nv12(slice, screen_w, screen_h, &active_ips);
                        } else {
                            let slice = unsafe { std::slice::from_raw_parts_mut(back_ptr as *mut u32, back_size / 4) };
                            text::draw_ip_dashboard_argb(slice, screen_w, screen_h, &active_ips);
                        }
                        let _ = card.page_flip(crtc_handle, back_fb, drm::control::PageFlipFlags::empty(), None);
                        current_buf_idx = 1 - current_buf_idx;
                    }
                }
            }

            // Immediately render every fully reassembled video frame in strict FIFO sequence
            Some(video_frame) = frame_rx.recv() => {
                let now = Instant::now();
                let delta_ms = now.duration_since(last_render_time).as_secs_f32() * 1000.0;
                last_render_time = now;

                if !is_streaming_active || delta_ms > 1000.0 {
                    is_streaming_active = true;
                    frame_time_deltas.clear();
                    v4l2_decoder::reset_decoder_pipeline();
                } else if delta_ms > 0.1 && delta_ms < 500.0 {
                    frame_time_deltas.push(delta_ms);
                    if frame_time_deltas.len() > 30 {
                        frame_time_deltas.remove(0);
                    }
                }

                let avg_delta = frame_time_deltas.iter().sum::<f32>() / frame_time_deltas.len() as f32;
                let measured_fps = if avg_delta > 0.0 { 1000.0 / avg_delta } else { 30.0 };
                let jitter_ms = (frame_time_deltas.iter().map(|&d| (d - avg_delta).powi(2)).sum::<f32>() / frame_time_deltas.len() as f32).sqrt();

                let (back_ptr, back_size, back_fb) = if current_buf_idx == 0 {
                    (ptr_1, size_1, fb_handle_1)
                } else {
                    (ptr_0, size_0, fb_handle_0)
                };

                if let Ok(_) = v4l2_decoder::render_frame_to_buffer(
                    &video_frame,
                    back_ptr,
                    back_size,
                    screen_w,
                    screen_h,
                    active_format,
                    &active_ips,
                    measured_fps,
                    jitter_ms,
                ) {
                    let mut res = card.page_flip(
                        crtc_handle,
                        back_fb,
                        drm::control::PageFlipFlags::ASYNC,
                        None,
                    ).or_else(|_| {
                        card.page_flip(
                            crtc_handle,
                            back_fb,
                            drm::control::PageFlipFlags::empty(),
                            None,
                        )
                    });

                    if res.is_err() {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                        res = card.page_flip(
                            crtc_handle,
                            back_fb,
                            drm::control::PageFlipFlags::empty(),
                            None,
                        );
                    }

                    if res.is_ok() {
                        current_buf_idx = 1 - current_buf_idx;
                    }
                }
            }
        }
    }
}
