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
pub fn draw_ip_dashboard_argb(buf: &mut [u32], width: u32, height: u32, refresh_hz: u32, ips: &[(String, String)]) {
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

    let header_h = 120 * scale / 2;

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
    let title = "RADXA ROCK 4C+ // RK3399 // DEVICE IPS";
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

    let hdmi_line = format!("HDMI OUTPUT: {}X{} @ {} HZ", width, height, refresh_hz);
    draw_string_argb(
        buf, width, height, box_x + 20 * scale / 2,
        box_y + header_h - 12 * scale, &hdmi_line, 0xFF00E5FF, scale,
    );

    // Real-Time Clock in Corner
    let time_str = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => {
            let secs = dur.as_secs();
            let hours = (secs / 3600 % 24) as u32;
            let mins = (secs / 60 % 60) as u32;
            let s = (secs % 60) as u32;
            format!("{:02}:{:02}:{:02} UTC", hours, mins, s)
        }
        Err(_) => "00:00:00 UTC".to_string(),
    };

    let clock_x = box_x + box_w - (time_str.len() * 8 * scale) - 20 * scale / 2;
    draw_string_argb(
        buf,
        width,
        height,
        clock_x,
        box_y + (header_h - 8 * scale) / 2,
        &time_str,
        0xFFFFCC00, // Bright Gold Clock
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
    let footer_text = format!(
        "HDMI: {}X{} // V4L2 HEVC DECODER -> DMA-BUF -> DRM KMS",
        width, height
    );
    let footer_scale = (scale * 3) / 4;
    draw_string_argb(
        buf,
        width,
        height,
        box_x + 30 * scale / 2,
        box_y + box_h - 25 * scale,
        &footer_text,
        0xFF8899A6,
        footer_scale.max(1),
    );
}

/// Draw character onto NV12 buffer (Y + interleaved UV)
pub fn draw_char_nv12(
    buf: &mut [u8],
    width: u32,
    height: u32,
    start_x: usize,
    start_y: usize,
    ch: char,
    y_val: u8,
    u_val: u8,
    v_val: u8,
    scale: usize,
) {
    let glyph = font::get_glyph(ch);
    let w = width as usize;
    let h = height as usize;
    let uv_offset = w * h;

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
                            let y_idx = y * w + x;
                            if y_idx < uv_offset {
                                buf[y_idx] = y_val;
                            }
                            let uv_idx = uv_offset + (y / 2) * w + (x / 2) * 2;
                            if uv_idx + 1 < buf.len() {
                                buf[uv_idx] = u_val;
                                buf[uv_idx + 1] = v_val;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Draw string onto NV12 buffer
pub fn draw_string_nv12(
    buf: &mut [u8],
    width: u32,
    height: u32,
    x: usize,
    y: usize,
    text: &str,
    y_val: u8,
    u_val: u8,
    v_val: u8,
    scale: usize,
) {
    let mut curr_x = x;
    let char_spacing = 8 * scale;

    for ch in text.chars() {
        draw_char_nv12(buf, width, height, curr_x, y, ch, y_val, u_val, v_val, scale);
        curr_x += char_spacing;
    }
}

/// Render the IP Dashboard onto NV12 buffer
pub fn draw_ip_dashboard_nv12(buf: &mut [u8], width: u32, height: u32, refresh_hz: u32, ips: &[(String, String)]) {
    let w = width as usize;
    let h = height as usize;
    let uv_offset = w * h;

    if buf.len() < uv_offset * 3 / 2 {
        return;
    }

    // Fill background: dark navy/gray Y=16, U=138, V=120
    buf[0..uv_offset].fill(16);
    let mut i = uv_offset;
    while i + 1 < buf.len() {
        buf[i] = 138;     // U
        buf[i + 1] = 120; // V
        i += 2;
    }

    let scale = (width / 480).max(2) as usize;
    let margin = 40 * scale / 2;

    let box_x = margin;
    let box_y = margin;
    let box_w = w.saturating_sub(margin * 2);
    let box_h = h.saturating_sub(margin * 2);

    let header_h = 120 * scale / 2;

    // Header Fill (Y = 35)
    for y in box_y..(box_y + header_h) {
        for x in box_x..(box_x + box_w) {
            if x < w && y < h {
                buf[y * w + x] = 35;
            }
        }
    }

    // Header Title (White: Y=255, U=128, V=128)
    let title = "RADXA ROCK 4C+ // RK3399 // DEVICE IPS";
    draw_string_nv12(
        buf,
        width,
        height,
        box_x + 20 * scale / 2,
        box_y + (header_h - 8 * scale) / 2,
        title,
        255, 128, 128,
        scale,
    );

    let hdmi_line = format!("HDMI OUTPUT: {}X{} @ {} HZ", width, height, refresh_hz);
    draw_string_nv12(
        buf, width, height, box_x + 20 * scale / 2,
        box_y + header_h - 12 * scale, &hdmi_line, 200, 220, 16, scale,
    );

    // Real-Time Clock
    let time_str = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(dur) => {
            let secs = dur.as_secs();
            let hours = (secs / 3600 % 24) as u32;
            let mins = (secs / 60 % 60) as u32;
            let s = (secs % 60) as u32;
            format!("{:02}:{:02}:{:02} UTC", hours, mins, s)
        }
        Err(_) => "00:00:00 UTC".to_string(),
    };

    let clock_x = box_x + box_w - (time_str.len() * 8 * scale) - 20 * scale / 2;
    draw_string_nv12(
        buf,
        width,
        height,
        clock_x,
        box_y + (header_h - 8 * scale) / 2,
        &time_str,
        220, 16, 160, // Gold
        scale,
    );

    // Frame Border (Electric Cyan: Y=200, U=220, V=16)
    let border_thick = scale;
    for y in box_y..(box_y + box_h) {
        for x in box_x..(box_x + box_w) {
            let is_border = x < box_x + border_thick
                || x >= box_x + box_w - border_thick
                || y < box_y + border_thick
                || y >= box_y + box_h - border_thick
                || y == box_y + header_h;
            if is_border && x < w && y < h {
                buf[y * w + x] = 200;
                let uv_idx = uv_offset + (y / 2) * w + (x / 2) * 2;
                if uv_idx + 1 < buf.len() {
                    buf[uv_idx] = 220;
                    buf[uv_idx + 1] = 16;
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
        let (y_c, u_c, v_c) = if iface.starts_with("eth") || iface.starts_with("wlan") || iface.starts_with("end") {
            (180, 32, 32) // Neon Green
        } else if iface == "lo" {
            (220, 16, 160) // Yellow
        } else {
            (200, 220, 16) // Cyan
        };

        draw_string_nv12(
            buf,
            width,
            height,
            box_x + 30 * scale / 2,
            current_y,
            &line_text,
            y_c, u_c, v_c,
            scale,
        );

        current_y += line_height;
        if current_y + line_height > box_y + box_h - 40 * scale {
            break;
        }
    }

    // Footer
    let footer_text = format!(
        "HDMI: {}X{} // V4L2 HEVC DECODER -> DMA-BUF -> DRM KMS",
        width, height
    );
    let footer_scale = (scale * 3) / 4;
    draw_string_nv12(
        buf,
        width,
        height,
        box_x + 30 * scale / 2,
        box_y + box_h - 25 * scale,
        &footer_text,
        160, 128, 128,
        footer_scale.max(1),
    );
}
