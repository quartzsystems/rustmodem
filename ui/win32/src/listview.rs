// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The list view, in report mode with the rows held by the program.
//!
//! The control is `SysListView32` in owner-data mode: it is told how many
//! rows there are and asks for the text of each cell as it draws it
//! ([`crate::Event::ListCell`]), so a row that changes is redrawn by
//! saying which row, and nothing is copied into the control. Full-row
//! select and double buffering are on and grid lines are off. The only
//! custom drawing is the colour of a cell's text
//! ([`crate::Event::ListCellColour`]).

use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Controls::{
    HDF_SORTDOWN, HDF_SORTUP, HDI_FORMAT, HDITEMW, HDM_GETITEMW, HDM_SETITEMW,
    LIST_VIEW_ITEM_STATE_FLAGS, LVCF_FMT, LVCF_SUBITEM, LVCF_TEXT, LVCF_WIDTH, LVCFMT_LEFT,
    LVCFMT_RIGHT, LVCOLUMNW, LVIS_FOCUSED, LVIS_SELECTED, LVITEMW, LVM_DELETECOLUMN,
    LVM_ENSUREVISIBLE, LVM_GETCOLUMNWIDTH, LVM_GETCOUNTPERPAGE, LVM_GETHEADER, LVM_GETITEMCOUNT,
    LVM_GETITEMRECT, LVM_GETITEMTEXTW, LVM_GETNEXTITEM, LVM_GETTOPINDEX, LVM_INSERTCOLUMNW,
    LVM_REDRAWITEMS, LVM_SCROLL, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVM_SETIMAGELIST, LVM_SETITEMCOUNT, LVM_SETITEMSTATE, LVNI_SELECTED, LVS_EX_DOUBLEBUFFER,
    LVS_EX_FULLROWSELECT, LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL,
    LVSICF_NOINVALIDATEALL, LVSICF_NOSCROLL, LVSIL_SMALL, SetWindowTheme, WC_LISTVIEWW,
};
use windows::Win32::UI::Controls::{LVN_COLUMNCLICK, NMHDR, NMLISTVIEW};
use windows::Win32::UI::WindowsAndMessaging::{WM_NOTIFY, WS_BORDER, WS_TABSTOP};
use windows::core::{PWSTR, w};

use crate::imagelist::ImageList;
use crate::util::wide;
use crate::window::{Hwnd, Rect, Window};

pub use crate::window::CellText as Cell;

/// How the text of a column is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// To the left: text.
    Left,
    /// To the right: numbers.
    Right,
}

/// Which way a column is sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// Smallest first: the arrow points up.
    Ascending,
    /// Largest first: the arrow points down.
    Descending,
}

/// A column of a list view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    /// The text of the header.
    pub title: String,
    /// The width in pixels.
    pub width: i32,
    /// How the text is set.
    pub align: Align,
}

impl Column {
    /// A column.
    pub fn new(title: &str, width: i32, align: Align) -> Column {
        Column {
            title: title.to_string(),
            width,
            align,
        }
    }
}

/// A list view. A handle on the control: copying it copies the handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListView(pub(crate) Hwnd);

impl ListView {
    /// Makes a list view in `parent`, with no columns and no rows.
    pub fn create(parent: Hwnd, id: u16) -> ListView {
        let style = LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_SHOWSELALWAYS;
        let list = Window::control(
            parent,
            WC_LISTVIEWW,
            "",
            style | WS_TABSTOP.0 | WS_BORDER.0,
            0,
            id,
        );
        ListView(list).prepare();
        ListView(list)
    }

    /// A list view that is already there, as one made from a dialog
    /// template is. Its styles are set as [`ListView::create`] sets them.
    pub fn of(control: Hwnd) -> ListView {
        ListView(control).prepare();
        ListView(control)
    }

    fn prepare(self) {
        // LVS_EX_INFOTIP: the list asks for the tip of the row the
        // pointer rests on.
        let extended = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | 0x0000_0400;
        self.0.send(
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            extended as usize,
            extended as isize,
        );
        // The look of the list views of Explorer: the selection and the
        // row under the pointer are drawn as they are there.
        // SAFETY: the handle is a control and the names are literals.
        unsafe {
            let _ = SetWindowTheme(self.0.0, w!("Explorer"), None);
        }
    }

