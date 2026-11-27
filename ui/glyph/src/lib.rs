// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The icons of rustmodem, drawn in code.
//!
//! Every image the product shows is described here as a few vector shapes
//! on a 16-unit grid and rendered at whatever size is asked for: the
//! application icon, the images of the manager's toolbar, and the images
//! of the line states in its grid. There are no image files in the
//! repository.
//!
//! - [`rgba`] renders a glyph to pixels.
//! - [`ico`] writes a Windows `.ico` file with several sizes.
//! - [`windows_res`] writes a compiled Windows resource file holding a
//!   program's icon, its version information and its application manifest,
//!   and [`link_windows_res`] links it from a build script. The MSVC
//!   linker takes a `.res` directly, so no resource compiler is used.
//!
//! The crate is pure Rust and has no dependencies, so that build scripts
//! can use it. The approach, and the resource writer, follow Quartz
//! Systems' own `quartz-systems-access` (`qsa-glyph`, `qsa-icons`).

#![forbid(unsafe_code)]

mod canvas;
mod draw;
mod ico;
pub mod png;
mod res;

pub use ico::{ICON_SIZES, ico, image};
pub use res::{
    LinkInto, Manifest, VersionInfo, link_windows_res, message_table, version_resource,
    windows_res, windows_res_with_messages,
};

/// The sizes the glyphs are made for, in pixels: 16 at 100 percent
/// scaling, 20 at 125, 24 at 150, 32 at 200, and 48 and 256 for the
/// application icon in Explorer.
pub const SIZES: [u32; 6] = ICON_SIZES;

/// The glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    /// The application icon: a modem with its lamps lit and a carrier.
    App,
    /// Toolbar: answer now. A handset, lifted.
    Answer,
    /// Toolbar: hang up. A handset on the hook.
    HangUp,
    /// Toolbar: reset line. An arrow once round.
    Reset,
    /// Toolbar: new line. A modem and a plus.
    NewLine,
    /// Toolbar: re-register. Two arrows, out and back.
    Reregister,
    /// Toolbar: reload configuration. A sheet and a turning arrow.
    Reload,
    /// Toolbar: settings. A gear.
    Settings,
    /// Line state: disabled. A circle with a bar through it.
    StateDisabled,
    /// Line state: not ready. An empty circle.
    StateNotReady,
    /// Line state: idle. A blue lamp.
    StateIdle,
    /// Line state: ringing. An amber lamp with the ring coming off it.
    StateRinging,
    /// Line state: handshaking. Two amber arrows.
    StateHandshaking,
    /// Line state: online. A green lamp.
    StateOnline,
    /// Line state: a port error. A red lamp with an exclamation mark.
    StateError,
}

impl Glyph {
    /// Every glyph, in a stable order.
    pub const ALL: [Glyph; 15] = [
        Glyph::App,
        Glyph::Answer,
        Glyph::HangUp,
        Glyph::Reset,
        Glyph::NewLine,
        Glyph::Reregister,
        Glyph::Reload,
        Glyph::Settings,
        Glyph::StateDisabled,
        Glyph::StateNotReady,
        Glyph::StateIdle,
        Glyph::StateRinging,
        Glyph::StateHandshaking,
        Glyph::StateOnline,
        Glyph::StateError,
    ];

    /// The images of the toolbar, in the order of its buttons.
    pub const TOOLBAR: [Glyph; 7] = [
        Glyph::Answer,
        Glyph::HangUp,
        Glyph::Reset,
        Glyph::NewLine,
        Glyph::Reregister,
        Glyph::Reload,
        Glyph::Settings,
    ];

    /// The images of the line states.
    pub const STATES: [Glyph; 7] = [
        Glyph::StateDisabled,
        Glyph::StateNotReady,
        Glyph::StateIdle,
        Glyph::StateRinging,
        Glyph::StateHandshaking,
        Glyph::StateOnline,
        Glyph::StateError,
    ];

    /// A short lower-case name, for file names and tests.
    pub fn name(self) -> &'static str {
        match self {
            Glyph::App => "app",
            Glyph::Answer => "answer",
            Glyph::HangUp => "hang-up",
            Glyph::Reset => "reset",
            Glyph::NewLine => "new-line",
            Glyph::Reregister => "re-register",
            Glyph::Reload => "reload",
            Glyph::Settings => "settings",
            Glyph::StateDisabled => "state-disabled",
            Glyph::StateNotReady => "state-not-ready",
            Glyph::StateIdle => "state-idle",
            Glyph::StateRinging => "state-ringing",
            Glyph::StateHandshaking => "state-handshaking",
            Glyph::StateOnline => "state-online",
            Glyph::StateError => "state-error",
        }
    }
}

