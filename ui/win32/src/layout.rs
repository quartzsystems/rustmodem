// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Laying out the controls of a window in rows and columns.
//!
//! A rectangle is cut into rows or into columns by a list of [`Track`]s:
//! some of a fixed size, written for 96 DPI and scaled to the window's
//! DPI, and some that share what is left in proportion to their weights.
//! The rectangles that come out are given to a [`Placement`], which moves
//! all the controls in one step with `DeferWindowPos`, so that a window
//! being resized is redrawn once and not control by control.
//!
//! A window lays itself out again whenever its size or its DPI changes.

use windows::Win32::UI::WindowsAndMessaging::{
    BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
};

use crate::metrics::scale;
use crate::window::{Hwnd, Rect};

/// One row or one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Track {
    /// A fixed size, in pixels at 96 DPI.
    Fixed(i32),
    /// A fixed size in pixels, whatever the DPI: a size that was measured
    /// from a control and is already in the window's pixels.
    Pixels(i32),
    /// A share of what is left over, in proportion to the weight.
    Fill(u32),
}

/// Cuts a length into tracks with a gap between each two. Returns where
/// each track starts, from `start`, and how long it is. `gap` is in pixels
/// at 96 DPI.
///
/// When the fixed tracks and the gaps need more than there is, the tracks
/// that fill get nothing and the fixed ones run past the end: a window
/// that small is below its minimum size.
pub fn split(start: i32, length: i32, tracks: &[Track], gap: i32, dpi: u32) -> Vec<(i32, i32)> {
    let gap = scale(gap, dpi);
    let fixed: i32 = tracks
        .iter()
        .map(|track| match track {
            Track::Fixed(size) => scale(*size, dpi),
            Track::Pixels(size) => *size,
            Track::Fill(_) => 0,
        })
        .sum();
    let gaps = gap * (tracks.len() as i32 - 1).max(0);
    let spare = (length - fixed - gaps).max(0);
    let weights: u32 = tracks
        .iter()
        .map(|track| match track {
            Track::Fill(weight) => *weight,
            _ => 0,
        })
        .sum();

    let mut out = Vec::with_capacity(tracks.len());
    let mut position = start;
    // What has been given to the filling tracks so far, so that rounding
    // does not lose pixels: the last of them takes what is left.
    let (mut given, mut weight_seen) = (0, 0u32);
    for track in tracks {
        let size = match track {
            Track::Fixed(size) => scale(*size, dpi),
            Track::Pixels(size) => *size,
            Track::Fill(weight) => {
                weight_seen += weight;
                let upto = if weights == 0 {
                    0
                } else {
                    (i64::from(spare) * i64::from(weight_seen) / i64::from(weights)) as i32
                };
                let size = upto - given;
                given = upto;
                size
            }
        };
        out.push((position, size));
        position += size + gap;
    }
    out
}

/// Cuts a rectangle into rows, top to bottom.
pub fn rows(rect: Rect, tracks: &[Track], gap: i32, dpi: u32) -> Vec<Rect> {
    split(rect.y, rect.height, tracks, gap, dpi)
        .into_iter()
        .map(|(y, height)| Rect::new(rect.x, y, rect.width, height))
        .collect()
}

/// Cuts a rectangle into columns, left to right.
pub fn columns(rect: Rect, tracks: &[Track], gap: i32, dpi: u32) -> Vec<Rect> {
    split(rect.x, rect.width, tracks, gap, dpi)
        .into_iter()
        .map(|(x, width)| Rect::new(x, rect.y, width, rect.height))
        .collect()
}

/// Where a set of controls is to go, applied in one step.
#[derive(Debug, Default)]
pub struct Placement {
    items: Vec<(Hwnd, Rect)>,
}

impl Placement {
    /// Nothing placed yet.
    pub fn new() -> Placement {
        Placement::default()
    }

    /// Places a control. The rectangle is in its parent's client area.
    pub fn place(&mut self, control: Hwnd, rect: Rect) {
        self.items.push((control, rect));
    }

    /// How many controls have been placed.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether no control has been placed.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Where a control was placed, if it was.
    pub fn rect_of(&self, control: Hwnd) -> Option<Rect> {
        self.items
            .iter()
            .rev()
            .find(|(placed, _)| *placed == control)
            .map(|(_, rect)| *rect)
    }

