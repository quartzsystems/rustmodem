// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The rich edit control, as a log pane: `RICHEDIT50W`, read-only, with a
//! colour for each line and a limit on how many lines it keeps.
//!
//! Lines are added at the end. If the pane is showing its end it follows
//! what is added; if the user has scrolled up to read, it stays where it
//! is. When it holds more lines than its limit the oldest are removed.

use windows::Win32::UI::Controls::RichEdit::{
    CFE_AUTOCOLOR, CFE_EFFECTS, CFM_COLOR, CHARFORMATW, CHARRANGE, EM_EXLIMITTEXT, EM_EXSETSEL,
    EM_SETCHARFORMAT, MSFTEDIT_CLASS, SCF_SELECTION,
};
use windows::Win32::UI::Controls::{
    EM_GETFIRSTVISIBLELINE, EM_GETLINECOUNT, EM_LINEINDEX, EM_LINESCROLL, EM_REPLACESEL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ES_AUTOVSCROLL, ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, GetScrollInfo, SB_BOTTOM, SB_VERT,
    SCROLLINFO, SIF_ALL, WM_GETTEXTLENGTH, WM_SETREDRAW, WM_VSCROLL, WS_EX_CLIENTEDGE, WS_TABSTOP,
    WS_VSCROLL,
};

use crate::util::wide;
use crate::window::{Colour, Hwnd, Window};

/// The most text the pane takes, in characters. Far more than the line
/// limit lets it reach; the control's own default is too small for a log.
const TEXT_LIMIT: usize = 16 * 1024 * 1024;

/// A read-only rich edit control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RichEdit(Hwnd);

impl RichEdit {
    /// The styles of a log pane.
    pub(crate) const STYLE: u32 = (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_NOHIDESEL)
        as u32
        | WS_VSCROLL.0
        | WS_TABSTOP.0;

    /// Makes a log pane in `parent`.
    pub fn create(parent: Hwnd, id: u16) -> RichEdit {
        let pane = Window::control(
            parent,
            MSFTEDIT_CLASS,
            "",
            RichEdit::STYLE,
            WS_EX_CLIENTEDGE.0,
            id,
        );
        RichEdit::of(pane)
    }

    /// A rich edit control that is already there.
    pub fn of(control: Hwnd) -> RichEdit {
        control.send(EM_EXLIMITTEXT, 0, TEXT_LIMIT as isize);
        RichEdit(control)
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// How many lines the pane holds. An empty pane has one, empty, line.
    pub fn line_count(self) -> usize {
        self.0.send(EM_GETLINECOUNT, 0, 0).max(1) as usize
    }

    /// The whole text, each line ended by a carriage return and a line
    /// feed, as a Windows text file has them.
    pub fn text(self) -> String {
        self.0.text()
    }

    /// Whether the pane is showing its end, or has nothing to scroll.
    pub fn at_end(self) -> bool {
        let mut info = SCROLLINFO {
            cbSize: size_of::<SCROLLINFO>() as u32,
            fMask: SIF_ALL,
            ..SCROLLINFO::default()
        };
        // SAFETY: `info` has its size and mask filled in and is valid for
        // the call to fill.
        if unsafe { GetScrollInfo(self.0.0, SB_VERT, &mut info) }.is_err() {
            return true;
        }
        // The thumb is one page long; it is at the end when its far edge
        // has reached the range's. A line of slack covers a partly shown
        // last line.
        info.nPos + info.nPage as i32 + 4 >= info.nMax
    }

    /// Scrolls to the end.
    pub fn scroll_to_end(self) {
        self.0.send(WM_VSCROLL, SB_BOTTOM.0 as usize, 0);
    }

    /// Puts the caret, and so the next text, at the end.
    fn select_end(self) {
        let length = self.0.send(WM_GETTEXTLENGTH, 0, 0) as i32;
        let mut range = CHARRANGE {
            cpMin: length,
            cpMax: length,
        };
        // The structure is valid for the length of the call.
        self.0.send(EM_EXSETSEL, 0, (&raw mut range) as isize);
    }

    /// Adds lines at the end, each in its colour or, with `None`, in the
    /// control's own, and then removes the oldest lines beyond `limit`.
    /// The pane follows its end if it was showing it, and otherwise goes
    /// on showing the lines it was showing.
    pub fn append(self, lines: &[(String, Option<Colour>)], limit: usize) {
        if lines.is_empty() {
            return;
        }
        let follow = self.at_end();
        let first_visible = self.0.send(EM_GETFIRSTVISIBLELINE, 0, 0).max(0);
        // Nothing is drawn until everything has been done, so that the
        // pane does not jump about on the way.
        self.0.send(WM_SETREDRAW, 0, 0);
        for (text, colour) in lines {
            self.select_end();
            let mut format = CHARFORMATW {
                cbSize: size_of::<CHARFORMATW>() as u32,
                dwMask: CFM_COLOR,
                ..CHARFORMATW::default()
            };
            match colour {
                Some(colour) => format.crTextColor = colour.colorref(),
                None => format.dwEffects = CFE_EFFECTS(CFE_AUTOCOLOR.0),
            }
            // The structure is valid for the length of the call, and sets
            // the colour of what is typed at the caret next.
            self.0.send(
                EM_SETCHARFORMAT,
                SCF_SELECTION as usize,
                (&raw mut format) as isize,
            );
            let line = wide(&format!("{text}\r\n"));
            // The text is terminated UTF-16 that outlives the call. Zero
            // means the change cannot be undone, which a log has no use
            // for.
            self.0.send(EM_REPLACESEL, 0, line.as_ptr() as isize);
        }

        // The last line is the empty one after the final line ending.
        let held = self.line_count().saturating_sub(1);
        let removed = held.saturating_sub(limit);
        if removed > 0 {
            let end = self.0.send(EM_LINEINDEX, removed, 0) as i32;
            let mut range = CHARRANGE {
                cpMin: 0,
                cpMax: end,
            };
            self.0.send(EM_EXSETSEL, 0, (&raw mut range) as isize);
            // The control is read-only to the user, not to this.
            let nothing = wide("");
            self.0.send(EM_REPLACESEL, 0, nothing.as_ptr() as isize);
            self.select_end();
        }

        if follow {
            self.scroll_to_end();
        } else {
            // Back to the line that was at the top, which is now
            // `removed` lines higher up.
            let wanted = (first_visible - removed as isize).max(0);
            let now = self.0.send(EM_GETFIRSTVISIBLELINE, 0, 0).max(0);
            self.0.send(EM_LINESCROLL, 0, wanted - now);
        }
        self.0.send(WM_SETREDRAW, 1, 0);
        self.0.redraw();
    }

    /// Removes everything.
    pub fn clear(self) {
        self.0.set_text("");
    }
}
