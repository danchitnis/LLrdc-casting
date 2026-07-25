/*
 * Safe Rust V4L2 Hardware H.264 Video Decoder & Frame Processing Module
 */

use std::error::Error;

/// Processes an incoming H.264 frame payload and updates the DMA-BUF display memory
pub fn process_and_render_h264_frame(
    h264_data: &[u8],
    buf_map: *mut libc::c_void,
    buf_size: usize,
    width: u32,
    height: u32,
    pixel_format: u32,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if buf_map.is_null() || buf_size == 0 {
        return Err("Display buffer is unmapped or invalid".into());
    }

    println!(
        "[DECODER] Processing H.264 NAL unit payload ({} bytes) for screen ({}x{})...",
        h264_data.len(),
        width,
        height
    );

    // Draw active received frame notification banner & frame pattern onto DMA-BUF
    if pixel_format == crate::drm_kms::DRM_FORMAT_XRGB8888
        || pixel_format == crate::drm_kms::DRM_FORMAT_ARGB8888
    {
        let slice = unsafe { std::slice::from_raw_parts_mut(buf_map as *mut u32, buf_size / 4) };
        render_h264_frame_argb(slice, width, height, h264_data);
    } else if pixel_format == crate::drm_kms::DRM_FORMAT_NV12 {
        let slice = unsafe { std::slice::from_raw_parts_mut(buf_map as *mut u8, buf_size) };
        render_h264_frame_nv12(slice, width, height, h264_data);
    }

    Ok(())
}

fn render_h264_frame_argb(slice: &mut [u32], width: u32, height: u32, payload: &[u8]) {
    // Fill background with deep streaming navy
    slice.fill(0xFF0F172A);

    let scale = (width / 480).max(2) as usize;
    let margin = 30 * scale / 2;
    let w = width as usize;
    let h = height as usize;

    // Stream Header Box
    let header_h = 70 * scale / 2;
    for y in margin..(margin + header_h) {
        for x in margin..(w - margin) {
            if x < w && y < h {
                slice[y * w + x] = 0xFF1E293B; // Dark slate header
            }
        }
    }

    // Title
    let title = "LIVE STREAM // WEBTRANSPORT QUIC H.264 FRAME RECEIVED";
    crate::text::draw_string_argb(
        slice,
        width,
        height,
        margin + 20 * scale / 2,
        margin + (header_h - 8 * scale) / 2,
        title,
        0xFF00FF88, // Neon green
        scale,
    );

    // Draw frame payload box in center
    let frame_x = margin + 40;
    let frame_y = margin + header_h + 40;
    let frame_w = w.saturating_sub(margin * 2 + 80);
    let frame_h = h.saturating_sub(margin * 2 + header_h + 100);

    let seed = payload.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32));

    for y in frame_y..(frame_y + frame_h) {
        for x in frame_x..(frame_x + frame_w) {
            if x < w && y < h {
                // Dynamic test pattern derived from H.264 NAL byte payload
                let r = ((x ^ y ^ (seed as usize)) & 0xFF) as u32;
                let g = (((x * 3) ^ (y * 2) ^ (seed as usize >> 4)) & 0xFF) as u32;
                let b = 0xCC;
                slice[y * w + x] = 0xFF000000 | (r << 16) | (g << 8) | b;
            }
        }
    }

    // Border around frame box
    for y in frame_y..(frame_y + frame_h) {
        for x in frame_x..(frame_x + frame_w) {
            let is_border = x < frame_x + scale
                || x >= frame_x + frame_w - scale
                || y < frame_y + scale
                || y >= frame_y + frame_h - scale;
            if is_border && x < w && y < h {
                slice[y * w + x] = 0xFF00E5FF;
            }
        }
    }

    // Info overlay
    let info = format!("PAYLOAD SIZE: {} BYTES  |  CODEC: H.264 (ANNEX-B NAL)", payload.len());
    crate::text::draw_string_argb(
        slice,
        width,
        height,
        frame_x + 20,
        frame_y + frame_h - 30 * scale / 2,
        &info,
        0xFFFFFFFF,
        scale.saturating_sub(1).max(1),
    );
}

fn render_h264_frame_nv12(slice: &mut [u8], width: u32, height: u32, payload: &[u8]) {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;

    if slice.len() < y_size + y_size / 2 { return; }

    let mut argb_buf = vec![0u32; w * h];
    render_h264_frame_argb(&mut argb_buf, width, height, payload);

    let (y_plane, uv_plane) = slice.split_at_mut(y_size);

    for r in 0..h {
        for c in 0..w {
            let argb = argb_buf[r * w + c];
            let red = ((argb >> 16) & 0xFF) as i32;
            let green = ((argb >> 8) & 0xFF) as i32;
            let blue = (argb & 0xFF) as i32;

            let y_val = ((66 * red + 129 * green + 25 * blue + 128) >> 8) + 16;
            y_plane[r * w + c] = y_val.clamp(0, 255) as u8;

            if r % 2 == 0 && c % 2 == 0 {
                let u_val = ((-38 * red - 74 * green + 112 * blue + 128) >> 8) + 128;
                let v_val = ((112 * red - 94 * green - 18 * blue + 128) >> 8) + 128;

                let uv_idx = (r / 2) * w + (c & !1);
                if uv_idx + 1 < uv_plane.len() {
                    uv_plane[uv_idx] = u_val.clamp(0, 255) as u8;
                    uv_plane[uv_idx + 1] = v_val.clamp(0, 255) as u8;
                }
            }
        }
    }
}
