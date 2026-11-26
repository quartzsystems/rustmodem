// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What each glyph looks like: a few shapes on the 16-unit grid.
//!
//! Outlines are a dark shade of the fill, one pixel wide at 16 pixels and
//! thicker in proportion above that; fills are saturated and shade lightly
//! from top to bottom. The line states differ in shape as well as in
//! colour, so that they can be told apart without the colour.

use crate::Glyph;
use crate::canvas::{Canvas, Paint, Rgb, arc, hex, turned};

const STEEL_LIGHT: Rgb = hex(0xF4F4F4);
const STEEL: Rgb = hex(0xC2C5CA);
const STEEL_EDGE: Rgb = hex(0x3E4248);
const GREY: Rgb = hex(0x8A8F96);
const GREY_EDGE: Rgb = hex(0x55595F);
const PAPER: Rgb = hex(0xFFFFFF);
const PAPER_SHADE: Rgb = hex(0xE9ECF0);
const BLUE_LIGHT: Rgb = hex(0x7BA6F0);
const BLUE: Rgb = hex(0x2A5FC6);
const BLUE_EDGE: Rgb = hex(0x163A82);
const GREEN_LIGHT: Rgb = hex(0x6FD46F);
const GREEN: Rgb = hex(0x23A02A);
const GREEN_EDGE: Rgb = hex(0x0F5E14);
const AMBER_LIGHT: Rgb = hex(0xFFE08A);
const AMBER: Rgb = hex(0xF0B030);
const AMBER_EDGE: Rgb = hex(0x7D5A0C);
const ORANGE: Rgb = hex(0xD9780A);
const RED_LIGHT: Rgb = hex(0xF57A6C);
const RED: Rgb = hex(0xDC2E1E);
const RED_EDGE: Rgb = hex(0x821408);
const LED_OFF: Rgb = hex(0x5B6066);

/// Draws a glyph on a canvas.
pub fn draw(glyph: Glyph, c: &mut Canvas) {
    match glyph {
        Glyph::App => app(c),
        Glyph::Answer => handset(c, -40.0, GREEN_LIGHT, GREEN, GREEN_EDGE),
        Glyph::HangUp => handset(c, 0.0, RED_LIGHT, RED, RED_EDGE),
        Glyph::Reset => reset(c),
        Glyph::NewLine => new_line(c),
        Glyph::Reregister => reregister(c),
        Glyph::Reload => reload(c),
        Glyph::Settings => settings(c),
        Glyph::StateDisabled => state_disabled(c),
        Glyph::StateNotReady => c.ring(8.0, 8.0, 4.5, 1.6, Paint::Flat(GREY)),
        Glyph::StateIdle => lamp(c, 4.6, BLUE_LIGHT, BLUE, BLUE_EDGE),
        Glyph::StateRinging => state_ringing(c),
        Glyph::StateHandshaking => state_handshaking(c),
        Glyph::StateOnline => lamp(c, 5.4, GREEN_LIGHT, GREEN, GREEN_EDGE),
        Glyph::StateError => state_error(c),
    }
}

/// The width of an outline: one grid unit, and never less than a pixel.
fn line(c: &Canvas) -> f32 {
    c.pixel().max(1.0)
}

/// A modem: a box with a row of lamps.
fn modem(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32, lamps: &[Rgb]) {
    let edge = line(c);
    c.boxed(
        [x, y, w, h],
        1.2,
        edge,
        STEEL_EDGE,
        Paint::Shade(STEEL_LIGHT, STEEL),
    );
    let middle = y + h / 2.0;
    let step = (w - 3.0) / lamps.len().max(1) as f32;
    for (index, colour) in lamps.iter().enumerate() {
        let cx = x + 2.0 + step * index as f32 + step / 2.0 - 0.5;
        c.disc(cx, middle, (h / 5.0).min(0.9), Paint::Flat(*colour));
    }
}

/// The application: a modem with its lamps lit, and a carrier above it.
fn app(c: &mut Canvas) {
    // The carrier: two cycles of a wave.
    let wave: Vec<(f32, f32)> = (0..=48)
        .map(|step| {
            let t = step as f32 / 48.0;
            (
                2.0 + 12.0 * t,
                4.0 - 2.2 * (t * 4.0 * std::f32::consts::PI).sin(),
            )
        })
        .collect();
    c.stroke(&wave, 2.4, Paint::Flat(BLUE_EDGE));
    c.stroke(&wave, 1.2, Paint::Flat(BLUE_LIGHT));
    modem(c, 1.0, 8.0, 14.0, 6.5, &[GREEN, GREEN, AMBER, LED_OFF]);
}

