// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Sizes and colours, all of them from the system.
//!
//! Sizes in the manager are written for 96 dots per inch, which is 100
//! percent scaling, and scaled to the DPI of the window they are used in.
//! Colours are system colours; the two the manager chooses for itself are
//! given up for the system's text colour when a high-contrast theme is on.

use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, COLOR_GRAYTEXT, COLOR_WINDOW, COLOR_WINDOWTEXT, GetSysColor, SYS_COLOR_INDEX,
};
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVSCROLL, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SystemParametersInfoW,
};

use crate::window::Colour;

/// The DPI of 100 percent scaling.
pub const BASE_DPI: u32 = 96;

/// A size written for 96 DPI, at `dpi`. Rounded to the nearest pixel.
pub fn scale(value: i32, dpi: u32) -> i32 {
    let scaled = i64::from(value) * i64::from(dpi);
    let half = i64::from(BASE_DPI) / 2;
    let rounded = if scaled >= 0 {
        (scaled + half) / i64::from(BASE_DPI)
    } else {
        (scaled - half) / i64::from(BASE_DPI)
    };
    rounded as i32
}

/// A size in pixels at `dpi`, as it would be at 96 DPI: what is saved, so
/// that a width saved on one monitor is right on another.
pub fn unscale(value: i32, dpi: u32) -> i32 {
    let dpi = i64::from(dpi.max(1));
    ((i64::from(value) * i64::from(BASE_DPI) + dpi / 2) / dpi) as i32
}

/// The DPI of the primary monitor, for before there is a window.
pub fn system_dpi() -> u32 {
    // SAFETY: the call takes nothing and returns a number.
    unsafe { GetDpiForSystem() }
}

fn system_colour(index: SYS_COLOR_INDEX) -> Colour {
    // SAFETY: the call takes an index and returns a colour; an index it
    // does not know gives black.
    let colour = unsafe { GetSysColor(index) };
    Colour::from_colorref(colour)
}

/// The colour of text in a window.
pub fn window_text() -> Colour {
    system_colour(COLOR_WINDOWTEXT)
}

/// The colour behind text in a window.
pub fn window_background() -> Colour {
    system_colour(COLOR_WINDOW)
}

/// The colour of disabled text.
pub fn gray_text() -> Colour {
    system_colour(COLOR_GRAYTEXT)
}

/// The colour of dialog and button faces.
pub fn face() -> Colour {
    system_colour(COLOR_BTNFACE)
}

/// Whether a high-contrast theme is on. Then every colour is the user's
/// own choice, and a program must not use one of its own.
pub fn high_contrast() -> bool {
    let mut info = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: `info` is a HIGHCONTRASTW with its size filled in, which is
    // what this action writes to.
    let asked = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            Some((&raw mut info).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    asked.is_ok() && info.dwFlags.contains(HCF_HIGHCONTRASTON)
}

/// A colour the program chose, or the system's text colour when a
/// high-contrast theme is on.
pub fn chosen(colour: Colour) -> Colour {
    if high_contrast() {
        window_text()
    } else {
        colour
    }
}

/// The width of a vertical scroll bar, in pixels.
pub fn scroll_bar_width() -> i32 {
    // SAFETY: the call takes an index and returns a number.
    unsafe { GetSystemMetrics(SM_CXVSCROLL) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_scale_with_the_dpi_and_round_to_the_nearest_pixel() {
        assert_eq!(scale(100, 96), 100);
        assert_eq!(scale(100, 144), 150);
        assert_eq!(scale(100, 192), 200);
        assert_eq!(scale(30, 120), 38);
        assert_eq!(scale(1, 120), 1);
        assert_eq!(scale(3, 120), 4);
        assert_eq!(scale(0, 144), 0);
        assert_eq!(scale(-10, 144), -15);
        assert_eq!(scale(-3, 120), -4);
    }

    #[test]
    fn a_size_saved_at_one_dpi_is_right_at_another() {
        for dpi in [96, 120, 144, 168, 192] {
            for width in [30, 45, 70, 110, 500] {
                assert_eq!(unscale(scale(width, dpi), dpi), width, "{width} at {dpi}");
            }
        }
        assert_eq!(unscale(150, 144), 100);
        assert_eq!(unscale(7, 0), 672, "a DPI of zero does not divide by zero");
    }

    #[test]
    fn the_system_answers() {
        assert!(system_dpi() >= 96);
        assert!(scroll_bar_width() > 0);
        // Whatever the colours are, the choice follows the theme.
        let red = Colour::rgb(200, 0, 0);
        let expected = if high_contrast() { window_text() } else { red };
        assert_eq!(chosen(red), expected);
        // Text can be read on its background.
        assert_ne!(window_text(), window_background());
        let _ = (gray_text(), face());
    }
}