    /// The control.
    pub fn hwnd(self) -> Hwnd {
        self.0
    }

    /// Replaces the columns.
    pub fn set_columns(self, columns: &[Column]) {
        while self.0.send(LVM_DELETECOLUMN, 0, 0) != 0 {}
        // The first column of a list view is always set to the left,
        // whatever it is asked for. A column put in ahead of it and taken
        // out again afterwards is the way to have it otherwise.
        let mut spare = LVCOLUMNW {
            mask: LVCF_WIDTH,
            cx: 0,
            ..LVCOLUMNW::default()
        };
        self.0.send(LVM_INSERTCOLUMNW, 0, (&raw mut spare) as isize);
        for (index, column) in columns.iter().enumerate() {
            let mut title = wide(&column.title);
            let mut description = LVCOLUMNW {
                mask: LVCF_FMT | LVCF_WIDTH | LVCF_TEXT | LVCF_SUBITEM,
                fmt: match column.align {
                    Align::Left => LVCFMT_LEFT,
                    Align::Right => LVCFMT_RIGHT,
                },
                cx: column.width,
                pszText: PWSTR(title.as_mut_ptr()),
                iSubItem: index as i32,
                ..LVCOLUMNW::default()
            };
            // The structure and the title it points at are valid for the
            // length of the call, which copies them.
            self.0.send(
                LVM_INSERTCOLUMNW,
                index + 1,
                (&raw mut description) as isize,
            );
        }
        self.0.send(LVM_DELETECOLUMN, 0, 0);
    }

    /// The width of a column in pixels.
    pub fn column_width(self, column: usize) -> i32 {
        self.0.send(LVM_GETCOLUMNWIDTH, column, 0) as i32
    }

    /// Sets the width of a column in pixels.
    pub fn set_column_width(self, column: usize, width: i32) {
        self.0.send(LVM_SETCOLUMNWIDTH, column, width as isize);
    }

    /// How many columns there are.
    pub fn column_count(self) -> usize {
        /// `HDM_GETITEMCOUNT`.
        const HDM_GETITEMCOUNT: u32 = 0x1200;
        self.header().send(HDM_GETITEMCOUNT, 0, 0).max(0) as usize
    }

    fn header(self) -> Hwnd {
        Hwnd::from_raw(self.0.send(LVM_GETHEADER, 0, 0))
    }

    /// Shows the sort arrow in the header of one column, or of none.
    pub fn set_sort_arrow(self, sorted: Option<(usize, Sort)>) {
        let header = self.header();
        for column in 0..self.column_count() {
            let mut item = HDITEMW {
                mask: HDI_FORMAT,
                ..HDITEMW::default()
            };
            // The structure is valid for the call to fill.
            header.send(HDM_GETITEMW, column, (&raw mut item) as isize);
            item.fmt.0 &= !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
            match sorted {
                Some((sorted_column, Sort::Ascending)) if sorted_column == column => {
                    item.fmt.0 |= HDF_SORTUP.0;
                }
                Some((sorted_column, Sort::Descending)) if sorted_column == column => {
                    item.fmt.0 |= HDF_SORTDOWN.0;
                }
                _ => {}
            }
            header.send(HDM_SETITEMW, column, (&raw mut item) as isize);
        }
    }

    /// Gives the list the images that rows start with. The list must
    /// outlive the control's use of it.
    pub fn set_images(self, images: &ImageList) {
        self.0
            .send(LVM_SETIMAGELIST, LVSIL_SMALL as usize, images.handle().0);
    }

    /// Says how many rows there are. The scroll position is kept, and
    /// only the rows that came or went are redrawn.
    pub fn set_row_count(self, rows: usize) {
        self.0.send(
            LVM_SETITEMCOUNT,
            rows,
            (LVSICF_NOSCROLL | LVSICF_NOINVALIDATEALL) as isize,
        );
    }

    /// How many rows the list has been told there are.
    pub fn row_count(self) -> usize {
        self.0.send(LVM_GETITEMCOUNT, 0, 0).max(0) as usize
    }

    /// Has one row drawn again: its text will be asked for.
    pub fn redraw_row(self, row: usize) {
        self.redraw_rows(row, row);
    }

