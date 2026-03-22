//! Shared rasterisation and base64 encoding for pixel-based renderers.
//!
//! Both the iTerm2 and Kitty protocols need the same "frame -> RGBA pixels"
//! rasterisation step and base64 encoding. This module provides those
//! functions once instead of duplicating them.

use crate::frame::{PackedFrame, COMPACT_X0, COMPACT_X1, FRAME_HEIGHT};
use crate::pixel::unpack_pixel;
use crate::theme::Theme;

/// Cropped image width in sprite pixels.
pub const CROP_W: usize = COMPACT_X1 - COMPACT_X0;

/// Cropped image height in sprite pixels.
pub const CROP_H: usize = FRAME_HEIGHT;

/// Maximum allowed scale factor.
///
/// At scale 16 the rasterised image is 384x416 pixels (~640 KB RGBA). Higher
/// values offer no visual benefit and can allocate hundreds of megabytes of
/// memory (e.g. scale 255 would allocate ~400 MB). The `rasterise` function
/// and all image-protocol renderers clamp to this value.
pub const MAX_SCALE: u8 = 16;

/// Transparent top padding added when rasterising for inline display.
///
/// Image-protocol images are top-aligned in the terminal cell grid. Without
/// padding the sprite sits at the very top of its block, forcing every
/// embedder to add their own top margin. Two sprite-pixel rows of padding
/// shifts the visible content down so it is roughly vertically centred at
/// the default `scale = 1` size (~2 text lines).
pub(crate) const VERT_PAD: usize = 2;

// ---------------------------------------------------------------------------
// Base64
// ---------------------------------------------------------------------------

const BASE64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64-encode `input`, appending the result to `out`.
pub fn base64_encode(input: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    let len = input.len();
    while i + 2 < len {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        out.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        out.push(BASE64_CHARS[((n >> 6) & 0x3F) as usize]);
        out.push(BASE64_CHARS[(n & 0x3F) as usize]);
        i += 3;
    }
    let remaining = len - i;
    if remaining == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        out.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        out.push(BASE64_CHARS[((n >> 6) & 0x3F) as usize]);
        out.push(b'=');
    } else if remaining == 1 {
        let n = (input[i] as u32) << 16;
        out.push(BASE64_CHARS[((n >> 18) & 0x3F) as usize]);
        out.push(BASE64_CHARS[((n >> 12) & 0x3F) as usize]);
        out.push(b'=');
        out.push(b'=');
    }
}

// ---------------------------------------------------------------------------
// Rasteriser
// ---------------------------------------------------------------------------

/// Rasterise a frame to RGBA pixels, applying the theme and scale factor.
///
/// Each sprite pixel becomes a `scale x scale` block of identical RGBA pixels.
/// Transparent pixels are encoded as `(0, 0, 0, 0)`.
///
/// Returns `(rgba_buffer, width_px, height_px)`.
pub fn rasterise(frame: &PackedFrame, theme: &Theme, scale: u8) -> (Vec<u8>, u32, u32) {
    let s = scale.clamp(1, MAX_SCALE) as usize;
    let w = CROP_W * s;
    let h = CROP_H * s;
    let mut rgba = vec![0u8; w * h * 4];

    for (y, row) in frame.iter().enumerate().take(CROP_H) {
        for x in 0..CROP_W {
            let px = row[x + COMPACT_X0];
            let (r, g, b, a) = match unpack_pixel(px) {
                Some(hue) => {
                    let (cr, cg, cb) = theme.lut[hue as usize];
                    (cr, cg, cb, 255u8)
                }
                None => (0, 0, 0, 0),
            };
            // Fill the scale x scale block
            for sy in 0..s {
                for sx in 0..s {
                    let px_x = x * s + sx;
                    let px_y = y * s + sy;
                    let idx = (px_y * w + px_x) * 4;
                    rgba[idx] = r;
                    rgba[idx + 1] = g;
                    rgba[idx + 2] = b;
                    rgba[idx + 3] = a;
                }
            }
        }
    }

    (rgba, w as u32, h as u32)
}

/// Rasterise a frame with transparent top padding for inline display.
///
/// Identical to [`rasterise`] but prepends [`VERT_PAD`] × `scale` rows of
/// fully-transparent pixels so the image sits lower in the terminal cell
/// grid. All image-protocol renderers (iTerm2, Kitty, Sixel) should prefer
/// this variant so users don't have to manually add top margins.
///
/// Returns `(rgba_buffer, width_px, height_px)`.
pub(crate) fn rasterise_padded(
    frame: &PackedFrame,
    theme: &Theme,
    scale: u8,
) -> (Vec<u8>, u32, u32) {
    let s = scale.clamp(1, MAX_SCALE) as usize;
    let (src, w, h) = rasterise(frame, theme, scale);
    let w = w as usize;
    let h = h as usize;
    let pad_rows = VERT_PAD * s;
    let new_h = h + pad_rows;

    let mut padded = vec![0u8; w * new_h * 4]; // 0-filled = transparent
    let offset = pad_rows * w * 4;
    padded[offset..offset + src.len()].copy_from_slice(&src);

    (padded, w as u32, new_h as u32)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FRAMES;

    #[test]
    fn base64_rfc_vectors() {
        let cases: &[(&[u8], &str)] = &[
            (b"", ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ];
        for &(input, expected) in cases {
            let mut out = Vec::new();
            base64_encode(input, &mut out);
            assert_eq!(
                std::str::from_utf8(&out).unwrap(),
                expected,
                "base64 mismatch for {:?}",
                std::str::from_utf8(input).unwrap()
            );
        }
    }

    #[test]
    fn rasterise_dimensions_scale_1() {
        let theme = crate::theme::Theme::classic();
        let (rgba, w, h) = rasterise(&FRAMES[0], &theme, 1);
        assert_eq!(w, CROP_W as u32);
        assert_eq!(h, CROP_H as u32);
        assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4);
    }

    #[test]
    fn rasterise_dimensions_scale_2() {
        let theme = crate::theme::Theme::classic();
        let (rgba, w, h) = rasterise(&FRAMES[0], &theme, 2);
        assert_eq!(w, (CROP_W * 2) as u32);
        assert_eq!(h, (CROP_H * 2) as u32);
        assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4);
    }

    #[test]
    fn rasterise_clamps_excessive_scale() {
        let theme = crate::theme::Theme::classic();
        // scale=255 must be clamped to MAX_SCALE, not allocate ~400 MB
        let (rgba, w, h) = rasterise(&FRAMES[0], &theme, 255);
        assert_eq!(w, (CROP_W * MAX_SCALE as usize) as u32);
        assert_eq!(h, (CROP_H * MAX_SCALE as usize) as u32);
        assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4);
    }

    #[test]
    fn rasterise_clamps_zero_scale_to_one() {
        let theme = crate::theme::Theme::classic();
        let (rgba, w, h) = rasterise(&FRAMES[0], &theme, 0);
        assert_eq!(w, CROP_W as u32);
        assert_eq!(h, CROP_H as u32);
        assert_eq!(rgba.len(), (w as usize) * (h as usize) * 4);
    }

    #[test]
    fn rasterise_transparent_pixel_is_zero_alpha() {
        let theme = crate::theme::Theme::classic();
        let (rgba, w, _h) = rasterise(&FRAMES[0], &theme, 1);
        // Row 0 is entirely transparent in all frames
        for x in 0..CROP_W {
            let idx = (0 * w as usize + x) * 4;
            assert_eq!(rgba[idx + 3], 0, "pixel ({x}, 0) should be transparent");
        }
    }
}
