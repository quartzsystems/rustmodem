// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The small controls: group box, static text, edit, button, check box,
//! radio button, combo box, progress bar, tab control and up-down control.
//!
//! Each is the stock window class with the styles a stock program gives
//! it. A control is made with no size; the window that owns it places it.
//! In a dialog made from a template the controls are already there, and
//! the typed wrappers are put on them with `of`.

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Controls::{
    PBM_GETPOS, PBM_SETPOS, PBM_SETRANGE32, PROGRESS_CLASSW, TCIF_TEXT, TCITEMW, TCM_ADJUSTRECT,
    TCM_GETCURSEL, TCM_INSERTITEMW, TCM_SETCURSEL, UDM_SETBUDDY, UDM_SETRANGE32, UDS_ALIGNRIGHT,
    UDS_ARROWKEYS, UDS_NOTHOUSANDS, UDS_SETBUDDYINT, UPDOWN_CLASSW, WC_TABCONTROLW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_GROUPBOX,
    BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCOUNT, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN,
    CB_RESETCONTENT, CB_SETCURSEL, CBS_AUTOHSCROLL, CBS_DROPDOWN, CBS_DROPDOWNLIST, ES_AUTOHSCROLL,
    ES_NUMBER, WS_EX_CLIENTEDGE, WS_GROUP, WS_TABSTOP, WS_VSCROLL,
};
use windows::core::{PWSTR, w};

use crate::util::{from_wide, wide};
use crate::window::{Hwnd, Rect, Window};

/// Static control styles. They are numbers of the `STATIC` class that the
/// `windows` crate keeps among the system services.
const SS_LEFT: u32 = 0x0000;
const SS_RIGHT: u32 = 0x0002;
const SS_NOPREFIX: u32 = 0x0080;
const SS_ENDELLIPSIS: u32 = 0x4000;

/// A control with no id of its own: nothing is ever asked of it by id.
const NO_ID: u16 = 0xFFFF;

/// A group box: a frame with a title, around other controls.
pub fn group_box(parent: Hwnd, text: &str) -> Hwnd {
    Window::control(parent, w!("BUTTON"), text, BS_GROUPBOX as u32, 0, NO_ID)
}

/// Static text that names the control after it: an ampersand marks its
/// mnemonic, which moves the focus to that control.
pub fn label(parent: Hwnd, text: &str) -> Hwnd {
    Window::control(parent, w!("STATIC"), text, SS_LEFT, 0, NO_ID)
}

/// Static text that shows a value: nothing in it is a mnemonic, and text
/// too long for it ends in an ellipsis.
pub fn value(parent: Hwnd, text: &str, right_aligned: bool) -> Hwnd {
    let align = if right_aligned { SS_RIGHT } else { SS_LEFT };
    Window::control(
        parent,
        w!("STATIC"),
        text,
        align | SS_NOPREFIX | SS_ENDELLIPSIS,
        0,
        NO_ID,
    )
}

/// A single-line edit control. With `digits_only` it takes digits only.
pub fn edit(parent: Hwnd, id: u16, digits_only: bool) -> Hwnd {
    let digits = if digits_only { ES_NUMBER as u32 } else { 0 };
    Window::control(
        parent,
        w!("EDIT"),
        "",
        ES_AUTOHSCROLL as u32 | digits | WS_TABSTOP.0,
        WS_EX_CLIENTEDGE.0,
        id,
    )
}

/// A push button. With `default` it is the one Enter presses.
pub fn button(parent: Hwnd, id: u16, text: &str, default: bool) -> Hwnd {
    let kind = if default {
        BS_DEFPUSHBUTTON
    } else {
        BS_PUSHBUTTON
    };
    Window::control(
        parent,
        w!("BUTTON"),
        text,
        kind as u32 | WS_TABSTOP.0,
        0,
        id,
    )
}

/// A check box.
pub fn check_box(parent: Hwnd, id: u16, text: &str) -> Hwnd {
    Window::control(
        parent,
        w!("BUTTON"),
        text,
        BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0,
        0,
        id,
    )
}

/// A radio button. The first of a group starts the group: the arrow keys
/// move among the buttons of one group, and Tab moves past it.
pub fn radio_button(parent: Hwnd, id: u16, text: &str, first_of_group: bool) -> Hwnd {
    let group = if first_of_group {
        WS_GROUP.0 | WS_TABSTOP.0
    } else {
        0
    };
    Window::control(
        parent,
        w!("BUTTON"),
        text,
        BS_AUTORADIOBUTTON as u32 | group,
        0,
        id,
    )
}

/// Whether a check box or radio button is checked.
pub fn checked(control: Hwnd) -> bool {
    control.send(BM_GETCHECK, 0, 0) == 1
}

/// Checks or clears a check box or radio button.
pub fn set_checked(control: Hwnd, checked: bool) {
    control.send(BM_SETCHECK, usize::from(checked), 0);
}

/// A combo box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo(Hwnd);

impl Combo {
    /// A drop-down list: one of its entries is chosen, and nothing can be
    /// typed.
    pub fn drop_list(parent: Hwnd, id: u16) -> Combo {
        let style = CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0;
        Combo(Window::control(parent, w!("COMBOBOX"), "", style, 0, id))
    }

    /// A drop-down with an edit field: one of its entries is chosen, or
    /// something else is typed.
    pub fn editable(parent: Hwnd, id: u16) -> Combo {
        let style = CBS_DROPDOWN as u32 | CBS_AUTOHSCROLL as u32 | WS_VSCROLL.0 | WS_TABSTOP.0;
        Combo(Window::control(parent, w!("COMBOBOX"), "", style, 0, id))
    }