    /// Has the rows from `first` to `last` drawn again.
    pub fn redraw_rows(self, first: usize, last: usize) {
        self.0.send(LVM_REDRAWITEMS, first, last as isize);
    }

    /// Has every row that is in view drawn again.
    pub fn redraw_visible(self) {
        let top = self.top_row();
        let page = self.0.send(LVM_GETCOUNTPERPAGE, 0, 0).max(0) as usize;
        self.redraw_rows(top, top + page + 1);
    }

    /// The selected row.
    pub fn selected(self) -> Option<usize> {
        let row = self
            .0
            .send(LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
        usize::try_from(row).ok()
    }

    /// Selects a row and gives it the focus within the list, or with
    /// `None` clears the selection.
    pub fn select(self, row: Option<usize>) {
        let selected_and_focused = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
        let mut item = LVITEMW {
            stateMask: selected_and_focused,
            ..LVITEMW::default()
        };
        // Clear every row, then set the one. The structure is valid for
        // the length of each call.
        self.0
            .send(LVM_SETITEMSTATE, usize::MAX, (&raw mut item) as isize);
        if let Some(row) = row {
            item.state = selected_and_focused;
            self.0.send(LVM_SETITEMSTATE, row, (&raw mut item) as isize);
        }
    }

    /// Tells the window the list is in that a column header was clicked,
    /// as the list does when one is. For tests, which have no pointer.
    pub fn click_column(self, column: usize) {
        let Some(parent) = self.0.parent() else {
            return;
        };
        let notification = NMLISTVIEW {
            hdr: NMHDR {
                hwndFrom: self.0.0,
                idFrom: self.0.control_id(),
                code: LVN_COLUMNCLICK,
            },
            iItem: -1,
            iSubItem: column as i32,
            ..NMLISTVIEW::default()
        };
        // The structure is valid for the length of the call.
        parent.send(
            WM_NOTIFY,
            notification.hdr.idFrom,
            (&raw const notification) as isize,
        );
    }

    /// Scrolls so that a row is in view.
    pub fn ensure_visible(self, row: usize) {
        self.0.send(LVM_ENSUREVISIBLE, row, 0);
    }

    /// The first row in view.
    pub fn top_row(self) -> usize {
        self.0.send(LVM_GETTOPINDEX, 0, 0).max(0) as usize
    }

    /// Scrolls so that `row` is the first row in view, as far as the
    /// number of rows allows.
    pub fn set_top_row(self, row: usize) {
        let mut bounds = RECT::default();
        // The rectangle is valid for the call to fill. The height of one
        // row is what a scroll is measured in.
        if self.0.send(LVM_GETITEMRECT, 0, (&raw mut bounds) as isize) == 0 {
            return;
        }
        let row_height = (bounds.bottom - bounds.top).max(1) as isize;
        let delta = row as isize - self.top_row() as isize;
        self.0.send(LVM_SCROLL, 0, delta * row_height);
    }

    /// The text of a cell, as the list shows it: the list asks its owner
    /// for it, as it does when it draws. For exporting what is shown, and
    /// for tests.
    pub fn cell_text(self, row: usize, column: usize) -> String {
        let mut buffer = [0u16; 512];
        let mut item = LVITEMW {
            iSubItem: column as i32,
            pszText: PWSTR(buffer.as_mut_ptr()),
            cchTextMax: buffer.len() as i32,
            ..LVITEMW::default()
        };
        // The structure and the buffer it points at are valid for the
        // length of the call, which writes at most `cchTextMax` units.
        self.0.send(LVM_GETITEMTEXTW, row, (&raw mut item) as isize);
        crate::util::from_wide(&buffer)
    }

    /// The rectangle of a row on the screen, for placing a context menu
    /// asked for from the keyboard.
    pub fn row_screen_rect(self, row: usize) -> Option<Rect> {
        let mut bounds = RECT::default();
        if self
            .0
            .send(LVM_GETITEMRECT, row, (&raw mut bounds) as isize)
            == 0
        {
            return None;
        }
        let origin = self.0.screen_rect();
        let mut rect = Rect::from_rect(bounds);
        rect.x += origin.x;
        rect.y += origin.y;
        Some(rect)
    }
}
