//! 256-color ANSI palette lookup table and SGR formatting.

/// 256-color ANSI palette lookup table mapping 8-bit index (0..=255) to RGB (r, g, b) triples.
///
/// - Indices 0..15: Standard and bright system ANSI colors.
/// - Indices 16..231: 6x6x6 color cube.
/// - Indices 232..255: 24 grayscale levels.
pub static COLOR_256_LUT: [(u8, u8, u8); 256] = generate_256_lut();

const fn generate_256_lut() -> [(u8, u8, u8); 256] {
    let mut lut = [(0, 0, 0); 256];

    // Standard 16 ANSI colors
    lut[0] = (0x00, 0x00, 0x00);   // Black
    lut[1] = (0x80, 0x00, 0x00);   // Red
    lut[2] = (0x00, 0x80, 0x00);   // Green
    lut[3] = (0x80, 0x80, 0x00);   // Yellow
    lut[4] = (0x00, 0x00, 0x80);   // Blue
    lut[5] = (0x80, 0x00, 0x80);   // Magenta
    lut[6] = (0x00, 0x80, 0x80);   // Cyan
    lut[7] = (0xc0, 0xc0, 0xc0);   // White
    lut[8] = (0x80, 0x80, 0x80);   // Bright Black (Gray)
    lut[9] = (0xff, 0x00, 0x00);   // Bright Red
    lut[10] = (0x00, 0xff, 0x00);  // Bright Green
    lut[11] = (0xff, 0xff, 0x00);  // Bright Yellow
    lut[12] = (0x00, 0x00, 0xff);  // Bright Blue
    lut[13] = (0xff, 0x00, 0xff);  // Bright Magenta
    lut[14] = (0x00, 0xff, 0xff);  // Bright Cyan
    lut[15] = (0xff, 0xff, 0xff);  // Bright White

    // 6x6x6 Color Cube (16..231)
    let steps: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let mut i = 0;
    while i < 216 {
        let r = steps[(i / 36) % 6];
        let g = steps[(i / 6) % 6];
        let b = steps[i % 6];
        lut[16 + i] = (r, g, b);
        i += 1;
    }

    // 24 Grayscale levels (232..255)
    let mut g = 0;
    while g < 24 {
        let v = 8 + g * 10;
        lut[232 + g] = (v as u8, v as u8, v as u8);
        g += 1;
    }

    lut
}

/// Convert a 256-color palette index (0..=255) to its (R, G, B) tuple using the LUT.
#[must_use]
pub fn color_256_to_rgb(code: u8) -> (u8, u8, u8) {
    COLOR_256_LUT[code as usize]
}

/// Find the closest 256-color palette index for a given RGB triple using Euclidean distance in RGB space.
#[must_use]
pub fn rgb_to_256_color(r: u8, g: u8, b: u8) -> u8 {
    let mut best_idx = 0u8;
    let mut best_dist = u32::MAX;

    let mut i = 0;
    while i < 256 {
        let (lr, lg, lb) = COLOR_256_LUT[i];
        let dr = (r as i32) - (lr as i32);
        let dg = (g as i32) - (lg as i32);
        let db = (b as i32) - (lb as i32);
        let dist = (dr * dr + dg * dg + db * db) as u32;

        if dist < best_dist {
            best_dist = dist;
            best_idx = i as u8;
            if dist == 0 {
                break;
            }
        }
        i += 1;
    }

    best_idx
}

/// Format text with 256-color foreground ANSI SGR escape sequence (`\x1b[38;5;Nm... \x1b[0m`).
#[must_use]
pub fn format_256_fg(code: u8, text: &str) -> String {
    format!("\x1b[38;5;{}m{}\x1b[0m", code, text)
}

/// Format text with 256-color background ANSI SGR escape sequence (`\x1b[48;5;Nm... \x1b[0m`).
#[must_use]
pub fn format_256_bg(code: u8, text: &str) -> String {
    format!("\x1b[48;5;{}m{}\x1b[0m", code, text)
}