/// Renders a glyph `px` pixels square: `px * px * 4` bytes of red, green,
/// blue and alpha, row by row from the top, the colour not multiplied by
/// the alpha.
pub fn rgba(glyph: Glyph, px: u32) -> Vec<u8> {
    let mut canvas = canvas::Canvas::new(px as usize);
    draw::draw(glyph, &mut canvas);
    canvas.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How much of a rendering is painted, from 0 to 1.
    fn coverage(pixels: &[u8]) -> f32 {
        let alpha: u32 = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| u32::from(p[3]))
            .sum();
        alpha as f32 / (pixels.len() / 4) as f32 / 255.0
    }

    #[test]
    fn every_glyph_renders_at_every_size() {
        for glyph in Glyph::ALL {
            for px in SIZES {
                let pixels = rgba(glyph, px);
                assert_eq!(pixels.len(), (px * px * 4) as usize);
                let covered = coverage(&pixels);
                // Something is drawn, and it is a shape, not a filled
                // square.
                assert!(
                    (0.08..0.85).contains(&covered),
                    "{} at {px}: {covered} covered",
                    glyph.name()
                );
                // Nothing is drawn where there is no alpha.
                for pixel in pixels.as_chunks::<4>().0 {
                    if pixel[3] == 0 {
                        assert_eq!(pixel[..3], [0, 0, 0], "{} at {px}", glyph.name());
                    }
                }
            }
        }
    }

    #[test]
    fn rendering_is_the_same_every_time() {
        for glyph in [Glyph::App, Glyph::Settings, Glyph::StateRinging] {
            assert_eq!(rgba(glyph, 32), rgba(glyph, 32));
        }
    }

    #[test]
    fn every_glyph_is_different_and_has_its_own_name() {
        let mut seen: Vec<(Vec<u8>, &str)> = Vec::new();
        for glyph in Glyph::ALL {
            let pixels = rgba(glyph, 16);
            for (other, name) in &seen {
                assert!(*other != pixels, "{} looks like {name}", glyph.name());
                assert_ne!(*name, glyph.name());
            }
            seen.push((pixels, glyph.name()));
        }
        // The toolbar and the states are in the set, with nothing twice.
        for glyph in Glyph::TOOLBAR.iter().chain(&Glyph::STATES) {
            assert!(Glyph::ALL.contains(glyph));
        }
        assert_eq!(
            1 + Glyph::TOOLBAR.len() + Glyph::STATES.len(),
            Glyph::ALL.len()
        );
    }

    #[test]
    fn the_states_differ_in_shape_as_well_as_in_colour() {
        // With the colour taken away, what is left is still different for
        // each state.
        let shapes: Vec<Vec<bool>> = Glyph::STATES
            .iter()
            .map(|glyph| {
                rgba(*glyph, 32)
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| pixel[3] > 127)
                    .collect()
            })
            .collect();
        for (index, shape) in shapes.iter().enumerate() {
            for other in &shapes[index + 1..] {
                let different = shape.iter().zip(other).filter(|(a, b)| a != b).count();
                assert!(different > 40, "only {different} pixels differ");
            }
        }
    }

    #[test]
    fn the_state_colours_are_the_ones_the_grid_uses() {
        // The middle of each lamp, at a size where it is flat colour.
        let middle = |glyph: Glyph| {
            let pixels = rgba(glyph, 32);
            let at = (20 * 32 + 16) * 4;
            (pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3])
        };
        let (r, g, b, a) = middle(Glyph::StateOnline);
        assert!(g > r + 60 && g > b + 60 && a == 255, "online is green");
        let (r, g, b, a) = middle(Glyph::StateIdle);
        assert!(b > r + 60 && b > g + 30 && a == 255, "idle is blue");
        // The exclamation mark is in the middle of the error lamp; look
        // beside it.
        let pixels = rgba(Glyph::StateError, 32);
        let at = (16 * 32 + 8) * 4;
        assert!(
            pixels[at] > pixels[at + 1] + 80 && pixels[at + 3] == 255,
            "error is red"
        );
        // Not ready is an empty circle: nothing in the middle.
        assert_eq!(middle(Glyph::StateNotReady).3, 0);
    }
}
