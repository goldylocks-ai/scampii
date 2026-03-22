//! DEC Sixel graphics protocol renderer.
//!
//! Encodes a [`PackedFrame`] as a Sixel image using the terminal's DCS
//! (Device Control String) mechanism. Uses 16 color registers from the
//! theme LUT with per-color scanlines and RLE compression.

use std::io::Write;

use crate::error::ScampiiError;
use crate::frame::{PackedFrame, COMPACT_X0};
use crate::pixel::{unpack_pixel, HUE_COUNT};
use crate::raster::{CROP_H, CROP_W, MAX_SCALE, VERT_PAD};
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a frame using the DEC Sixel protocol.
///
/// Each sprite pixel is rendered as `scale x scale` screen pixels. The frame
/// is cropped to the bounding box, and Sixel bands (6 rows each) are emitted
/// with per-color scanlines and RLE compression.
///
/// `buf` is a reusable scratch buffer to avoid per-frame allocation.
pub fn draw_sixel<Out: Write>(
    buf: &mut Vec<u8>,
    out: &mut Out,
    frame: &PackedFrame,
    theme: &Theme,
    scale: u8,
) -> Result<(), ScampiiError> {
    let scale = scale.clamp(1, MAX_SCALE);
    let s = scale as usize;
    let img_w = CROP_W * s;
    let pad_rows = VERT_PAD * s;
    let img_h = CROP_H * s + pad_rows;

    buf.clear();

    // DCS introducer: P0;0;q  (P0 = no aspect ratio, 0 = bg transparent, q = sixel mode)
    // Set raster attributes: "Pan;Pad;Ph;Pv  (aspect 1:1, width, height)
    buf.extend_from_slice(b"\x1bPq");
    // Raster attributes
    let raster = format!("\"1;1;{};{}", img_w, img_h);
    buf.extend_from_slice(raster.as_bytes());

    // Define color palette: #N;2;R;G;B (RGB percentages)
    for hue_idx in 0..HUE_COUNT {
        let (r, g, b) = theme.lut[hue_idx];
        let rp = (r as u16 * 100 / 255) as u8;
        let gp = (g as u16 * 100 / 255) as u8;
        let bp = (b as u16 * 100 / 255) as u8;
        let def = format!("#{};2;{};{};{}", hue_idx, rp, gp, bp);
        buf.extend_from_slice(def.as_bytes());
    }

    // Build a color index map for the scaled image with top padding.
    // The first `pad_rows` pixel rows are transparent (0) so the sprite
    // sits lower in the terminal cell grid for inline vertical centering.
    // 0 = transparent, 1..=HUE_COUNT = color index + 1
    let mut color_map = vec![0u8; img_w * img_h];
    for (y, row) in frame.iter().enumerate().take(CROP_H) {
        for x in 0..CROP_W {
            let px = row[x + COMPACT_X0];
            if let Some(hue) = unpack_pixel(px) {
                let ci = hue as u8; // 0-based index into lut
                for sy in 0..s {
                    for sx in 0..s {
                        let px_x = x * s + sx;
                        let px_y = pad_rows + y * s + sy;
                        color_map[px_y * img_w + px_x] = ci + 1; // +1 so 0 = transparent
                    }
                }
            }
        }
    }

    // Emit sixel bands (each band = 6 pixel rows)
    let band_count = img_h.div_ceil(6);
    for band in 0..band_count {
        let band_y = band * 6;

        // For each color, check if it appears in this band
        for ci in 0..HUE_COUNT {
            let ci_val = (ci as u8) + 1;

            // Check if this color appears in this band at all
            let mut has_color = false;
            'check: for dy in 0..6 {
                let y = band_y + dy;
                if y >= img_h {
                    break;
                }
                for x in 0..img_w {
                    if color_map[y * img_w + x] == ci_val {
                        has_color = true;
                        break 'check;
                    }
                }
            }

            if !has_color {
                continue;
            }

            // Select this color
            let sel = format!("#{}", ci);
            buf.extend_from_slice(sel.as_bytes());

            // Build sixel data for this color in this band
            let mut sixels = Vec::with_capacity(img_w);
            for x in 0..img_w {
                let mut sixel_val: u8 = 0;
                for dy in 0..6 {
                    let y = band_y + dy;
                    if y < img_h && color_map[y * img_w + x] == ci_val {
                        sixel_val |= 1 << dy;
                    }
                }
                sixels.push(sixel_val + 0x3F); // Sixel encoding offset
            }

            // RLE encode the sixel data
            rle_encode(&sixels, buf);

            // Graphics carriage return ($) to overlay next color on same band
            buf.push(b'$');
        }

        // Graphics new line (-) to advance to next band
        if band + 1 < band_count {
            buf.push(b'-');
        }
    }

    // String Terminator
    buf.extend_from_slice(b"\x1b\\");

    out.write_all(buf)?;
    out.flush()?;
    Ok(())
}

