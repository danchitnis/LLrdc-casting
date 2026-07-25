/*
 * 100% Safe Rust Text-to-Graphics Module
 * Renders text and dynamic system dashboard (Active IP addresses) onto framebuffers.
 */

#![forbid(unsafe_code)]

/// Simple embedded 8x8 ASCII font bitmap (ASCII 32..=126)
mod font {
    pub fn get_glyph(c: char) -> [u8; 8] {
        let code = c as usize;
        match code {
            32 => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], // Space
            33 => [0x18, 0x3C, 0x3C, 0x18, 0x18, 0x00, 0x18, 0x00], // !
            34 => [0x66, 0x66, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00], // "
            35 => [0x6C, 0x6C, 0xFE, 0x6C, 0xFE, 0x6C, 0x6C, 0x00], // #
            36 => [0x18, 0x3E, 0x60, 0x3C, 0x06, 0x7C, 0x18, 0x00], // $
            37 => [0x00, 0x66, 0x6C, 0x18, 0x30, 0x66, 0x46, 0x00], // %
            38 => [0x3C, 0x66, 0x3C, 0x38, 0x6E, 0x66, 0x3B, 0x00], // &
            39 => [0x18, 0x18, 0x30, 0x00, 0x00, 0x00, 0x00, 0x00], // '
            40 => [0x0C, 0x18, 0x30, 0x30, 0x30, 0x18, 0x0C, 0x00], // (
            41 => [0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x18, 0x30, 0x00], // )
            42 => [0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00], // *
            43 => [0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00], // +
            44 => [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30], // ,
            45 => [0x00, 0x00, 0x00, 0xFE, 0x00, 0x00, 0x00, 0x00], // -
            46 => [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00], // .
            47 => [0x00, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x00, 0x00], // /
            48 => [0x3C, 0x66, 0x6E, 0x76, 0x66, 0x66, 0x3C, 0x00], // 0
            49 => [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00], // 1
            50 => [0x3C, 0x66, 0x06, 0x1C, 0x30, 0x60, 0x7E, 0x00], // 2
            51 => [0x3C, 0x66, 0x06, 0x1C, 0x06, 0x66, 0x3C, 0x00], // 3
            52 => [0x0E, 0x1E, 0x36, 0x66, 0x7F, 0x06, 0x0F, 0x00], // 4
            53 => [0x7E, 0x60, 0x7C, 0x06, 0x06, 0x66, 0x3C, 0x00], // 5
            54 => [0x1C, 0x30, 0x60, 0x7C, 0x66, 0x66, 0x3C, 0x00], // 6
            55 => [0x7E, 0x66, 0x0C, 0x18, 0x18, 0x18, 0x18, 0x00], // 7
            56 => [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0x00], // 8
            57 => [0x3C, 0x66, 0x66, 0x3E, 0x06, 0x0C, 0x38, 0x00], // 9
            58 => [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00], // :
            59 => [0x00, 0x18, 0x18, 0x00, 0x18, 0x18, 0x30, 0x00], // ;
            60 => [0x06, 0x0C, 0x18, 0x30, 0x18, 0x0C, 0x06, 0x00], // <
            61 => [0x00, 0x00, 0x7E, 0x00, 0x7E, 0x00, 0x00, 0x00], // =
            62 => [0x60, 0x30, 0x18, 0x0C, 0x18, 0x30, 0x60, 0x00], // >
            63 => [0x3C, 0x66, 0x06, 0x0C, 0x18, 0x00, 0x18, 0x00], // ?
            64 => [0x3C, 0x66, 0x6E, 0x6E, 0x60, 0x62, 0x3C, 0x00], // @
            65 => [0x18, 0x3C, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x00], // A
            66 => [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0x00], // B
            67 => [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0x00], // C
            68 => [0x78, 0x6C, 0x66, 0x66, 0x66, 0x6C, 0x78, 0x00], // D
            69 => [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x7E, 0x00], // E
            70 => [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0x00], // F
            71 => [0x3C, 0x66, 0x60, 0x6E, 0x66, 0x66, 0x3C, 0x00], // G
            72 => [0x66, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x66, 0x00], // H
            73 => [0x3E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3E, 0x00], // I
            74 => [0x1E, 0x0C, 0x0C, 0x0C, 0x0C, 0x6C, 0x38, 0x00], // J
            75 => [0x66, 0x6C, 0x78, 0x70, 0x78, 0x6C, 0x66, 0x00], // K
            76 => [0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x7E, 0x00], // L
            77 => [0x63, 0x77, 0x7F, 0x6B, 0x63, 0x63, 0x63, 0x00], // M
            78 => [0x66, 0x76, 0x7E, 0x7E, 0x6E, 0x66, 0x66, 0x00], // N
            79 => [0x3C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00], // O
            80 => [0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x00], // P
            81 => [0x3C, 0x66, 0x66, 0x66, 0x6A, 0x6C, 0x36, 0x00], // Q
            82 => [0x7C, 0x66, 0x66, 0x7C, 0x6C, 0x66, 0x66, 0x00], // R
            83 => [0x3C, 0x66, 0x60, 0x3C, 0x06, 0x66, 0x3C, 0x00], // S
            84 => [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00], // T
            85 => [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00], // U
            86 => [0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x18, 0x00], // V
            87 => [0x63, 0x63, 0x63, 0x6B, 0x7F, 0x77, 0x63, 0x00], // W
            88 => [0x66, 0x66, 0x3C, 0x18, 0x3C, 0x66, 0x66, 0x00], // X
            89 => [0x66, 0x66, 0x66, 0x3C, 0x18, 0x18, 0x18, 0x00], // Y
            90 => [0x7E, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x7E, 0x00], // Z
            91 => [0x3C, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3C, 0x00], // [
            92 => [0x00, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x00, 0x00], // \
            93 => [0x3C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x3C, 0x00], // ]
            94 => [0x18, 0x3C, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00], // ^
            95 => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00], // _
            96 => [0x30, 0x18, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x00], // `
            97..=122 => {
                // Map lowercase to uppercase for clean uniform rendering
                get_glyph((code - 32) as u8 as char)
            }
            123 => [0x0E, 0x18, 0x18, 0x70, 0x18, 0x18, 0x0E, 0x00], // {
            124 => [0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x00], // |
            125 => [0x70, 0x18, 0x18, 0x0E, 0x18, 0x18, 0x70, 0x00], // }
            126 => [0x31, 0x6B, 0x46, 0x00, 0x00, 0x00, 0x00, 0x00], // ~
            _ => [0x00, 0x7E, 0x42, 0x42, 0x42, 0x42, 0x7E, 0x00],   // Default box
        }
    }
}

