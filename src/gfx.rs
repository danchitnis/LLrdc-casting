/*
 * 100% Safe Rust Graphics Module: Geometric shape rendering
 */

#![forbid(unsafe_code)]

/// Draw concentric rectangles into an ARGB8888 frame buffer slice
pub fn draw_rectangles_argb(buf: &mut [u32], width: u32, height: u32) {
    let bg_color = 0xFF1E1E24;
    buf.fill(bg_color);

    let rect_x = (width / 4) as usize;
    let rect_y = (height / 4) as usize;
    let rect_w = (width / 2) as usize;
    let rect_h = (height / 2) as usize;
    let mut border_thick = (width / 160) as usize;
    if border_thick < 4 { border_thick = 4; }

    let border_color = 0xFF00FFCC; // Cyan
    let fill_color   = 0xFFFF3366; // Coral Pink

    let inner_x = rect_x + border_thick;
    let inner_y = rect_y + border_thick;
    let inner_w = rect_w.saturating_sub(2 * border_thick);
    let inner_h = rect_h.saturating_sub(2 * border_thick);

    for y in rect_y..(rect_y + rect_h) {
        for x in rect_x..(rect_x + rect_w) {
            if y < height as usize && x < width as usize {
                let idx = y * (width as usize) + x;
                if idx < buf.len() {
                    if x >= inner_x && x < inner_x + inner_w && y >= inner_y && y < inner_y + inner_h {
                        buf[idx] = fill_color;
                    } else {
                        buf[idx] = border_color;
                    }
                }
            }
        }
    }

    let box2_w = rect_w / 3;
    let box2_h = rect_h / 3;
    let box2_x = rect_x + (rect_w - box2_w) / 2;
    let box2_y = rect_y + (rect_h - box2_h) / 2;
    let box2_color = 0xFFFFCC00; // Bright yellow

    for y in box2_y..(box2_y + box2_h) {
        for x in box2_x..(box2_x + box2_w) {
            if y < height as usize && x < width as usize {
                let idx = y * (width as usize) + x;
                if idx < buf.len() {
                    buf[idx] = box2_color;
                }
            }
        }
    }
}

/// Draw concentric rectangles into an NV12 YUV frame buffer slice
pub fn draw_rectangles_nv12(buf: &mut [u8], width: u32, height: u32) {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;

    if buf.len() < y_size + y_size / 2 { return; }

    let (y_plane, uv_plane) = buf.split_at_mut(y_size);

    y_plane.fill(40);
    uv_plane.fill(128);

    let rect_x = w / 4;
    let rect_y = h / 4;
    let rect_w = w / 2;
    let rect_h = h / 2;
    let mut border_thick = w / 160;
    if border_thick < 4 { border_thick = 4; }

    for r in rect_y..(rect_y + rect_h) {
        for c in rect_x..(rect_x + rect_w) {
            let is_border = r < rect_y + border_thick || r >= rect_y + rect_h - border_thick ||
                            c < rect_x + border_thick || c >= rect_x + rect_w - border_thick;

            if r < h && c < w {
                y_plane[r * w + c] = if is_border { 170 } else { 145 };

                if r % 2 == 0 && c % 2 == 0 {
                    let uv_idx = (r / 2) * w + (c & !1);
                    if uv_idx + 1 < uv_plane.len() {
                        uv_plane[uv_idx]     = if is_border { 166 } else { 54 };
                        uv_plane[uv_idx + 1] = if is_border { 16 }  else { 34 };
                    }
                }
            }
        }
    }
}