/// A telephone handset, turned by `degrees`: level it is on the hook,
/// tilted it is lifted.
///
/// It is the handset of a desk telephone seen from the side: a long
/// grip, bowed a little, with the earpiece at one end and the mouthpiece
/// at the other, each a block that stands out from the grip towards the
/// face.
fn handset(c: &mut Canvas, degrees: f32, light: Rgb, fill: Rgb, edge: Rgb) {
    let about = |points: &[(f32, f32)]| turned(points, 8.0, 8.0, degrees);
    // The grip: a shallow bow from end to end.
    let grip = about(&arc(8.0, 16.4, 11.0, 241.0, 299.0));
    // The earpiece and the mouthpiece, hanging from its ends.
    let ear = about(&[(3.3, 7.0), (3.3, 10.0)]);
    let mouth = about(&[(12.7, 7.0), (12.7, 10.0)]);
    let (grip_width, block_width) = (2.7, 3.7);
    let rim = 2.0 * line(c);
    c.stroke(&grip, grip_width + rim, Paint::Flat(edge));
    c.stroke(&ear, block_width + rim, Paint::Flat(edge));
    c.stroke(&mouth, block_width + rim, Paint::Flat(edge));
    c.stroke(&grip, grip_width, Paint::Shade(light, fill));
    c.stroke(&ear, block_width, Paint::Flat(fill));
    c.stroke(&mouth, block_width, Paint::Flat(fill));
}

/// An arrowhead at `tip` pointing along `degrees`.
fn arrowhead(c: &mut Canvas, tip: (f32, f32), degrees: f32, size: f32, paint: Paint) {
    let shape = [
        (tip.0, tip.1),
        (tip.0 - size, tip.1 - size * 0.8),
        (tip.0 - size, tip.1 + size * 0.8),
    ];
    c.polygon(&turned(&shape, tip.0, tip.1, degrees), paint);
}

/// An arc with an arrowhead at its end, going clockwise from `from` to
/// `to`.
fn arrow_arc(c: &mut Canvas, radius: f32, from: f32, to: f32, colour: Rgb) {
    let points = arc(8.0, 8.0, radius, from, to);
    c.stroke(&points, 1.8, Paint::Flat(colour));
    let end = points[points.len() - 1];
    // The head points along the tangent, a quarter turn on from the
    // radius, and reaches a little past the end of the arc.
    let tangent = to + 90.0;
    let (sin, cos) = tangent.to_radians().sin_cos();
    let tip = (end.0 + 2.0 * cos, end.1 + 2.0 * sin);
    arrowhead(c, tip, tangent, 3.2, Paint::Flat(colour));
}

/// Reset: once round and back to the start.
fn reset(c: &mut Canvas) {
    arrow_arc(c, 5.0, -60.0, 225.0, ORANGE);
    c.disc(8.0, 8.0, 1.5, Paint::Flat(ORANGE));
}

/// New line: a modem and a plus.
fn new_line(c: &mut Canvas) {
    modem(c, 0.5, 8.0, 11.5, 6.0, &[GREEN, LED_OFF, LED_OFF]);
    let rim = line(c);
    for (width, colour) in [(2.2 + 2.0 * rim, GREEN_EDGE), (2.2, GREEN_LIGHT)] {
        let reach = 3.4 + (width - 2.2) / 2.0;
        c.rect(
            11.3 - width / 2.0,
            4.6 - reach,
            width,
            2.0 * reach,
            0.0,
            Paint::Flat(colour),
        );
        c.rect(
            11.3 - reach,
            4.6 - width / 2.0,
            2.0 * reach,
            width,
            0.0,
            Paint::Flat(colour),
        );
    }
}

/// Re-register: out and back.
fn reregister(c: &mut Canvas) {
    for (y, from, to, degrees) in [(5.2, 2.5, 13.5, 0.0), (10.8, 13.5, 2.5, 180.0)] {
        let shaft_end = if to > from { to - 2.5 } else { to + 2.5 };
        c.stroke(&[(from, y), (shaft_end, y)], 1.8, Paint::Flat(BLUE));
        arrowhead(c, (to, y), degrees, 3.4, Paint::Flat(BLUE));
    }
}