/// RLE-encode a sixel scanline into `out`.
///
/// Repeated characters are encoded as `!N<char>` where N is the repeat count.
fn rle_encode(data: &[u8], out: &mut Vec<u8>) {
    if data.is_empty() {
        return;
    }
    let mut i = 0;
    while i < data.len() {
        let ch = data[i];
        let mut count = 1usize;
        while i + count < data.len() && data[i + count] == ch {
            count += 1;
        }
        if count >= 3 {
            let cs = count.to_string();
            out.push(b'!');
            out.extend_from_slice(cs.as_bytes());
            out.push(ch);
        } else {
            for _ in 0..count {
                out.push(ch);
            }
        }
        i += count;
    }
}

/// Sixel graphics renderer that owns its scratch buffer.
///
/// Wraps [`draw_sixel`] so callers don't need to manage a reusable buffer.
#[derive(Debug)]
pub struct SixelRenderer {
    buf: Vec<u8>,
}

impl SixelRenderer {
    /// Create a new renderer with a pre-allocated scratch buffer.
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64 * 1024),
        }
    }

    /// Render a frame using the DEC Sixel protocol.
    pub fn draw<Out: Write>(
        &mut self,
        out: &mut Out,
        frame: &PackedFrame,
        theme: &Theme,
        scale: u8,
    ) -> Result<(), ScampiiError> {
        draw_sixel(&mut self.buf, out, frame, theme, scale)
    }
}

impl Default for SixelRenderer {
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

    #[test]
    fn sixel_smoke_test() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        let mut out = Vec::new();

        draw_sixel(&mut buf, &mut out, &FRAMES[0], &theme, 1).expect("draw_sixel should not fail");

        // Must start with DCS introducer
        assert!(
            out.starts_with(b"\x1bPq"),
            "output must start with DCS sixel introducer"
        );
        // Must end with String Terminator
        assert!(
            out.ends_with(b"\x1b\\"),
            "output must end with String Terminator"
        );
    }

    #[test]
    fn sixel_contains_color_definitions() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        let mut out = Vec::new();

        draw_sixel(&mut buf, &mut out, &FRAMES[0], &theme, 1).unwrap();
        let s = String::from_utf8_lossy(&out);

        // Should contain at least one color definition
        assert!(
            s.contains("#0;2;"),
            "output should contain color definitions"
        );
    }

    #[test]
    fn sixel_scaled() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        let mut out_1x = Vec::new();
        let mut out_2x = Vec::new();

        draw_sixel(&mut buf, &mut out_1x, &FRAMES[0], &theme, 1).unwrap();
        draw_sixel(&mut buf, &mut out_2x, &FRAMES[0], &theme, 2).unwrap();

        // 2x should produce more data than 1x
        assert!(
            out_2x.len() > out_1x.len(),
            "2x scale should produce more data"
        );
    }

    #[test]
    fn rle_encode_basic() {
        let mut out = Vec::new();
        rle_encode(b"AAABBC", &mut out);
        let s = std::str::from_utf8(&out).unwrap();
        assert_eq!(s, "!3ABBC");
    }

    #[test]
    fn rle_encode_no_repeats() {
        let mut out = Vec::new();
        rle_encode(b"ABC", &mut out);
        assert_eq!(&out, b"ABC");
    }

    #[test]
    fn all_frames_render_sixel() {
        let theme = Theme::classic();
        let mut buf = Vec::new();
        for (i, frame) in FRAMES.iter().enumerate() {
            let mut out = Vec::new();
            draw_sixel(&mut buf, &mut out, frame, &theme, 1)
                .unwrap_or_else(|e| panic!("draw_sixel failed on frame {i}: {e}"));
            assert!(!out.is_empty(), "frame {i} produced empty output");
        }
    }
}
