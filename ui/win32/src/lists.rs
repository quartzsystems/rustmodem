// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The two plain lists: the list box, and the list view that holds its
//! own rows and has a check box on each.
//!
//! The grid of the main window is a list view that asks for every cell
//! ([`crate::ListView`]). These two are for a dialog with a handful of
//! entries, where the control keeping them is simpler than being asked.

use windows::Win32::UI::Controls::{
    LVIF_TEXT, LVIS_STATEIMAGEMASK, LVITEMW, LVM_DELETEALLITEMS, LVM_GETITEMCOUNT,
    LVM_GETITEMSTATE, LVM_INSERTITEMW, LVM_SETEXTENDEDLISTVIEWSTYLE, LVM_SETITEMSTATE,
    LVM_SETITEMTEXTW, LVS_EX_CHECKBOXES, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, SetWindowTheme,
};
use windows::Win32::UI::WindowsAndMessaging::{
    LB_ADDSTRING, LB_GETCOUNT, LB_GETCURSEL, LB_GETTEXT, LB_GETTEXTLEN, LB_RESETCONTENT,
    LB_SETCURSEL,
};
use windows::core::{PWSTR, w};

use crate::listview::{Column, ListView};
use crate::util::{from_wide, wide};
use crate::window::Hwnd;

/// A list box: entries of one line of text, one of them chosen. A handle
/// on the control: copying it copies the handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListBox(Hwnd);

impl ListBox {
    /// A list box that is already there, as one made from a dialog
    /// template is.
    pub fn of(control: Hwnd) -> ListBox {
        ListBox(control)
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
        self.0.send(LB_ADDSTRING, 0, text.as_ptr() as isize);
    }

    /// Removes every entry.
    pub fn clear(self) {
        self.0.send(LB_RESETCONTENT, 0, 0);
    }

    /// Replaces the entries, and keeps the same place chosen if there is
    /// still an entry there.
    pub fn set_entries(self, entries: &[String], selected: Option<usize>) {
        self.clear();
        for entry in entries {
            self.add(entry);
        }
        self.select(selected.filter(|index| *index < entries.len()));
    }

    /// How many entries there are.
    pub fn count(self) -> usize {
        self.0.send(LB_GETCOUNT, 0, 0).max(0) as usize
    }

    /// The text of an entry.
    pub fn entry(self, index: usize) -> String {
        let length = self.0.send(LB_GETTEXTLEN, index, 0);
        let Ok(length) = usize::try_from(length) else {
            return String::new();
        };
        let mut buffer = vec![0u16; length + 1];
        // The buffer is one longer than the length the control just gave,
        // which is what the message asks for.
        self.0.send(LB_GETTEXT, index, buffer.as_mut_ptr() as isize);
        from_wide(&buffer)
    }

    /// The entry that is chosen.
    pub fn selected(self) -> Option<usize> {
        usize::try_from(self.0.send(LB_GETCURSEL, 0, 0)).ok()
    }

    /// Chooses an entry, or with `None` none.
    pub fn select(self, index: Option<usize>) {
        self.0.send(LB_SETCURSEL, index.unwrap_or(usize::MAX), 0);
    }
}

/// The state image of a row whose box is checked; the one before it is
/// the empty box.
const CHECKED: u32 = 2;

/// A list view that keeps its own rows, with a check box at the start of
/// each. A handle on the control: copying it copies the handle.
///
/// The dialog is told of a box the user checks or clears with
/// [`crate::Event::ListCheck`]. It is told of one the program sets as
/// well, as Windows tells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckList(Hwnd);

impl CheckList {
    /// A check list that is already there, as one made from a dialog
    /// template is. Its boxes are added here.
    pub fn of(control: Hwnd) -> CheckList {
        let extended = LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        control.send(
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            extended as usize,
            extended as isize,
        );
        // SAFETY: the handle is a control and the names are literals.
        unsafe {
            let _ = SetWindowTheme(control.0, w!("Explorer"), None);
        }
        CheckList(control)
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Sets the columns.
    pub fn set_columns(self, columns: &[Column]) {
        ListView(self.0).set_columns(columns);
    }

    /// Removes every row.
    pub fn clear(self) {
        self.0.send(LVM_DELETEALLITEMS, 0, 0);
    }

    /// How many rows there are.
    pub fn row_count(self) -> usize {
        self.0.send(LVM_GETITEMCOUNT, 0, 0).max(0) as usize
    }

    /// Adds a row at the end: the text of each of its cells, and whether
    /// its box is checked.
    pub fn add_row(self, cells: &[String], checked: bool) {
        let row = self.row_count();
        for (column, cell) in cells.iter().enumerate() {
            let mut text = wide(cell);
            let mut item = LVITEMW {
                mask: LVIF_TEXT,
                iItem: row as i32,
                iSubItem: column as i32,
                pszText: PWSTR(text.as_mut_ptr()),
                ..Default::default()
            };
            // The first cell makes the row; the others are set in it.
            // The structure and the text it points at outlive the call,
            // which copies the text.
            let message = if column == 0 {
                LVM_INSERTITEMW
            } else {
                LVM_SETITEMTEXTW
            };
            self.0
                .send(message, row, std::ptr::from_mut(&mut item) as isize);
        }
        self.set_checked(row, checked);
    }

    /// Whether the box of a row is checked.
    pub fn checked(self, row: usize) -> bool {
        let state = self
            .0
            .send(LVM_GETITEMSTATE, row, LVIS_STATEIMAGEMASK.0 as isize);
        (state as u32 >> 12) == CHECKED
    }

    /// Checks or clears the box of a row.
    pub fn set_checked(self, row: usize, checked: bool) {
        let image = if checked { CHECKED } else { CHECKED - 1 };
        let mut item = LVITEMW {
            stateMask: LVIS_STATEIMAGEMASK,
            state: windows::Win32::UI::Controls::LIST_VIEW_ITEM_STATE_FLAGS(image << 12),
            ..Default::default()
        };
        // The structure outlives the call.
        self.0.send(
            LVM_SETITEMSTATE,
            row,
            std::ptr::from_mut(&mut item) as isize,
        );
    }

    /// The row that is selected.
    pub fn selected(self) -> Option<usize> {
        ListView(self.0).selected()
    }

    /// Selects a row, or with `None` clears the selection.
    pub fn select(self, row: Option<usize>) {
        ListView(self.0).select(row);
    }

    /// What a cell shows.
    pub fn cell_text(self, row: usize, column: usize) -> String {
        ListView(self.0).cell_text(row, column)
    }
}
