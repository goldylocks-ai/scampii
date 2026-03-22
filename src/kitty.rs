//! Kitty graphics protocol renderer.
//!
//! Rasterises a [`PackedFrame`] to RGBA pixels, base64-encodes the raw data,
//! and transmits it via the Kitty graphics protocol escape sequence. Data
//! larger than 4096 bytes is chunked with continuation flags.

use std::io::Write;

use crate::error::ScampiiError;
use crate::frame::PackedFrame;
use crate::raster::{base64_encode, rasterise_padded, MAX_SCALE};
use crate::theme::Theme;

/// Maximum payload per Kitty graphics chunk (bytes of base64).
const CHUNK_SIZE: usize = 4096;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a frame using the Kitty graphics protocol.
///
/// Each sprite pixel is rendered as `scale x scale` screen pixels. The frame
/// is cropped to the bounding box, rasterised to RGBA, base64-encoded, and
/// transmitted via `\x1b_G...` escape sequences. Payloads larger than 4096
/// bytes are chunked with `m=1` continuation flags.
///
/// `buf` is a reusable scratch buffer to avoid per-frame allocation.
pub fn draw_kitty<Out: Write>(
    buf: &mut Vec<u8>,
    out: &mut Out,
    frame: &PackedFrame,
    theme: &Theme,
    scale: u8,
) -> Result<(), ScampiiError> {
    let scale = scale.clamp(1, MAX_SCALE);
    let (rgba, w, h) = rasterise_padded(frame, theme, scale);

    // Base64 encode the raw RGBA data into buf
    buf.clear();
    base64_encode(&rgba, buf);

    // Write directly from buf -- no clone needed. We slice into the
    // already-populated base64 data while writing chunk headers inline.
    let total_len = buf.len();

    if total_len <= CHUNK_SIZE {
        // Single chunk: a=T (transmit and display), f=32 (RGBA), t=d (direct data)
        let header = format!(
            "\x1b_Gf=32,s={},v={},a=T,t=d;",
            w, h
        );
        out.write_all(header.as_bytes())?;
        out.write_all(buf)?;
        out.write_all(b"\x1b\\")?;
    } else {
        // Multi-chunk transmission. We need to read from buf while writing
        // to out, so swap the base64 data into a separate vec. This reuses
        // buf's allocation on the next frame (caller keeps buf across calls).
        let mut b64_data = std::mem::take(buf);

        let mut offset = 0;
        let mut first = true;

        while offset < total_len {
            let remaining = total_len - offset;
            let chunk_len = remaining.min(CHUNK_SIZE);
            let is_last = offset + chunk_len >= total_len;
            let m = if is_last { 0 } else { 1 };

            if first {
                let header = format!(
                    "\x1b_Gf=32,s={},v={},a=T,t=d,m={};",
                    w, h, m
                );
                out.write_all(header.as_bytes())?;
                first = false;
            } else {
                let header = format!("\x1b_Gm={};", m);
                out.write_all(header.as_bytes())?;
            }

            out.write_all(&b64_data[offset..offset + chunk_len])?;
            out.write_all(b"\x1b\\")?;

            offset += chunk_len;
        }

        // Give the allocation back to buf so it can be reused next frame
        b64_data.clear();
        *buf = b64_data;
    }

    out.flush()?;
    Ok(())
}

/// Kitty graphics renderer that owns its scratch buffer.
///
/// Wraps [`draw_kitty`] so callers don't need to manage a reusable buffer.
#[derive(Debug)]
pub struct KittyRenderer {
    buf: Vec<u8>,
}

impl KittyRenderer {
    /// Create a new renderer with a pre-allocated scratch buffer.
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64 * 1024),
        }
    }

    /// Render a frame using the Kitty graphics protocol.
    pub fn draw<Out: Write>(
        &mut self,
        out: &mut Out,
        frame: &PackedFrame,
        theme: &Theme,
        scale: u8,
    ) -> Result<(), ScampiiError> {
        draw_kitty(&mut self.buf, out, frame, theme, scale)
    }
}

impl Default for KittyRenderer {
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
    use crate::raster::{CROP_H, CROP_W, VERT_PAD};

    #[test]
    fn kitty_small_single_chunk() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        let mut out = Vec::new();

        // scale=1 produces a small image that fits in one chunk
        draw_kitty(&mut buf, &mut out, &FRAMES[0], &theme, 1).expect("draw_kitty should not fail");

        let s = String::from_utf8_lossy(&out);
        // Must contain Kitty graphics header
        assert!(s.contains("\x1b_G"), "output must contain Kitty APC start");
        // Must contain ST
        assert!(s.contains("\x1b\\"), "output must contain ST");
        // Must contain format and action params
        assert!(s.contains("f=32"), "output must specify RGBA format");
        assert!(s.contains("a=T"), "output must specify transmit+display");
    }

    #[test]
    fn kitty_large_multi_chunk() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        let mut out = Vec::new();

        // scale=4 produces a large image requiring multiple chunks
        draw_kitty(&mut buf, &mut out, &FRAMES[0], &theme, 4)
            .expect("draw_kitty should not fail at 4x");

        let s = String::from_utf8_lossy(&out);
        // Should have continuation chunks (m=1)
        assert!(s.contains("m=1"), "large image should use multi-chunk mode");
        // Should end with m=0 (final chunk)
        assert!(s.contains("m=0"), "last chunk should have m=0");
    }

    #[test]
    fn kitty_dimensions_in_header() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        let mut out = Vec::new();
        let scale = 2u8;

        draw_kitty(&mut buf, &mut out, &FRAMES[0], &theme, scale).unwrap();

        let s = String::from_utf8_lossy(&out);
        let expected_w = CROP_W * scale as usize;
        let expected_h = (CROP_H + VERT_PAD) * scale as usize;
        let w_str = format!("s={}", expected_w);
        let h_str = format!("v={}", expected_h);
        assert!(
            s.contains(&w_str),
            "header must contain width s={expected_w}"
        );
        assert!(
            s.contains(&h_str),
            "header must contain height v={expected_h}"
        );
    }

    #[test]
    fn all_frames_render_kitty() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        for (i, frame) in FRAMES.iter().enumerate() {
            let mut out = Vec::new();
            draw_kitty(&mut buf, &mut out, frame, &theme, 1)
                .unwrap_or_else(|e| panic!("draw_kitty failed on frame {i}: {e}"));
            assert!(!out.is_empty(), "frame {i} produced empty output");
        }
    }

    #[test]
    fn kitty_buf_reusable_across_frames() {
        let theme = Theme::classic();
        let mut buf = Vec::new();

        // Render multiple frames with the same buf to verify it's properly reused
        for frame in FRAMES.iter() {
            let mut out = Vec::new();
            draw_kitty(&mut buf, &mut out, frame, &theme, 4).unwrap();
            assert!(!out.is_empty());
        }
    }
}
