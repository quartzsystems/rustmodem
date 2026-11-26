// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The fonts, from the system.
//!
//! There are two. The message font is the one Windows uses for the text
//! of dialogs and controls, at the size the user has set, for the DPI of
//! the window. The log font is the one exception to never naming a font:
//! log panes are in Consolas, at the message font's size.

use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DeleteObject, FIXED_PITCH, FONT_PITCH, HFONT, LOGFONTW,
};
use windows::Win32::UI::HiDpi::SystemParametersInfoForDpi;
use windows::Win32::UI::WindowsAndMessaging::{NONCLIENTMETRICSW, SPI_GETNONCLIENTMETRICS};

use crate::metrics::BASE_DPI;
use crate::util::from_wide;

/// The face of the log panes.
const LOG_FACE: &str = "Consolas";

/// A font, deleted when dropped. A control that has been given a font
/// must not outlive it.
pub struct Font {
    handle: HFONT,
    /// The height of the font in pixels, as a positive number.
    height: i32,
    face: String,
}

/// The message font as Windows describes it for a DPI.
fn message_logfont(dpi: u32) -> LOGFONTW {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..NONCLIENTMETRICSW::default()
    };
    // SAFETY: `metrics` is a NONCLIENTMETRICSW with its size filled in,
    // which is what this action writes to.
    let asked = unsafe {
        SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            metrics.cbSize,
            Some((&raw mut metrics).cast()),
            0,
            dpi,
        )
    };
    if asked.is_err() {
        // Not expected on any Windows this runs on. A zeroed description
        // makes the system's default font, which is still a system font.
        return LOGFONTW::default();
    }
    metrics.lfMessageFont
}

impl Font {
    fn create(description: &LOGFONTW) -> Font {
        // SAFETY: `description` is a whole LOGFONTW. A null handle, which
        // would mean the system is out of resources, makes controls fall
        // back to the system font and is harmless to delete.
        let handle = unsafe { CreateFontIndirectW(description) };
        Font {
            handle,
            height: description.lfHeight.abs(),
            face: from_wide(&description.lfFaceName),
        }
    }

    /// The message font for a window at `dpi`.
    pub fn message(dpi: u32) -> Font {
        Font::create(&message_logfont(dpi))
    }

    /// The font of log panes for a window at `dpi`: Consolas at the
    /// message font's size.
    pub fn log(dpi: u32) -> Font {
        let mut description = message_logfont(dpi);
        description.lfFaceName = [0; 32];
        for (slot, unit) in description
            .lfFaceName
            .iter_mut()
            .zip(LOG_FACE.encode_utf16())
        {
            *slot = unit;
        }
        description.lfPitchAndFamily = FONT_PITCH(FIXED_PITCH.0).0;
        Font::create(&description)
    }

    pub(crate) fn handle(&self) -> HFONT {
        self.handle
    }

    /// The height of the font in pixels.
    pub fn height(&self) -> i32 {
        self.height
    }

    /// The name of the face.
    pub fn face(&self) -> &str {
        &self.face
    }

    /// The size of the font in points, for a dialog template: the message
    /// font at 96 DPI, whatever DPI this font was made for. The dialog
    /// manager scales a template for the DPI itself.
    pub fn points(&self, dpi: u32) -> u16 {
        let dpi = i64::from(dpi.max(1));
        // 72 points to the inch.
        let points = (i64::from(self.height) * 72 + dpi / 2) / dpi;
        points.clamp(1, 72) as u16
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: the handle was made by `create` and is deleted nowhere
        // else.
        unsafe {
            let _ = DeleteObject(self.handle.into());
        }
    }
}

/// The message font's face and size in points, for dialog templates.
pub(crate) fn message_face_and_points() -> (String, u16) {
    let font = Font::message(BASE_DPI);
    (font.face().to_string(), font.points(BASE_DPI))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_font_is_the_systems_and_grows_with_the_dpi() {
        let small = Font::message(96);
        let large = Font::message(192);
        assert!(!small.face().is_empty());
        assert_eq!(small.face(), large.face());
        assert!(small.height() >= 8, "{} pixels", small.height());
        // Twice the DPI is twice the pixels, give or take rounding.
        assert!((large.height() - 2 * small.height()).abs() <= 1);
        // And the same size in points at both.
        assert_eq!(small.points(96), large.points(192));
        assert!((6..=24).contains(&small.points(96)));
        assert!(!small.handle().is_invalid());
        let (face, points) = message_face_and_points();
        assert_eq!(face, small.face());
        assert_eq!(points, small.points(96));
    }

    #[test]
    fn the_log_font_is_consolas_at_the_message_fonts_size() {
        let message = Font::message(144);
        let log = Font::log(144);
        assert_eq!(log.face(), "Consolas");
        assert_eq!(log.height(), message.height());
    }
}
