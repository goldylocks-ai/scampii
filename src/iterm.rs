//! iTerm2 inline image protocol (OSC 1337) renderer.
//!
//! Rasterises a [`PackedFrame`] to RGBA pixels, encodes as PNG,
//! base64-encodes the result, and wraps it in the
//! `\x1b]1337;File=inline=1;...` escape sequence.

use std::io::Write;

use crate::error::ScampiiError;
use crate::frame::PackedFrame;
use crate::raster::{base64_encode, rasterise_padded, MAX_SCALE};
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// CRC-32 (ISO 3309 / PNG)
// ---------------------------------------------------------------------------

/// CRC-32 lookup table (polynomial 0xEDB88320, reflected).
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

/// Compute the CRC-32 checksum of `data`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = CRC32_TABLE[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

// ---------------------------------------------------------------------------
// Adler-32
// ---------------------------------------------------------------------------

/// Compute the Adler-32 checksum of `data`.
fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    // Process in chunks to avoid overflow. The modulus base is 65521.
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

// ---------------------------------------------------------------------------
// PNG encoder (stored DEFLATE, no compression)
// ---------------------------------------------------------------------------

/// Write a 4-byte big-endian u32.
fn write_be32(out: &mut Vec<u8>, val: u32) {
    out.extend_from_slice(&val.to_be_bytes());
}

/// Write a PNG chunk: length + type + data + CRC.
fn write_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    write_be32(out, data.len() as u32);
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    // CRC covers type + data
    let crc_start = out.len() - data.len() - 4;
    let crc = crc32(&out[crc_start..]);
    write_be32(out, crc);
}

/// Encode an RGBA image as a minimal PNG (stored DEFLATE blocks).
fn encode_png(rgba: &[u8], width: u32, height: u32, out: &mut Vec<u8>) {
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
    write_png_chunk(out, b"IHDR", &ihdr);

    // Build raw (unfiltered) scanlines: for each row, a filter byte (0) + RGBA data
    let row_bytes = (width as usize) * 4;
    let raw_len = (1 + row_bytes) * (height as usize);

    // DEFLATE stored blocks wrapping the raw scanline data.
    // Each stored block can hold at most 65535 bytes.
    let mut deflate_data = Vec::with_capacity(raw_len + 64);

    // zlib header: CM=8, CINFO=7, FCHECK so header % 31 == 0
    let cmf: u8 = 0x78; // CM=8 (deflate), CINFO=7 (32K window)
    let flg: u8 = 0x01; // FCHECK=1, no dict, FLEVEL=0
    deflate_data.push(cmf);
    deflate_data.push(flg);

    // Build the uncompressed payload (filter-byte + row data)
    let mut payload = Vec::with_capacity(raw_len);
    for y in 0..height as usize {
        payload.push(0); // filter: None
        let start = y * row_bytes;
        payload.extend_from_slice(&rgba[start..start + row_bytes]);
    }

    // Emit stored DEFLATE blocks (max 65535 bytes each)
    let mut offset = 0;
    while offset < payload.len() {
        let remaining = payload.len() - offset;
        let block_len = remaining.min(65535);
        let is_final = offset + block_len >= payload.len();
        deflate_data.push(if is_final { 0x01 } else { 0x00 }); // BFINAL + BTYPE=00
        let len16 = block_len as u16;
        let nlen16 = !len16;
        deflate_data.extend_from_slice(&len16.to_le_bytes());
        deflate_data.extend_from_slice(&nlen16.to_le_bytes());
        deflate_data.extend_from_slice(&payload[offset..offset + block_len]);
        offset += block_len;
    }

    // Adler-32 of uncompressed data
    let adler = adler32(&payload);
    deflate_data.extend_from_slice(&adler.to_be_bytes());

    write_png_chunk(out, b"IDAT", &deflate_data);

    // IEND
    write_png_chunk(out, b"IEND", &[]);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a frame using the iTerm2 inline image protocol (OSC 1337).
///
/// Each sprite pixel is rendered as `scale x scale` screen pixels. The frame
/// is cropped to the tight bounding box, rasterised to RGBA, encoded as PNG,
/// base64-encoded, and wrapped in the iTerm2 escape sequence.
///
/// `buf` is a reusable scratch buffer to avoid per-frame allocation.
pub fn draw_iterm<Out: Write>(
    buf: &mut Vec<u8>,
    out: &mut Out,
    frame: &PackedFrame,
    theme: &Theme,
    scale: u8,
) -> Result<(), ScampiiError> {
    let scale = scale.clamp(1, MAX_SCALE);
    let (rgba, w, h) = rasterise_padded(frame, theme, scale);

    // Encode PNG
    buf.clear();
    encode_png(&rgba, w, h, buf);
    let png_size = buf.len();

    // Base64 encode
    let mut b64 = Vec::with_capacity((png_size * 4 / 3) + 4);
    base64_encode(buf, &mut b64);

    // Build OSC 1337 sequence.
    // Do NOT specify width/height — letting the terminal auto-size from the
    // PNG dimensions keeps the image at the correct 1:1 pixel scale. Forcing
    // pixel dimensions causes some terminals to upscale dramatically.
    buf.clear();
    buf.extend_from_slice(b"\x1b]1337;File=inline=1;size=");
    let size_str = png_size.to_string();
    buf.extend_from_slice(size_str.as_bytes());
    buf.extend_from_slice(b";preserveAspectRatio=1:");
    buf.extend_from_slice(&b64);
    buf.push(0x07); // BEL terminator

    out.write_all(buf)?;
    out.flush()?;
    Ok(())
}

/// iTerm2 renderer that owns its scratch buffer.
///
/// Wraps [`draw_iterm`] so callers don't need to manage a reusable buffer.
#[derive(Debug)]
pub struct ItermRenderer {
    buf: Vec<u8>,
}

impl ItermRenderer {
    /// Create a new renderer with a pre-allocated scratch buffer.
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64 * 1024),
        }
    }

    /// Render a frame using the iTerm2 inline image protocol.
    pub fn draw<Out: Write>(
        &mut self,
        out: &mut Out,
        frame: &PackedFrame,
        theme: &Theme,
        scale: u8,
    ) -> Result<(), ScampiiError> {
        draw_iterm(&mut self.buf, out, frame, theme, scale)
    }
}

impl Default for ItermRenderer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FRAMES;
    use crate::raster::rasterise;

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn adler32_known_value() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_signature_check() {
        let theme = Theme::classic();
        let (rgba, w, h) = rasterise(&FRAMES[0], &theme, 1);
        let mut png = Vec::new();
        encode_png(&rgba, w, h, &mut png);
        assert_eq!(&png[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    }

    #[test]
    fn osc_output_check() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        let mut out = Vec::new();

        draw_iterm(&mut buf, &mut out, &FRAMES[0], &theme, 1).expect("draw_iterm should not fail");

        assert!(
            out.starts_with(b"\x1b]1337;File=inline=1;size="),
            "output must start with OSC 1337 header"
        );
        assert_eq!(out.last(), Some(&0x07), "output must end with BEL");
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains(':'), "output must contain colon separator");
    }

    #[test]
    fn all_frames_render_iterm() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        for (i, frame) in FRAMES.iter().enumerate() {
            let mut out = Vec::new();
            draw_iterm(&mut buf, &mut out, frame, &theme, 1)
                .unwrap_or_else(|e| panic!("draw_iterm failed on frame {i}: {e}"));
            assert!(!out.is_empty(), "frame {i} produced empty output");
        }
    }
}