/// Draw character onto ARGB32 buffer
pub fn draw_char_argb(
    buf: &mut [u32],
    width: u32,
    height: u32,
    start_x: usize,
    start_y: usize,
    ch: char,
    color: u32,
    scale: usize,
) {
    let glyph = font::get_glyph(ch);
    let w = width as usize;
    let h = height as usize;

    for row in 0..8 {
        let line = glyph[row];
        for col in 0..8 {
            if (line & (1 << (7 - col))) != 0 {
                let px = start_x + col * scale;
                let py = start_y + row * scale;

                for sy in 0..scale {
                    for sx in 0..scale {
                        let x = px + sx;
                        let y = py + sy;
                        if x < w && y < h {
                            let idx = y * w + x;
                            if idx < buf.len() {
                                buf[idx] = color;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Draw string onto ARGB32 buffer
pub fn draw_string_argb(
    buf: &mut [u32],
    width: u32,
    height: u32,
    x: usize,
    y: usize,
    text: &str,
    color: u32,
    scale: usize,
) {
    let mut curr_x = x;
    let char_spacing = 8 * scale;

    for ch in text.chars() {
        draw_char_argb(buf, width, height, curr_x, y, ch, color, scale);
        curr_x += char_spacing;
    }
}

/// Render the IP Dashboard onto ARGB8888 buffer
pub fn draw_ip_dashboard_argb(buf: &mut [u32], width: u32, height: u32, ips: &[(String, String)]) {
    let bg_color = 0xFF0E1017; // Midnight dark blue/gray
    buf.fill(bg_color);

    let scale = (width / 480).max(2) as usize; // Dynamic text scaling based on screen width
    let margin = 40 * scale / 2;

    let box_x = margin;
    let box_y = margin;
    let box_w = (width as usize).saturating_sub(margin * 2);
    let box_h = (height as usize).saturating_sub(margin * 2);

    let border_color = 0xFF00E5FF; // Electric Cyan
    let header_bg = 0xFF182232;

    let header_h = 80 * scale / 2;

    // Header Fill
    for y in box_y..(box_y + header_h) {
        for x in box_x..(box_x + box_w) {
            if x < width as usize && y < height as usize {
                let idx = y * (width as usize) + x;
                if idx < buf.len() {
                    buf[idx] = header_bg;
                }
            }
        }
    }

    // Header Title
    let title = "RADXA ROCK 5C+ // ACTIVE DEVICE IP ADDRESSES";
    draw_string_argb(
        buf,
        width,
        height,
        box_x + 20 * scale / 2,
        box_y + (header_h - 8 * scale) / 2,
        title,
        0xFFFFFFFF,
        scale,
    );

    // Frame Border
    let border_thick = scale;
    for y in box_y..(box_y + box_h) {
        for x in box_x..(box_x + box_w) {
            let is_border = x < box_x + border_thick
                || x >= box_x + box_w - border_thick
                || y < box_y + border_thick
                || y >= box_y + box_h - border_thick
                || y == box_y + header_h;
            if is_border && x < width as usize && y < height as usize {
                let idx = y * (width as usize) + x;
                if idx < buf.len() {
                    buf[idx] = border_color;
                }
            }
        }
    }

    // IP List Rendering
    let start_content_y = box_y + header_h + 30 * scale / 2;
    let line_height = 14 * scale;

    let mut current_y = start_content_y;

    for (i, (iface, ip)) in ips.iter().enumerate() {
        let line_text = format!("[{:02}] {:<12} : {}", i + 1, iface, ip);
        let color = if iface.starts_with("eth") || iface.starts_with("wlan") || iface.starts_with("end") {
            0xFF00FF88 // Neon Green for physical interfaces
        } else if iface == "lo" {
            0xFFFFCC00 // Yellow for loopback
        } else {
            0xFF00E5FF // Cyan for docker/virtual
        };

        draw_string_argb(
            buf,
            width,
            height,
            box_x + 30 * scale / 2,
            current_y,
            &line_text,
            color,
            scale,
        );

        current_y += line_height;
        if current_y + line_height > box_y + box_h - 40 * scale {
            break;
        }
    }

    // Footer
    let footer_text = "HARDWARE PIPELINE: V4L2 DECODER -> DMA-BUF FD -> DRM ATOMIC COMMIT";
    let footer_scale = (scale * 3) / 4;
    draw_string_argb(
        buf,
        width,
        height,
        box_x + 30 * scale / 2,
        box_y + box_h - 25 * scale,
        footer_text,
        0xFF8899A6,
        footer_scale.max(1),
    );
}

/// Render IP Dashboard onto NV12 YUV buffer
pub fn draw_ip_dashboard_nv12(buf: &mut [u8], width: u32, height: u32, ips: &[(String, String)]) {
    let w = width as usize;
    let h = height as usize;
    let y_size = w * h;

    if buf.len() < y_size + y_size / 2 {
        return;
    }

    // Convert dashboard to ARGB first, then translate into NV12 Y UV planes
    let mut argb_buf = vec![0u32; w * h];
    draw_ip_dashboard_argb(&mut argb_buf, width, height, ips);

    let (y_plane, uv_plane) = buf.split_at_mut(y_size);

    for r in 0..h {
        for c in 0..w {
            let argb = argb_buf[r * w + c];
            let red = ((argb >> 16) & 0xFF) as i32;
            let green = ((argb >> 8) & 0xFF) as i32;
            let blue = (argb & 0xFF) as i32;

            // BT.601 RGB to YUV Conversion
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
