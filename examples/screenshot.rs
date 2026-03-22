//! Generate a PNG screenshot emulating a terminal with an inline shrimp sprite.
//!
//! Renders frame 0 of the scampii sprite at scale=1 composited onto a dark
//! terminal background with simple bitmapped text, producing a self-contained
//! PNG with no external image crates.
//!
//! ```sh
//! cargo run --example screenshot --no-default-features
//! ```

use scampii::raster::{rasterise, CROP_H, CROP_W};
use scampii::{Theme, FRAMES};

// ---------------------------------------------------------------------------
// Minimal PNG encoder (mirrors the private one in iterm.rs)
// ---------------------------------------------------------------------------

const CRC32_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0u32;
    while i < 256 {
        let mut crc = i;
        let mut j = 0;
        while j < 8 {
            if crc & 1 != 0 {
                crc = 0xEDB8_8320 ^ (crc >> 1);
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        table[i as usize] = crc;
        i += 1;
    }
    table
};

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

fn write_be32(out: &mut Vec<u8>, val: u32) {
    out.extend_from_slice(&val.to_be_bytes());
}

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    write_be32(out, data.len() as u32);
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let crc_start = out.len() - data.len() - 4;
    let crc = crc32(&out[crc_start..]);
    write_be32(out, crc);
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let mut out = Vec::new();
    // PNG signature
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type: RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    write_png_chunk(&mut out, b"IHDR", &ihdr);

    // Build raw scanlines
    let row_bytes = (width as usize) * 4;
    let mut payload = Vec::with_capacity((1 + row_bytes) * (height as usize));
    for y in 0..height as usize {
        payload.push(0); // filter: None
        let start = y * row_bytes;
        payload.extend_from_slice(&rgba[start..start + row_bytes]);
    }

    // DEFLATE stored blocks
    let mut deflate_data = Vec::with_capacity(payload.len() + 64);
    deflate_data.push(0x78); // CMF
    deflate_data.push(0x01); // FLG

    let mut offset = 0;
    while offset < payload.len() {
        let remaining = payload.len() - offset;
        let block_len = remaining.min(65535);
        let is_final = offset + block_len >= payload.len();
        deflate_data.push(if is_final { 0x01 } else { 0x00 });
        let len16 = block_len as u16;
        let nlen16 = !len16;
        deflate_data.extend_from_slice(&len16.to_le_bytes());
        deflate_data.extend_from_slice(&nlen16.to_le_bytes());
        deflate_data.extend_from_slice(&payload[offset..offset + block_len]);
        offset += block_len;
    }

    let adler = adler32(&payload);
    deflate_data.extend_from_slice(&adler.to_be_bytes());

    write_png_chunk(&mut out, b"IDAT", &deflate_data);
    write_png_chunk(&mut out, b"IEND", &[]);

    out
}

// ---------------------------------------------------------------------------
// Simple 5x7 bitmap font (uppercase + digits + punctuation)
// ---------------------------------------------------------------------------