    /// A combo box that is already there.
    pub fn of(control: Hwnd) -> Combo {
        Combo(control)
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Adds an entry at the end.
    pub fn add(self, text: &str) {
        let text = wide(text);
        // The text is terminated UTF-16 that outlives the call, which
        // copies it.
        self.0.send(CB_ADDSTRING, 0, text.as_ptr() as isize);
    }

    /// Removes every entry.
    pub fn clear(self) {
        self.0.send(CB_RESETCONTENT, 0, 0);
    }

    /// How many entries there are.
    pub fn count(self) -> usize {
        self.0.send(CB_GETCOUNT, 0, 0).max(0) as usize
    }

    /// The text of an entry.
    pub fn entry(self, index: usize) -> String {
        let length = self.0.send(CB_GETLBTEXTLEN, index, 0);
        let Ok(length) = usize::try_from(length) else {
            return String::new();
        };
        let mut buffer = vec![0u16; length + 1];
        // The buffer is one longer than the length the control just gave,
        // which is what the message asks for.
        self.0
            .send(CB_GETLBTEXT, index, buffer.as_mut_ptr() as isize);
        from_wide(&buffer)
    }

    /// The entry that is chosen.
    pub fn selected(self) -> Option<usize> {
        usize::try_from(self.0.send(CB_GETCURSEL, 0, 0)).ok()
    }

    /// Chooses an entry, or with `None` none.
    pub fn select(self, index: Option<usize>) {
        self.0.send(CB_SETCURSEL, index.unwrap_or(usize::MAX), 0);
    }

    /// What the combo box shows: the entry chosen, or what was typed.
    pub fn text(self) -> String {
        self.0.text()
    }

    /// Sets what an editable combo box shows.
    pub fn set_text(self, text: &str) {
        self.0.set_text(text);
    }
}

/// A progress bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress(Hwnd);

impl Progress {
    /// A progress bar from 0 to 100.
    pub fn create(parent: Hwnd) -> Progress {
        let bar = Window::control(parent, PROGRESS_CLASSW, "", 0, 0, NO_ID);
        bar.send(PBM_SETRANGE32, 0, 100);
        Progress(bar)
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Sets how far along the bar is, from 0 to 100.
    pub fn set_percent(self, percent: u32) {
        let percent = percent.min(100) as usize;
        // The bar is animated when it moves; not moving it when it has not
        // changed keeps it still.
        if self.0.send(PBM_GETPOS, 0, 0) as usize != percent {
            self.0.send(PBM_SETPOS, percent, 0);
        }
    }

    /// How far along the bar is.
    pub fn percent(self) -> u32 {
        self.0.send(PBM_GETPOS, 0, 0).max(0) as u32
    }
}

/// An up-down control: the pair of arrows on the end of an edit control
/// that holds a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpDown(Hwnd);

impl UpDown {
    /// The styles of an up-down control that sets the number in its buddy
    /// edit control, sits inside its right end, and takes the arrow keys.
    pub(crate) const STYLE: u32 =
        UDS_SETBUDDYINT | UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_NOTHOUSANDS;

    /// An up-down control on `buddy`, for numbers from `low` to `high`.
    pub fn create(parent: Hwnd, buddy: Hwnd, low: i32, high: i32) -> UpDown {
        let arrows = Window::control(parent, UPDOWN_CLASSW, "", UpDown::STYLE, 0, NO_ID);
        UpDown(arrows).attach(buddy, low, high);
        UpDown(arrows)
    }

    /// An up-down control that is already there.
    pub fn of(control: Hwnd) -> UpDown {
        UpDown(control)
    }

    /// Attaches the control to its edit control and sets its range. The
    /// control places itself inside the edit control when this is called,
    /// so it is called again after the edit control is moved.
    pub fn attach(self, buddy: Hwnd, low: i32, high: i32) {
        self.0.send(UDM_SETBUDDY, buddy.raw() as usize, 0);
        self.0.send(UDM_SETRANGE32, low as usize, high as isize);
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }
}

/// A tab control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tabs(Hwnd);

impl Tabs {
    /// A tab control with no tabs.
    pub fn create(parent: Hwnd, id: u16) -> Tabs {
        Tabs(Window::control(
            parent,
            WC_TABCONTROLW,
            "",
            WS_TABSTOP.0,
            0,
            id,
        ))
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Adds a tab at the end.
    pub fn add(self, text: &str) {
        let mut text = wide(text);
        let mut item = TCITEMW {
            mask: TCIF_TEXT,
            pszText: PWSTR(text.as_mut_ptr()),
            ..TCITEMW::default()
        };
        // The structure and the text it points at are valid for the
        // length of the call, which copies them. An index past the end
        // adds at the end.
        self.0
            .send(TCM_INSERTITEMW, i32::MAX as usize, (&raw mut item) as isize);
    }

    /// The tab that is in front.
    pub fn selected(self) -> Option<usize> {
        usize::try_from(self.0.send(TCM_GETCURSEL, 0, 0)).ok()
    }

    /// Brings a tab to the front.
    pub fn select(self, index: usize) {
        self.0.send(TCM_SETCURSEL, index, 0);
    }

    /// The part of a rectangle given to the tab control that is left for
    /// the page, inside the tabs and the frame.
    pub fn page_rect(self, control: Rect) -> Rect {
        let mut rect = RECT {
            left: control.x,
            top: control.y,
            right: control.x + control.width,
            bottom: control.y + control.height,
        };
        // The rectangle is valid for the call to change. Zero asks for
        // the display area of a window rectangle.
        self.0.send(TCM_ADJUSTRECT, 0, (&raw mut rect) as isize);
        Rect::from_rect(rect)
    }
}