/// Reload configuration: a sheet, read again.
fn reload(c: &mut Canvas) {
    let edge = line(c);
    c.boxed(
        [2.0, 1.0, 9.5, 13.0],
        0.6,
        edge,
        STEEL_EDGE,
        Paint::Shade(PAPER, PAPER_SHADE),
    );
    for y in [4.0, 6.5, 9.0] {
        c.rect(4.0, y, 5.5, edge.min(1.0), 0.0, Paint::Flat(GREY));
    }
    let points = arc(10.8, 11.0, 3.3, -70.0, 200.0);
    c.stroke(&points, 3.0, Paint::Flat(PAPER));
    c.stroke(&points, 1.6, Paint::Flat(GREEN));
    let end = points[points.len() - 1];
    arrowhead(
        c,
        (end.0 + 0.4, end.1 - 1.6),
        290.0,
        3.0,
        Paint::Flat(GREEN),
    );
}

/// Settings: a gear.
fn settings(c: &mut Canvas) {
    let teeth = 8;
    let gear = |outer: f32, inner: f32| -> Vec<(f32, f32)> {
        let mut points = Vec::new();
        for tooth in 0..teeth {
            let turn = 360.0 / teeth as f32;
            let at = tooth as f32 * turn;
            // Each tooth is a flat top with a flat valley after it.
            for (offset, radius) in [(-0.22, inner), (-0.16, outer), (0.16, outer), (0.22, inner)] {
                let angle = (at + offset * turn).to_radians();
                points.push((8.0 + radius * angle.cos(), 8.0 + radius * angle.sin()));
            }
        }
        points
    };
    let rim = line(c);
    c.polygon(&gear(7.2, 5.2), Paint::Flat(STEEL_EDGE));
    c.polygon(
        &gear(7.2 - rim, 5.2 - rim),
        Paint::Shade(STEEL_LIGHT, STEEL),
    );
    c.disc(8.0, 8.0, 2.3 + rim, Paint::Flat(STEEL_EDGE));
    c.disc(8.0, 8.0, 2.3, Paint::Clear);
}

/// A lit lamp: a filled circle with an outline and a highlight.
fn lamp(c: &mut Canvas, radius: f32, light: Rgb, fill: Rgb, edge: Rgb) {
    c.disc(8.0, 8.0, radius, Paint::Flat(edge));
    c.disc(8.0, 8.0, radius - line(c), Paint::Shade(light, fill));
}

/// Disabled: a circle with a bar through it.
fn state_disabled(c: &mut Canvas) {
    c.ring(8.0, 8.0, 5.0, 1.6, Paint::Flat(GREY_EDGE));
    c.stroke(&[(4.6, 11.4), (11.4, 4.6)], 1.6, Paint::Flat(GREY_EDGE));
}

/// Ringing: a lamp with the ring coming off it on both sides.
fn state_ringing(c: &mut Canvas) {
    lamp(c, 3.2, AMBER_LIGHT, AMBER, AMBER_EDGE);
    for (from, to) in [(-40.0, 40.0), (140.0, 220.0)] {
        c.stroke(&arc(8.0, 8.0, 5.4, from, to), 1.3, Paint::Flat(AMBER_EDGE));
        c.stroke(&arc(8.0, 8.0, 7.4, from, to), 1.1, Paint::Flat(AMBER));
    }
}

/// Handshaking: the two ends working toward each other.
fn state_handshaking(c: &mut Canvas) {
    for (y, from, to, degrees) in [(5.6, 2.0, 12.5, 0.0), (10.4, 14.0, 3.5, 180.0)] {
        let shaft_end = if to > from { to - 2.0 } else { to + 2.0 };
        c.stroke(&[(from, y), (shaft_end, y)], 1.7, Paint::Flat(ORANGE));
        arrowhead(c, (to, y), degrees, 3.2, Paint::Flat(ORANGE));
    }
}

/// A port error: a red lamp with an exclamation mark.
fn state_error(c: &mut Canvas) {
    lamp(c, 6.0, RED_LIGHT, RED, RED_EDGE);
    c.rect(7.1, 4.0, 1.8, 5.2, 0.6, Paint::Flat(PAPER));
    c.disc(8.0, 11.3, 1.05, Paint::Flat(PAPER));
}