/// Each glyph is 5 columns x 7 rows, stored as 7 bytes (each byte = 5 bits,
/// MSB = leftmost column).
const FONT: [(u8, [u8; 7]); 44] = [
    // Letters A-Z
    (b'A', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    (b'B', [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110]),
    (b'C', [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110]),
    (b'D', [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110]),
    (b'E', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111]),
    (b'F', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000]),
    (b'G', [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110]),
    (b'H', [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    (b'I', [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    (b'J', [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100]),
    (b'K', [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001]),
    (b'L', [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111]),
    (b'M', [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001]),
    (b'N', [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001]),
    (b'O', [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
    (b'P', [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000]),
    (b'Q', [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101]),
    (b'R', [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001]),
    (b'S', [0b01110, 0b10001, 0b10000, 0b01110, 0b00001, 0b10001, 0b01110]),
    (b'T', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
    (b'U', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
    (b'V', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100]),
    (b'W', [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001]),
    (b'X', [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001]),
    (b'Y', [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100]),
    (b'Z', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111]),
    // Digits
    (b'0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
    (b'1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    (b'2', [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111]),
    (b'3', [0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110]),
    (b'4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
    (b'5', [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
    (b'6', [0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
    (b'7', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000]),
    (b'8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
    (b'9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110]),
    // Punctuation
    (b'.', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100]),
    (b'!', [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100]),
    (b'$', [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100]),
    (b' ', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
    (b'>', [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000]),
    (b'-', [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000]),
    (b'[', [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110]),
    (b']', [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110]),
];

fn glyph(ch: u8) -> Option<&'static [u8; 7]> {
    // Case-insensitive: lowercase -> uppercase
    let ch = if ch >= b'a' && ch <= b'z' {
        ch - 32
    } else {
        ch
    };
    FONT.iter().find(|(c, _)| *c == ch).map(|(_, g)| g)
}

// ---------------------------------------------------------------------------
// Canvas helper
// ---------------------------------------------------------------------------

struct Canvas {
    width: usize,
    height: usize,
    /// Row-major RGBA pixels.
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize, bg: [u8; 4]) -> Self {
        let mut pixels = vec![0u8; width * height * 4];
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.copy_from_slice(&bg);
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Set a single pixel (bounds-checked).
    fn set(&mut self, x: usize, y: usize, rgba: [u8; 4]) {
        if x < self.width && y < self.height {
            let idx = (y * self.width + x) * 4;
            self.pixels[idx..idx + 4].copy_from_slice(&rgba);
        }
    }

    /// Alpha-composite `src` over `dst` (premultiply-aware).
    fn blend(&mut self, x: usize, y: usize, rgba: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = (y * self.width + x) * 4;
        let sa = rgba[3] as u32;
        if sa == 0 {
            return;
        }
        if sa == 255 {
            self.pixels[idx..idx + 4].copy_from_slice(&rgba);
            return;
        }
        let da = self.pixels[idx + 3] as u32;
        let inv = 255 - sa;
        for c in 0..3 {
            let sc = rgba[c] as u32;
            let dc = self.pixels[idx + c] as u32;
            self.pixels[idx + c] = ((sc * sa + dc * inv) / 255) as u8;
        }
        self.pixels[idx + 3] = (sa + da * inv / 255) as u8;
    }

    /// Draw a text string using the 5x7 bitmap font.
    /// `cell_w` and `cell_h` are the terminal cell dimensions (glyph is
    /// centered within the cell). Returns the x position after the last char.
    fn draw_text(
        &mut self,
        text: &str,
        mut x: usize,
        y: usize,
        color: [u8; 4],
        cell_w: usize,
        cell_h: usize,
    ) -> usize {
        let glyph_w = 5;
        let glyph_h = 7;
        let ox = (cell_w.saturating_sub(glyph_w)) / 2; // center glyph in cell
        let oy = (cell_h.saturating_sub(glyph_h)) / 2;

        for &ch in text.as_bytes() {
            if let Some(g) = glyph(ch) {
                for row in 0..glyph_h {
                    for col in 0..glyph_w {
                        if g[row] & (1 << (4 - col)) != 0 {
                            self.set(x + ox + col, y + oy + row, color);
                        }
                    }
                }
            }
            x += cell_w;
        }
        x
    }

    /// Blit an RGBA sprite onto the canvas with alpha blending.
    fn blit_rgba(
        &mut self,
        sprite: &[u8],
        sw: usize,
        sh: usize,
        dst_x: usize,
        dst_y: usize,
    ) {
        for sy in 0..sh {
            for sx in 0..sw {
                let si = (sy * sw + sx) * 4;
                let rgba = [sprite[si], sprite[si + 1], sprite[si + 2], sprite[si + 3]];
                self.blend(dst_x + sx, dst_y + sy, rgba);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let theme = Theme::classic();

    // Rasterise frame 0 at scale=1 -> 24x26 RGBA.
    let (sprite_rgba, sw, sh) = rasterise(&FRAMES[0], &theme, 1);
    let sw = sw as usize; // 24 (CROP_W)
    let sh = sh as usize; // 26 (CROP_H)

    // Add 2px transparent top padding to get 24x28 (matches rasterise_padded).
    let padded_h = sh + 2;
    let mut padded = vec![0u8; sw * padded_h * 4]; // zeros = transparent
    let offset = 2 * sw * 4;
    padded[offset..offset + sprite_rgba.len()].copy_from_slice(&sprite_rgba);

    // Terminal cell geometry.
    let cell_w: usize = 8;
    let cell_h: usize = 16;

    // Image dimensions: wide enough for text + sprite, tall enough for several rows.
    let img_w: usize = 600;
    let img_h: usize = 200;

    let bg = [0x1E, 0x1E, 0x1E, 0xFF]; // dark gray background
    let prompt_color = [0x5F, 0xD3, 0x5F, 0xFF]; // green for $
    let text_color = [0xCC, 0xCC, 0xCC, 0xFF]; // light gray text
    let ready_color = [0x5F, 0xD3, 0x5F, 0xFF]; // green for "ready!"
    let _bracket_color = [0x88, 0x88, 0x88, 0xFF]; // dim for brackets
    let bar_color = [0x44, 0x44, 0x44, 0xFF]; // title bar

    let mut canvas = Canvas::new(img_w, img_h, bg);

    // --- Title bar ---
    for y in 0..20 {
        for x in 0..img_w {
            canvas.set(x, y, bar_color);
        }
    }
    // Three window dots (close/minimize/maximize)
    let dots: [(usize, [u8; 4]); 3] = [
        (12, [0xFF, 0x5F, 0x56, 0xFF]), // red
        (26, [0xFF, 0xBD, 0x2E, 0xFF]), // yellow
        (40, [0x27, 0xC9, 0x3F, 0xFF]), // green
    ];
    for &(cx, color) in &dots {
        for dy in -4i32..=4 {
            for dx in -4i32..=4 {
                if dx * dx + dy * dy <= 16 {
                    canvas.set((cx as i32 + dx) as usize, (10i32 + dy) as usize, color);
                }
            }
        }
    }

    // Title text
    canvas.draw_text("scampii", 200, 6, text_color, cell_w, 8);

    // --- Terminal content area (below title bar) ---
    let content_y = 24; // start of content area

    // Row 0: "$ Loading..."
    let row0_y = content_y + 2;
    let mut x = 8;
    x = canvas.draw_text("$ ", x, row0_y, prompt_color, cell_w, cell_h);
    x = canvas.draw_text("LOADING", x, row0_y, text_color, cell_w, cell_h);
    x = canvas.draw_text("...", x, row0_y, text_color, cell_w, cell_h);
    // Leave a small gap
    x += cell_w;

    // Blit the shrimp sprite inline. The sprite is 24x28, spanning ~2 text rows
    // and ~3 columns. Vertically center it on the two text rows.
    let shrimp_x = x;
    let shrimp_y = row0_y; // top-aligned with the text row
    canvas.blit_rgba(&padded, sw, padded_h, shrimp_x, shrimp_y);

    // After the shrimp: advance by ~3 columns (24px)
    x = shrimp_x + sw + cell_w;

    // "READY!"
    canvas.draw_text("READY!", x, row0_y, ready_color, cell_w, cell_h);

    // Row 2 (below the shrimp area): additional output lines
    let row2_y = row0_y + cell_h * 2 + 4;
    canvas.draw_text("  ALL SYSTEMS GO.", 8, row2_y, text_color, cell_w, cell_h);

    // Row 3: another prompt line
    let row3_y = row2_y + cell_h + 2;
    canvas.draw_text("  3 TESTS PASSED.", 8, row3_y, ready_color, cell_w, cell_h);

    // Row 4: empty prompt with blinking cursor
    let row4_y = row3_y + cell_h + 2;
    let mut x2 = 8;
    x2 = canvas.draw_text("$ ", x2, row4_y, prompt_color, cell_w, cell_h);
    // Blinking cursor block
    for dy in 0..cell_h {
        for dx in 0..cell_w {
            canvas.set(x2 + dx, row4_y + dy, text_color);
        }
    }

    // Encode as PNG and write to disk.
    let png_data = encode_png(&canvas.pixels, img_w as u32, img_h as u32);
    let out_path = "screenshot.png";
    std::fs::write(out_path, &png_data).expect("failed to write screenshot.png");

    println!(
        "Wrote {} ({img_w}x{img_h}, {} bytes)",
        out_path,
        png_data.len()
    );
    println!("Sprite dimensions: {sw}x{padded_h} (CROP_W={CROP_W}, CROP_H={CROP_H} + 2px pad)");
}