    /// Moves every control to its place.
    pub fn apply(self) {
        // SAFETY: the calls take a count, handles and plain numbers. A
        // handle that is no longer a window makes the batch fail, in
        // which case the controls are moved one at a time below.
        let batched = unsafe {
            let mut batch = BeginDeferWindowPos(self.items.len() as i32).ok();
            for (control, rect) in &self.items {
                batch = match batch {
                    Some(handle) => DeferWindowPos(
                        handle,
                        control.0,
                        None,
                        rect.x,
                        rect.y,
                        rect.width,
                        rect.height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                    .ok(),
                    None => None,
                };
            }
            batch.is_some_and(|handle| EndDeferWindowPos(handle).is_ok())
        };
        if !batched {
            for (control, rect) in &self.items {
                control.set_rect(*rect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_tracks_keep_their_size_and_the_rest_is_shared() {
        // A toolbar, a grid that takes what is left, a row of panels and
        // a status bar, with no gaps.
        let tracks = [
            Track::Fixed(30),
            Track::Fill(1),
            Track::Fixed(200),
            Track::Fixed(22),
        ];
        assert_eq!(
            split(0, 600, &tracks, 0, 96),
            [(0, 30), (30, 348), (378, 200), (578, 22)]
        );
        // At 150 percent the fixed tracks are half as large again.
        assert_eq!(
            split(0, 900, &tracks, 0, 144),
            [(0, 45), (45, 522), (567, 300), (867, 33)]
        );
    }

    #[test]
    fn gaps_go_between_tracks_and_not_around_them() {
        let tracks = [Track::Fixed(100), Track::Fixed(100), Track::Fill(1)];
        assert_eq!(
            split(10, 400, &tracks, 8, 96),
            [(10, 100), (118, 100), (226, 184)]
        );
        // The last track ends where the length does.
        let last = split(10, 400, &tracks, 8, 96)[2];
        assert_eq!(last.0 + last.1, 10 + 400);
        assert_eq!(split(0, 100, &[Track::Fill(1)], 8, 96), [(0, 100)]);
        assert_eq!(split(0, 100, &[], 8, 96), []);
    }

    #[test]
    fn filling_tracks_share_by_weight_and_lose_no_pixels_to_rounding() {
        let thirds = [Track::Fill(1), Track::Fill(1), Track::Fill(1)];
        let parts = split(0, 100, &thirds, 0, 96);
        assert_eq!(parts, [(0, 33), (33, 33), (66, 34)]);
        assert_eq!(parts.iter().map(|(_, size)| size).sum::<i32>(), 100);

        let weighted = [Track::Fill(1), Track::Fill(3)];
        assert_eq!(split(0, 200, &weighted, 0, 96), [(0, 50), (50, 150)]);
        for length in 0..300 {
            let parts = split(0, length, &thirds, 0, 96);
            assert_eq!(parts.iter().map(|(_, size)| size).sum::<i32>(), length);
        }
    }

    #[test]
    fn a_measured_size_is_not_scaled() {
        let tracks = [Track::Pixels(31), Track::Fill(1)];
        assert_eq!(split(0, 100, &tracks, 0, 192), [(0, 31), (31, 69)]);
    }

    #[test]
    fn too_little_room_gives_the_filling_tracks_nothing() {
        let tracks = [Track::Fixed(80), Track::Fill(1), Track::Fixed(80)];
        assert_eq!(split(0, 100, &tracks, 0, 96), [(0, 80), (80, 0), (80, 80)]);
    }

    #[test]
    fn a_rectangle_is_cut_into_rows_and_into_columns() {
        let rect = Rect::new(10, 20, 300, 200);
        assert_eq!(
            rows(rect, &[Track::Fixed(50), Track::Fill(1)], 10, 96),
            [Rect::new(10, 20, 300, 50), Rect::new(10, 80, 300, 140)]
        );
        assert_eq!(
            columns(rect, &[Track::Fill(1), Track::Fixed(100)], 10, 96),
            [Rect::new(10, 20, 190, 200), Rect::new(210, 20, 100, 200)]
        );
    }

    #[test]
    fn a_placement_remembers_where_things_go() {
        let mut placement = Placement::new();
        assert!(placement.is_empty());
        let (one, two) = (Hwnd::from_raw(1), Hwnd::from_raw(2));
        placement.place(one, Rect::new(0, 0, 10, 10));
        placement.place(two, Rect::new(5, 5, 20, 20));
        placement.place(one, Rect::new(1, 1, 10, 10));
        assert_eq!(placement.len(), 3);
        assert_eq!(placement.rect_of(one), Some(Rect::new(1, 1, 10, 10)));
        assert_eq!(placement.rect_of(Hwnd::from_raw(3)), None);
        // Handles that are not windows: applying does nothing, harmlessly.
        placement.apply();
    }
}
