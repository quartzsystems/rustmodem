// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Icon images as Windows keeps them, in `.ico` files and in the icon
//! resources of a program.
//!
//! An image below 256 pixels is a device-independent bitmap: a
//! `BITMAPINFOHEADER`, the pixels as blue, green, red and alpha from the
//! bottom row up, and a one-bit mask of the transparent pixels. An image
//! of 256 pixels is a PNG file. Both forms are the same in a file and in a
//! resource.

use crate::{Glyph, png, rgba};

/// The sizes an icon file and an icon resource hold, in pixels.
pub const ICON_SIZES: [u32; 6] = [16, 20, 24, 32, 48, 256];

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// One image of an icon at `px` pixels, as it is stored.
pub fn image(glyph: Glyph, px: u32) -> Vec<u8> {
    let pixels = rgba(glyph, px);
    if px >= 256 {
        return png::encode(&pixels, px, px);
    }
    let side = px as usize;
    let mut out = Vec::new();
    // BITMAPINFOHEADER. The height counts the colour rows and the mask
    // rows together.
    put32(&mut out, 40);
    put32(&mut out, px);
    put32(&mut out, px * 2);
    put16(&mut out, 1);
    put16(&mut out, 32);
    put32(&mut out, 0);
    put32(&mut out, 0);
    put32(&mut out, 0);
    put32(&mut out, 0);
    put32(&mut out, 0);
    put32(&mut out, 0);
    for y in (0..side).rev() {
        for x in 0..side {
            let at = (y * side + x) * 4;
            out.extend_from_slice(&[pixels[at + 2], pixels[at + 1], pixels[at], pixels[at + 3]]);
        }
    }
    // The mask: a bit set for each pixel that is wholly transparent, each
    // row padded to 32 bits.
    let mask_row = side.div_ceil(32) * 4;
    for y in (0..side).rev() {
        let mut row = vec![0u8; mask_row];
        for x in 0..side {
            if pixels[(y * side + x) * 4 + 3] == 0 {
                row[x / 8] |= 0x80 >> (x % 8);
            }
        }
        out.extend_from_slice(&row);
    }
    out
}

/// The directory entry of one image, less where the image is: width,
/// height, colours, reserved, planes, bits per pixel and size in bytes.
/// 256 pixels is written as 0.
pub(crate) fn entry(out: &mut Vec<u8>, px: u32, bytes: usize) {
    let side = if px >= 256 { 0 } else { px as u8 };
    out.push(side);
    out.push(side);
    out.push(0);
    out.push(0);
    put16(out, 1);
    put16(out, 32);
    put32(out, bytes as u32);
}

/// A glyph as a Windows `.ico` file at [`ICON_SIZES`].
pub fn ico(glyph: Glyph) -> Vec<u8> {
    let images: Vec<Vec<u8>> = ICON_SIZES.iter().map(|px| image(glyph, *px)).collect();
    let mut out = Vec::new();
    // ICONDIR: reserved, type 1 (icon), count.
    put16(&mut out, 0);
    put16(&mut out, 1);
    put16(&mut out, images.len() as u16);
    let mut offset = 6 + 16 * images.len();
    for (px, image) in ICON_SIZES.iter().zip(&images) {
        entry(&mut out, *px, image.len());
        put32(&mut out, offset as u32);
        offset += image.len();
    }
    for image in &images {
        out.extend_from_slice(image);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16_at(data: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([data[at], data[at + 1]])
    }

    fn u32_at(data: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn an_icon_file_has_a_directory_and_an_image_for_each_size() {
        let file = ico(Glyph::App);
        assert_eq!(file[..6], [0, 0, 1, 0, 6, 0]);
        let mut end_of_last = 6 + 16 * ICON_SIZES.len();
        for (index, px) in ICON_SIZES.iter().enumerate() {
            let at = 6 + 16 * index;
            let side = if *px == 256 { 0 } else { *px as u8 };
            assert_eq!((file[at], file[at + 1]), (side, side));
            assert_eq!((u16_at(&file, at + 4), u16_at(&file, at + 6)), (1, 32));
            let (bytes, offset) = (
                u32_at(&file, at + 8) as usize,
                u32_at(&file, at + 12) as usize,
            );
            // The images follow one another with nothing between.
            assert_eq!(offset, end_of_last);
            end_of_last = offset + bytes;
            assert_eq!(file[offset..offset + bytes], image(Glyph::App, *px)[..]);
        }
        assert_eq!(end_of_last, file.len());
    }

    #[test]
    fn a_small_image_is_a_bitmap_with_its_mask() {
        for px in [16u32, 20, 24, 32, 48] {
            let stored = image(Glyph::StateOnline, px);
            let side = px as usize;
            let mask_row = side.div_ceil(32) * 4;
            assert_eq!(stored.len(), 40 + side * side * 4 + side * mask_row, "{px}");
            assert_eq!(u32_at(&stored, 0), 40);
            assert_eq!((u32_at(&stored, 4), u32_at(&stored, 8)), (px, px * 2));
            assert_eq!((u16_at(&stored, 12), u16_at(&stored, 14)), (1, 32));

            // The rows run from the bottom up, in blue, green, red, alpha.
            let pixels = rgba(Glyph::StateOnline, px);
            let (x, y) = (side / 2, side / 3);
            let from = (y * side + x) * 4;
            let to = 40 + ((side - 1 - y) * side + x) * 4;
            assert_eq!(
                stored[to..to + 4],
                [
                    pixels[from + 2],
                    pixels[from + 1],
                    pixels[from],
                    pixels[from + 3]
                ]
            );
            assert_eq!(pixels[from + 3], 255, "the middle of the lamp is opaque");

            // The corner is transparent and masked; the middle is not.
            let mask = &stored[40 + side * side * 4..];
            let masked = |x: usize, y: usize| {
                mask[(side - 1 - y) * mask_row + x / 8] & (0x80 >> (x % 8)) != 0
            };
            assert!(masked(0, 0), "{px}");
            assert!(masked(side - 1, side - 1), "{px}");
            assert!(!masked(x, y), "{px}");
        }
    }

    #[test]
    fn the_largest_image_is_a_png() {
        let stored = image(Glyph::App, 256);
        assert_eq!(stored[..8], png::SIGNATURE);
        let (pixels, width, height) = png::tests::decode(&stored);
        assert_eq!((width, height), (256, 256));
        assert_eq!(pixels, rgba(Glyph::App, 256));
        // Drawn shapes compress well even with a simple encoder.
        assert!(stored.len() < 256 * 256, "{} bytes", stored.len());
    }
}
