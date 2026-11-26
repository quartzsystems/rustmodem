// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The canvas the glyphs are drawn on.
//!
//! Shapes are given in units of a 16-unit grid, whatever the size in
//! pixels. The canvas has several samples to each pixel in each direction;
//! a shape paints every sample whose centre is inside it, and the samples
//! are averaged down at the end, which smooths the edges that do not fall
//! on pixel boundaries and leaves the ones that do crisp.

/// An RGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// A colour from its `0xRRGGBB` value.
pub const fn hex(value: u32) -> Rgb {
    Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

/// Samples to each pixel, in each direction.
const SAMPLES: usize = 4;

/// The side of the grid shapes are described on.
pub const GRID: f32 = 16.0;

/// A fill: one colour, or a shade from one colour at the top of the shape
/// to another at its bottom.
#[derive(Clone, Copy)]
pub enum Paint {
    /// One colour.
    Flat(Rgb),
    /// From the first colour at the top to the second at the bottom.
    Shade(Rgb, Rgb),
    /// Nothing: what was painted there is removed.
    Clear,
}

impl Paint {
    fn at(self, t: f32) -> Option<Rgb> {
        match self {
            Paint::Flat(colour) => Some(colour),
            Paint::Shade(top, bottom) => {
                let mix =
                    |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
                Some(Rgb(
                    mix(top.0, bottom.0),
                    mix(top.1, bottom.1),
                    mix(top.2, bottom.2),
                ))
            }
            Paint::Clear => None,
        }
    }
}

/// A square canvas. Coordinates are in grid units, the origin at the top
/// left.
pub struct Canvas {
    /// Samples to a side.
    side: usize,
    /// Samples to a grid unit.
    scale: f32,
    samples: Vec<Option<Rgb>>,
}

impl Canvas {
    /// An empty canvas for a `px`-pixel square.
    pub fn new(px: usize) -> Canvas {
        let side = px * SAMPLES;
        Canvas {
            side,
            scale: side as f32 / GRID,
            samples: vec![None; side * side],
        }
    }

    /// The width of one pixel in grid units: the thinnest line that is
    /// still a whole pixel.
    pub fn pixel(&self) -> f32 {
        SAMPLES as f32 / self.scale
    }

    /// Paints every sample whose centre `inside` accepts, within a
    /// bounding box of left, top, right and bottom.
    fn paint(
        &mut self,
        bounds: (f32, f32, f32, f32),
        paint: Paint,
        inside: impl Fn(f32, f32) -> bool,
    ) {
        let (left, top, right, bottom) = bounds;
        let scale = self.scale;
        let low = |v: f32| ((v * scale).floor().max(0.0) as usize).min(self.side);
        let high = |v: f32| ((v * scale).ceil().max(0.0) as usize).min(self.side);
        let height = (bottom - top).max(f32::EPSILON);
        for sample_y in low(top)..high(bottom) {
            for sample_x in low(left)..high(right) {
                let x = (sample_x as f32 + 0.5) / scale;
                let y = (sample_y as f32 + 0.5) / scale;
                if inside(x, y) {
                    let t = ((y - top) / height).clamp(0.0, 1.0);
                    self.samples[sample_y * self.side + sample_x] = paint.at(t);
                }
            }
        }
    }

    /// A rectangle with corners of radius `radius`.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, paint: Paint) {
        self.paint((x, y, x + w, y + h), paint, |px, py| {
            let cx = px.clamp(x + radius, x + w - radius);
            let cy = py.clamp(y + radius, y + h - radius);
            let (dx, dy) = (px - cx, py - cy);
            px >= x && px <= x + w && py >= y && py <= y + h && dx * dx + dy * dy <= radius * radius
        });
    }

    /// A rectangle `[x, y, w, h]` with an outline of width `line` in
    /// `edge` around a body of `fill`.
    pub fn boxed(&mut self, rect: [f32; 4], radius: f32, line: f32, edge: Rgb, fill: Paint) {
        let [x, y, w, h] = rect;
        self.rect(x, y, w, h, radius, Paint::Flat(edge));
        self.rect(
            x + line,
            y + line,
            w - 2.0 * line,
            h - 2.0 * line,
            (radius - line).max(0.0),
            fill,
        );
    }

    /// A polygon, filled by the even-odd rule.
    pub fn polygon(&mut self, points: &[(f32, f32)], paint: Paint) {
        let fold = |pick: fn(&(f32, f32)) -> f32, start: f32, join: fn(f32, f32) -> f32| {
            points.iter().map(pick).fold(start, join)
        };
        let bounds = (
            fold(|p| p.0, f32::MAX, f32::min),
            fold(|p| p.1, f32::MAX, f32::min),
            fold(|p| p.0, f32::MIN, f32::max),
            fold(|p| p.1, f32::MIN, f32::max),
        );
        self.paint(bounds, paint, |px, py| {
            let mut inside = false;
            let mut previous = points.len() - 1;
            for current in 0..points.len() {
                let (xi, yi) = points[current];
                let (xj, yj) = points[previous];
                if (yi > py) != (yj > py) && px < (xj - xi) * (py - yi) / (yj - yi) + xi {
                    inside = !inside;
                }
                previous = current;
            }
            inside
        });
    }

    /// A line through `points`, `width` wide, with round joins and ends.
    pub fn stroke(&mut self, points: &[(f32, f32)], width: f32, paint: Paint) {
        let radius = width / 2.0;
        for segment in points.windows(2) {
            let ((ax, ay), (bx, by)) = (segment[0], segment[1]);
            let bounds = (
                ax.min(bx) - radius,
                ay.min(by) - radius,
                ax.max(bx) + radius,
                ay.max(by) + radius,
            );
            self.paint(bounds, paint, |px, py| {
                let (vx, vy) = (bx - ax, by - ay);
                let length_squared = vx * vx + vy * vy;
                let along = if length_squared == 0.0 {
                    0.0
                } else {
                    (((px - ax) * vx + (py - ay) * vy) / length_squared).clamp(0.0, 1.0)
                };
                let (dx, dy) = (px - (ax + along * vx), py - (ay + along * vy));
                dx * dx + dy * dy <= radius * radius
            });
        }
    }

    /// A filled circle.
    pub fn disc(&mut self, cx: f32, cy: f32, radius: f32, paint: Paint) {
        self.paint(
            (cx - radius, cy - radius, cx + radius, cy + radius),
            paint,
            |px, py| (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius,
        );
    }

    /// A circle `width` wide, its middle at `radius` from the centre.
    pub fn ring(&mut self, cx: f32, cy: f32, radius: f32, width: f32, paint: Paint) {
        let (inner, outer) = (radius - width / 2.0, radius + width / 2.0);
        self.paint(
            (cx - outer, cy - outer, cx + outer, cy + outer),
            paint,
            |px, py| {
                let distance_squared = (px - cx).powi(2) + (py - cy).powi(2);
                distance_squared <= outer * outer && distance_squared >= inner * inner
            },
        );
    }

    /// Averages the samples down to pixels: `px * px * 4` bytes of red,
    /// green, blue and alpha, the colour not multiplied by the alpha.
    pub fn finish(self) -> Vec<u8> {
        let px = self.side / SAMPLES;
        let mut out = vec![0u8; px * px * 4];
        for y in 0..px {
            for x in 0..px {
                let (mut red, mut green, mut blue, mut covered) = (0u32, 0u32, 0u32, 0u32);
                for sample_y in 0..SAMPLES {
                    for sample_x in 0..SAMPLES {
                        let index = (y * SAMPLES + sample_y) * self.side + x * SAMPLES + sample_x;
                        if let Some(colour) = self.samples[index] {
                            red += u32::from(colour.0);
                            green += u32::from(colour.1);
                            blue += u32::from(colour.2);
                            covered += 1;
                        }
                    }
                }
                // The colour is the average of the samples that are
                // painted; the alpha is how many of them there are.
                if let (Some(red), Some(green), Some(blue)) = (
                    red.checked_div(covered),
                    green.checked_div(covered),
                    blue.checked_div(covered),
                ) {
                    let at = (y * px + x) * 4;
                    out[at] = red as u8;
                    out[at + 1] = green as u8;
                    out[at + 2] = blue as u8;
                    out[at + 3] = (covered * 255 / (SAMPLES * SAMPLES) as u32) as u8;
                }
            }
        }
        out
    }
}

/// Points along an arc of a circle, from angle `from` to angle `to` in
/// degrees. Angles are measured clockwise from the positive x axis, as the
/// y axis points down.
pub fn arc(cx: f32, cy: f32, radius: f32, from: f32, to: f32) -> Vec<(f32, f32)> {
    let steps = ((to - from).abs() / 8.0).ceil().max(1.0) as usize;
    (0..=steps)
        .map(|step| {
            let angle = (from + (to - from) * step as f32 / steps as f32).to_radians();
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

/// Turns points by `degrees` clockwise about `(cx, cy)`.
pub fn turned(points: &[(f32, f32)], cx: f32, cy: f32, degrees: f32) -> Vec<(f32, f32)> {
    let (sin, cos) = degrees.to_radians().sin_cos();
    points
        .iter()
        .map(|(x, y)| {
            let (dx, dy) = (x - cx, y - cy);
            (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
        })
        .collect()
}
