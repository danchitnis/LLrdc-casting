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
