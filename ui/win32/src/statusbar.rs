// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The status bar: `msctls_statusbar32`, with parts and a size grip.

use windows::Win32::UI::Controls::{
    SB_SETPARTS, SB_SETTEXTW, SB_SIMPLE, SB_SIMPLEID, SBARS_SIZEGRIP, SBT_NOBORDERS,
    STATUSCLASSNAMEW,
};
use windows::Win32::UI::WindowsAndMessaging::WM_SIZE;

use crate::util::wide;
use crate::window::{Hwnd, Window};

/// A status bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusBar(Hwnd);

impl StatusBar {
    /// A status bar along the bottom of `parent`, with a size grip.
    pub fn create(parent: Hwnd, id: u16) -> StatusBar {
        StatusBar(Window::control(
            parent,
            STATUSCLASSNAMEW,
            "",
            SBARS_SIZEGRIP,
            0,
            id,
        ))
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Has the bar place itself along the bottom of its parent, which it
    /// does by itself when told that the size changed. Call when the
    /// parent is resized.
    pub fn place(self) {
        self.0.send(WM_SIZE, 0, 0);
    }

    /// The height of the bar in pixels, whether or not it is shown.
    pub fn height(self) -> i32 {
        self.0.screen_rect().height
    }

    /// Divides the bar into parts, each given by where its right edge is,
    /// in pixels from the left of the bar. The last may be -1, for the
    /// rest of the bar.
    pub fn set_parts(self, right_edges: &[i32]) {
        // The slice is valid for the length of the call, which copies it.
        self.0.send(
            SB_SETPARTS,
            right_edges.len(),
            right_edges.as_ptr() as isize,
        );
    }

    /// Sets the text of a part.
    pub fn set_text(self, part: usize, text: &str) {
        let text = wide(text);
        // The text is terminated UTF-16 that outlives the call, which
        // copies it.
        self.0
            .send(SB_SETTEXTW, part & 0xFF, text.as_ptr() as isize);
    }

    /// Shows one line of text across the whole bar in place of its parts,
    /// or with `None` shows the parts again. This is how a status bar
    /// describes the menu command under the highlight.
    pub fn set_simple(self, text: Option<&str>) {
        match text {
            Some(text) => {
                let text = wide(text);
                self.0.send(SB_SIMPLE, 1, 0);
                self.0.send(
                    SB_SETTEXTW,
                    (SB_SIMPLEID | SBT_NOBORDERS) as usize,
                    text.as_ptr() as isize,
                );
            }
            None => {
                self.0.send(SB_SIMPLE, 0, 0);
            }
        }
    }
}
