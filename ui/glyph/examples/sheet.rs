// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Writes every glyph at every size to one picture, and the application
//! icon to an icon file, for looking at:
//!
//! ```text
//! cargo run -p glyph --example sheet [DIRECTORY]
//! ```
//!
//! The files are `glyphs.png` and `rustmodem.ico` in the directory given,
//! or in `target/glyphs`. Each row of the picture is one glyph, at 16, 20,
//! 24, 32 and 48 pixels and then at 96, on a white half and a dark half.

use std::path::PathBuf;

use glyph::{Glyph, ico, png, rgba};

/// The sizes of a row, in pixels.
const ROW_SIZES: [u32; 6] = [16, 20, 24, 32, 48, 96];

/// The height of a row and the space around each image, in pixels.
const ROW: usize = 104;
const GAP: usize = 12;

fn main() {
    let directory = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("target").join("glyphs"), PathBuf::from);
    std::fs::create_dir_all(&directory).expect("the directory can be made");

    let strip: usize = ROW_SIZES.iter().map(|px| *px as usize + GAP).sum::<usize>() + GAP;
    let width = strip * 2;
    let height = ROW * Glyph::ALL.len();
    let mut sheet = vec![0u8; width * height * 4];
    for (index, pixel) in sheet.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        // The left half is white and the right half dark, to show the
        // glyphs on both.
        let shade = if index % width < strip { 255 } else { 32 };
        *pixel = [shade, shade, shade, 255];
    }
    for (row, glyph) in Glyph::ALL.iter().enumerate() {
        for half in 0..2 {
            let mut left = half * strip + GAP;
            for px in ROW_SIZES {
                let side = px as usize;
                let image = rgba(*glyph, px);
                let top = row * ROW + (ROW - side) / 2;
                for y in 0..side {
                    for x in 0..side {
                        let from = (y * side + x) * 4;
                        let to = ((top + y) * width + left + x) * 4;
                        let alpha = u32::from(image[from + 3]);
                        for channel in 0..3 {
                            let over = u32::from(image[from + channel]);
                            let under = u32::from(sheet[to + channel]);
                            sheet[to + channel] =
                                ((over * alpha + under * (255 - alpha)) / 255) as u8;
                        }
                    }
                }
                left += side + GAP;
            }
        }
    }
    let picture = directory.join("glyphs.png");
    std::fs::write(&picture, png::encode(&sheet, width as u32, height as u32))
        .expect("the picture can be written");
    let icon = directory.join("rustmodem.ico");
    std::fs::write(&icon, ico(Glyph::App)).expect("the icon can be written");
    println!("{}", picture.display());
    println!("{}", icon.display());
}
