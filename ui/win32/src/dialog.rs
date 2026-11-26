// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Dialogs, built in memory and run by the dialog manager.
//!
//! A [`DialogTemplate`] is what a resource script would hold: a title, a
//! size and a list of controls, all in dialog units. It is written out as
//! a `DLGTEMPLATEEX` and given to `DialogBoxIndirectParam` or
//! `CreateDialogIndirectParam`, so that everything a dialog does in
//! Windows is done by Windows: the tab order is the order of the
//! controls, Enter presses the default button, Escape cancels, mnemonics
//! move the focus, and the size follows the font and the DPI.
//!
//! The font of the template is the system's message font, read when the
//! template is written, so that no font is named here.
//!
//! The events of a dialog go to a [`Handler`], as a window's do. A
//! command with the id of OK or Cancel that the handler leaves alone ends
//! a modal dialog with that id and closes a modeless one.

use std::rc::Rc;

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::UI::Controls::RichEdit::MSFTEDIT_CLASS;
use windows::Win32::UI::Controls::{
    LVS_OWNERDATA, LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL, PROGRESS_CLASSW, UDS_AUTOBUDDY,
    UPDOWN_CLASSW, WC_LISTVIEWW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON,
    CBS_AUTOHSCROLL, CBS_DROPDOWN, CBS_DROPDOWNLIST, CreateDialogIndirectParamW, DLGTEMPLATE,
    DS_CONTROL, DS_MODALFRAME, DS_SETFONT, DialogBoxIndirectParamW, ES_AUTOHSCROLL, ES_NUMBER,
    ES_READONLY, EndDialog, GW_OWNER, GetWindow, MapDialogRect, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, SetWindowPos, WS_BORDER, WS_CAPTION, WS_CHILD, WS_EX_CLIENTEDGE, WS_GROUP,
    WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::controls::UpDown;
use crate::font::message_face_and_points;
use crate::richedit::RichEdit;
use crate::util::instance;
use crate::window::{
    Handler, Hwnd, Rect, add_dialog_window, dialog_proc, id, is_dialog_window, page_proc,
};

/// The margin of a dialog, the height of its buttons and their width, and
/// the gap between related controls, in dialog units: the sizes the
/// Windows guidelines give.
pub const MARGIN: i16 = 7;
/// The height of a push button.
pub const BUTTON_HEIGHT: i16 = 14;
/// The width of a push button.
pub const BUTTON_WIDTH: i16 = 50;
/// The gap between related controls.
pub const GAP: i16 = 4;
/// The height of an edit control or a combo box.
pub const FIELD_HEIGHT: i16 = 14;
/// The height of a line of static text.
pub const TEXT_HEIGHT: i16 = 8;

/// The class of a control in a template.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Class {
    /// One of the system's classes, which a template names by number.
    System(u16),
    /// A class named by its name.
    Named(String),
}

const CLASS_BUTTON: u16 = 0x0080;
const CLASS_EDIT: u16 = 0x0081;
const CLASS_STATIC: u16 = 0x0082;
const CLASS_LISTBOX: u16 = 0x0083;
const CLASS_COMBO: u16 = 0x0085;

/// Static control styles; see `controls`.
const SS_RIGHT: u32 = 0x0002;
const SS_ICON: u32 = 0x0003;
const SS_NOPREFIX: u32 = 0x0080;
const SS_ENDELLIPSIS: u32 = 0x4000;

/// The id of a static control nothing asks for.
const STATIC_ID: u16 = 0xFFFF;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    class: Class,
    text: String,
    id: u16,
    rect: (i16, i16, i16, i16),
    style: u32,
    ex_style: u32,
}

/// A dialog, described in dialog units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogTemplate {
    title: String,
    width: i16,
    height: i16,
    items: Vec<Item>,
}

/// A modeless dialog: a window that stays open beside the main window.
/// Destroyed when dropped.
pub struct ModelessDialog {
    window: Hwnd,
}

impl ModelessDialog {
    /// The dialog's window.
    pub fn hwnd(&self) -> Hwnd {
        self.window
    }
}

impl Drop for ModelessDialog {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

impl DialogTemplate {
    /// A dialog with no controls. The size is that of its client area.
    pub fn new(title: &str, width: i16, height: i16) -> DialogTemplate {
        DialogTemplate {
            title: title.to_string(),
            width,
            height,
            items: Vec::new(),
        }
    }

    /// The width of the dialog's client area, in dialog units.
    pub fn width(&self) -> i16 {
        self.width
    }

    /// The height of the dialog's client area, in dialog units.
    pub fn height(&self) -> i16 {
        self.height
    }

    fn item(
        mut self,
        class: Class,
        id: u16,
        rect: (i16, i16, i16, i16),
        text: &str,
        style: u32,
        ex_style: u32,
    ) -> DialogTemplate {
        self.items.push(Item {
            class,
            text: text.to_string(),
            id,
            rect,
            style: style | WS_CHILD.0 | WS_VISIBLE.0,
            ex_style,
        });
        self
    }

    /// Static text that names the control after it. An ampersand marks
    /// its mnemonic, which moves the focus to that control.
    pub fn label(self, x: i16, y: i16, w: i16, h: i16, text: &str) -> DialogTemplate {
        self.item(
            Class::System(CLASS_STATIC),
            STATIC_ID,
            (x, y, w, h),
            text,
            0,
            0,
        )
    }

    /// Static text that shows a value, to be set by its id. Nothing in it
    /// is a mnemonic.
    pub fn value(self, id: u16, x: i16, y: i16, w: i16, h: i16, text: &str) -> DialogTemplate {
        let style = SS_NOPREFIX | SS_ENDELLIPSIS;
        self.item(
            Class::System(CLASS_STATIC),
            id,
            (x, y, w, h),
            text,
            style,
            0,
        )
    }

    /// Static text that shows a value, set to the right.
    pub fn value_right(
        self,
        id: u16,
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        text: &str,
    ) -> DialogTemplate {
        let style = SS_NOPREFIX | SS_RIGHT;
        self.item(
            Class::System(CLASS_STATIC),
            id,
            (x, y, w, h),
            text,
            style,
            0,
        )
    }

    /// The program's icon, at the size of a large icon.
    pub fn icon(self, id: u16, x: i16, y: i16) -> DialogTemplate {
        // An icon control sizes itself to its icon.
        self.item(
            Class::System(CLASS_STATIC),
            id,
            (x, y, 20, 20),
            "",
            SS_ICON,
            0,
        )
    }

    /// A group box.
    pub fn group(self, x: i16, y: i16, w: i16, h: i16, text: &str) -> DialogTemplate {
        self.item(
            Class::System(CLASS_BUTTON),
            STATIC_ID,
            (x, y, w, h),
            text,
            BS_GROUPBOX as u32,
            0,
        )
    }

    /// A single-line edit control.
    pub fn edit(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        let style = ES_AUTOHSCROLL as u32 | WS_TABSTOP.0;
        self.item(
            Class::System(CLASS_EDIT),
            id,
            (x, y, w, h),
            "",
            style,
            WS_EX_CLIENTEDGE.0,
        )
    }

    /// A single-line edit control that shows a dot for each character
    /// typed: for a password.
    pub fn password(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        // ES_PASSWORD.
        let style = ES_AUTOHSCROLL as u32 | 0x0020 | WS_TABSTOP.0;
        self.item(
            Class::System(CLASS_EDIT),
            id,
            (x, y, w, h),
            "",
            style,
            WS_EX_CLIENTEDGE.0,
        )
    }

    /// An edit control that takes digits only.
    pub fn number(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        let style = (ES_AUTOHSCROLL | ES_NUMBER) as u32 | WS_TABSTOP.0;
        self.item(
            Class::System(CLASS_EDIT),
            id,
            (x, y, w, h),
            "",
            style,
            WS_EX_CLIENTEDGE.0,
        )
    }

    /// An edit control that shows text and does not take any: it can be
    /// selected and copied from.
    pub fn read_only(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        let style = (ES_AUTOHSCROLL | ES_READONLY) as u32 | WS_TABSTOP.0;
        self.item(
            Class::System(CLASS_EDIT),
            id,
            (x, y, w, h),
            "",
            style,
            WS_EX_CLIENTEDGE.0,
        )
    }

    /// An up-down control on the edit control just before it, which it
    /// finds by itself. Give it its range with [`UpDown::attach`] when the
    /// dialog starts.
    pub fn up_down(self, id: u16) -> DialogTemplate {
        // It places itself inside its edit control.
        self.item(
            Class::Named(unsafe_free_name(UPDOWN_CLASSW)),
            id,
            (0, 0, 10, 14),
            "",
            UpDown::STYLE | UDS_AUTOBUDDY,
            0,
        )
    }

    /// A drop-down list. `h` is the height with the list dropped.
    pub fn combo(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        let style = CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0 | WS_TABSTOP.0;
        self.item(Class::System(CLASS_COMBO), id, (x, y, w, h), "", style, 0)
    }

    /// A drop-down with an edit field. `h` is the height with the list
    /// dropped.
    pub fn combo_editable(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        let style = (CBS_DROPDOWN | CBS_AUTOHSCROLL) as u32 | WS_VSCROLL.0 | WS_TABSTOP.0;
        self.item(Class::System(CLASS_COMBO), id, (x, y, w, h), "", style, 0)
    }

    /// A check box.
    pub fn check_box(self, id: u16, x: i16, y: i16, w: i16, h: i16, text: &str) -> DialogTemplate {
        let style = BS_AUTOCHECKBOX as u32 | WS_TABSTOP.0;
        self.item(
            Class::System(CLASS_BUTTON),
            id,
            (x, y, w, h),
            text,
            style,
            0,
        )
    }

    /// A radio button. The first of a group starts the group.
    #[allow(clippy::too_many_arguments)]
    pub fn radio_button(
        self,
        id: u16,
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        text: &str,
        first_of_group: bool,
    ) -> DialogTemplate {
        let group = if first_of_group {
            WS_GROUP.0 | WS_TABSTOP.0
        } else {
            0
        };
        let style = BS_AUTORADIOBUTTON as u32 | group;
        self.item(
            Class::System(CLASS_BUTTON),
            id,
            (x, y, w, h),
            text,
            style,
            0,
        )
    }

    /// A push button.
    pub fn button(self, id: u16, x: i16, y: i16, w: i16, h: i16, text: &str) -> DialogTemplate {
        // A control after a group of radio buttons starts a group of its
        // own, which ends theirs.
        let style = BS_PUSHBUTTON as u32 | WS_TABSTOP.0 | WS_GROUP.0;
        self.item(
            Class::System(CLASS_BUTTON),
            id,
            (x, y, w, h),
            text,
            style,
            0,
        )
    }

    /// The push button Enter presses.
    pub fn default_button(
        self,
        id: u16,
        x: i16,
        y: i16,
        w: i16,
        h: i16,
        text: &str,
    ) -> DialogTemplate {
        let style = BS_DEFPUSHBUTTON as u32 | WS_TABSTOP.0 | WS_GROUP.0;
        self.item(
            Class::System(CLASS_BUTTON),
            id,
            (x, y, w, h),
            text,
            style,
            0,
        )
    }

    /// A list view in report mode. Put a [`crate::ListView`] on it with
    /// `ListView::of` when the dialog starts.
    pub fn list_view(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        // Owner data, one selection, the selection shown without the
        // focus: as `ListView::create` makes one.
        let style = LVS_REPORT | LVS_OWNERDATA | LVS_SINGLESEL | LVS_SHOWSELALWAYS;
        self.item(
            Class::Named(unsafe_free_name(WC_LISTVIEWW)),
            id,
            (x, y, w, h),
            "",
            style | WS_TABSTOP.0 | WS_BORDER.0,
            0,
        )
    }

    /// A list view in report mode that keeps its own rows and has a check
    /// box on each. Put a [`crate::CheckList`] on it with `CheckList::of`
    /// when the dialog starts.
    pub fn check_list(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        let style = LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS;
        self.item(
            Class::Named(unsafe_free_name(WC_LISTVIEWW)),
            id,
            (x, y, w, h),
            "",
            style | WS_TABSTOP.0 | WS_BORDER.0,
            0,
        )
    }

    /// A list box: entries of one line, in the order they are added, one
    /// of them chosen. Choosing one is a command with the code
    /// [`crate::code::LIST_SELECTED`], and a double click one with
    /// [`crate::code::LIST_DOUBLE_CLICK`].
    pub fn list_box(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        // LBS_NOTIFY, and LBS_NOINTEGRALHEIGHT so that the box is the
        // height the template gives it.
        let style = 0x0001 | 0x0100 | WS_VSCROLL.0 | WS_TABSTOP.0 | WS_BORDER.0;
        self.item(Class::System(CLASS_LISTBOX), id, (x, y, w, h), "", style, 0)
    }

    /// A read-only rich edit control, for a log or a terminal.
    pub fn rich_edit(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        self.item(
            Class::Named(unsafe_free_name(MSFTEDIT_CLASS)),
            id,
            (x, y, w, h),
            "",
            RichEdit::STYLE,
            WS_EX_CLIENTEDGE.0,
        )
    }

    /// A progress bar.
    pub fn progress(self, id: u16, x: i16, y: i16, w: i16, h: i16) -> DialogTemplate {
        self.item(
            Class::Named(unsafe_free_name(PROGRESS_CLASSW)),
            id,
            (x, y, w, h),
            "",
            0,
            0,
        )
    }

    /// OK and Cancel at the bottom right, OK the default.
    pub fn ok_cancel(self) -> DialogTemplate {
        let y = self.height - MARGIN - BUTTON_HEIGHT;
        let cancel_x = self.width - MARGIN - BUTTON_WIDTH;
        let ok_x = cancel_x - GAP - BUTTON_WIDTH;
        self.default_button(id::OK, ok_x, y, BUTTON_WIDTH, BUTTON_HEIGHT, "OK")
            .button(
                id::CANCEL,
                cancel_x,
                y,
                BUTTON_WIDTH,
                BUTTON_HEIGHT,
                "Cancel",
            )
    }

    /// Close at the bottom right, the default, with the id of Cancel so
    /// that Escape presses it too.
    pub fn close_button(self) -> DialogTemplate {
        let y = self.height - MARGIN - BUTTON_HEIGHT;
        let x = self.width - MARGIN - BUTTON_WIDTH;
        self.default_button(id::CANCEL, x, y, BUTTON_WIDTH, BUTTON_HEIGHT, "Close")
    }

    /// The ids of the controls, in tab order. For tests.
    pub fn ids(&self) -> Vec<u16> {
        self.items.iter().map(|item| item.id).collect()
    }

    /// The template as a `DLGTEMPLATEEX`, in 32-bit words so that it is
    /// aligned as the dialog manager wants. With `page` it is a page of a
    /// property sheet: a child with no frame.
    pub(crate) fn words(&self, page: bool) -> Vec<u32> {
        let style = if page {
            WS_CHILD.0 | (DS_CONTROL | DS_SETFONT) as u32
        } else {
            WS_POPUP.0 | WS_CAPTION.0 | WS_SYSMENU.0 | (DS_MODALFRAME | DS_SETFONT) as u32
        };
        let (face, points) = message_face_and_points();
        let mut out: Vec<u16> = Vec::new();
        let dword = |out: &mut Vec<u16>, value: u32| {
            out.push(value as u16);
            out.push((value >> 16) as u16);
        };
        let text = |out: &mut Vec<u16>, value: &str| {
            out.extend(value.encode_utf16().take_while(|unit| *unit != 0));
            out.push(0);
        };
        // DLGTEMPLATEEX: version 1, the signature of the extended form.
        out.push(1);
        out.push(0xFFFF);
        dword(&mut out, 0); // help id
        dword(&mut out, 0); // extended style
        dword(&mut out, style);
        out.push(self.items.len() as u16);
        out.push(0); // x
        out.push(0); // y
        out.push(self.width as u16);
        out.push(self.height as u16);
        out.push(0); // no menu
        out.push(0); // the dialog class
        text(&mut out, &self.title);
        // DS_SETFONT: point size, weight (0 is whatever the font is), not
        // italic, the default character set, and the face.
        out.push(points);
        out.push(0);
        out.push(1 << 8);
        text(&mut out, &face);

        for item in &self.items {
            // Each DLGITEMTEMPLATEEX starts on a 32-bit boundary.
            if out.len() % 2 == 1 {
                out.push(0);
            }
            dword(&mut out, 0); // help id
            dword(&mut out, item.ex_style);
            dword(&mut out, item.style);
            let (x, y, w, h) = item.rect;
            out.extend([x as u16, y as u16, w as u16, h as u16]);
            dword(&mut out, u32::from(item.id));
            match &item.class {
                Class::System(number) => {
                    out.push(0xFFFF);
                    out.push(*number);
                }
                Class::Named(name) => text(&mut out, name),
            }
            text(&mut out, &item.text);
            out.push(0); // no creation data
        }
        if out.len() % 2 == 1 {
            out.push(0);
        }
        out.as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u32::from(pair[0]) | (u32::from(pair[1]) << 16))
            .collect()
    }

    /// Shows the dialog and returns when it is closed, with the id it was
    /// ended with: [`id::OK`], [`id::CANCEL`], or whatever the handler
    /// gave [`Hwnd::end_dialog`]. If the dialog cannot be shown the answer
    /// is [`id::CANCEL`].
    pub fn show_modal(&self, owner: Option<Hwnd>, handler: Rc<dyn Handler>) -> u16 {
        let template = self.words(false);
        // The dialog procedure takes this over at WM_INITDIALOG and frees
        // it at WM_NCDESTROY.
        let state = Box::into_raw(Box::new(handler));
        // SAFETY: the template is a whole DLGTEMPLATEEX, aligned to 32
        // bits, that outlives the call; the procedure is this crate's; and
        // the parameter is a valid pointer the procedure takes ownership
        // of. The call returns when the dialog has been destroyed.
        let result = unsafe {
            DialogBoxIndirectParamW(
                Some(instance()),
                template.as_ptr().cast::<DLGTEMPLATE>(),
                owner.map(|owner| owner.0),
                Some(dialog_proc),
                LPARAM(state as isize),
            )
        };
        u16::try_from(result).unwrap_or(id::CANCEL)
    }

    /// Makes the dialog as a window that stays open beside its owner, not
    /// yet shown.
    pub fn create_modeless(
        &self,
        owner: Option<Hwnd>,
        handler: Rc<dyn Handler>,
    ) -> Option<ModelessDialog> {
        let template = self.words(false);
        let state = Box::into_raw(Box::new(handler));
        // SAFETY: as in `show_modal`. The template is copied by the call.
        let created = unsafe {
            CreateDialogIndirectParamW(
                Some(instance()),
                template.as_ptr().cast::<DLGTEMPLATE>(),
                owner.map(|owner| owner.0),
                Some(dialog_proc),
                LPARAM(state as isize),
            )
        };
        let window = created.ok()?;
        // The message loop offers it keys for dialog navigation.
        add_dialog_window(window);
        Some(ModelessDialog {
            window: Hwnd(window),
        })
    }

    pub(crate) fn page_procedure() -> windows::Win32::UI::WindowsAndMessaging::DLGPROC {
        Some(page_proc)
    }
}

/// The name of a class the `windows` crate gives as a pointer to a wide
/// literal.
fn unsafe_free_name(name: windows::core::PCWSTR) -> String {
    // SAFETY: the pointer is one of the crate's own terminated wide
    // literals.
    unsafe { name.to_string() }.unwrap_or_default()
}

impl Hwnd {
    /// Ends a modal dialog with a result, which is what
    /// [`DialogTemplate::show_modal`] returns. A modeless dialog is
    /// closed.
    pub fn end_dialog(self, result: u16) {
        if is_dialog_window(self.0) {
            self.destroy();
            return;
        }
        // SAFETY: the call takes any handle; for one that is not a modal
        // dialog it fails.
        unsafe {
            let _ = EndDialog(self.0, isize::from(result as i16).max(0));
        }
    }

    /// A rectangle in dialog units as pixels of this dialog.
    pub fn dialog_units(self, rect: Rect) -> Rect {
        let mut mapped = RECT {
            left: rect.x,
            top: rect.y,
            right: rect.x + rect.width,
            bottom: rect.y + rect.height,
        };
        // SAFETY: `mapped` is valid for the call to change.
        unsafe {
            let _ = MapDialogRect(self.0, &mut mapped);
        }
        Rect::from_rect(mapped)
    }

    /// Puts the window in the middle of its owner, as a dialog is.
    pub(crate) fn centre_on_owner(self) {
        // SAFETY: the calls take any handle and plain numbers.
        unsafe {
            let Ok(owner) = GetWindow(self.0, GW_OWNER) else {
                return;
            };
            let owner = Hwnd(owner).screen_rect();
            let own = self.screen_rect();
            let x = owner.x + (owner.width - own.width) / 2;
            let y = owner.y + (owner.height - own.height) / 2;
            let _ = SetWindowPos(
                self.0,
                None,
                x.max(0),
                y.max(0),
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a template back: the words as 16-bit units.
    fn units(template: &DialogTemplate, page: bool) -> Vec<u16> {
        template
            .words(page)
            .iter()
            .flat_map(|word| [*word as u16, (*word >> 16) as u16])
            .collect()
    }

    fn text_at(units: &[u16], at: usize) -> (String, usize) {
        let length = units[at..].iter().position(|unit| *unit == 0).unwrap();
        (
            String::from_utf16_lossy(&units[at..at + length]),
            at + length + 1,
        )
    }

    fn sample() -> DialogTemplate {
        DialogTemplate::new("Edit line", 200, 100)
            .label(7, 9, 40, 8, "&Name:")
            .edit(100, 50, 7, 143, 14)
            .number(101, 50, 25, 40, 14)
            .up_down(102)
            .combo(103, 50, 43, 80, 100)
            .check_box(104, 7, 61, 100, 10, "&Enabled")
            .ok_cancel()
    }

    #[test]
    fn the_header_is_a_dlgtemplateex_with_the_message_font() {
        let template = sample();
        let units = units(&template, false);
        assert_eq!(units[0], 1, "version");
        assert_eq!(units[1], 0xFFFF, "the extended form");
        let style = u32::from(units[6]) | (u32::from(units[7]) << 16);
        assert_ne!(style & DS_SETFONT as u32, 0);
        assert_ne!(style & DS_MODALFRAME as u32, 0);
        assert_ne!(style & WS_CAPTION.0, 0);
        assert_eq!(style & WS_CHILD.0, 0);
        assert_eq!(units[8], 8, "the number of controls");
        assert_eq!(units[11..13], [200, 100], "the size");
        assert_eq!(units[13..15], [0, 0], "no menu, the dialog class");
        let (title, after) = text_at(&units, 15);
        assert_eq!(title, "Edit line");
        // The font is the system's, whatever it is.
        let (face, points) = message_face_and_points();
        assert_eq!(units[after], points);
        let (written, _) = text_at(&units, after + 3);
        assert_eq!(written, face);
        assert!(!face.is_empty() && points >= 6);
    }

    #[test]
    fn a_page_of_a_property_sheet_is_a_child_with_no_frame() {
        let units = units(&sample(), true);
        let style = u32::from(units[6]) | (u32::from(units[7]) << 16);
        assert_ne!(style & WS_CHILD.0, 0);
        assert_ne!(style & DS_CONTROL as u32, 0);
        assert_eq!(style & WS_CAPTION.0, 0);
    }

    #[test]
    fn every_control_starts_on_a_32_bit_boundary_with_its_id_and_class() {
        let template = sample();
        let units = units(&template, false);
        // Find each control by walking the template as the dialog manager
        // does.
        let (_, after_title) = text_at(&units, 15);
        let (_, mut at) = text_at(&units, after_title + 3);
        let mut seen = Vec::new();
        for _ in 0..template.items.len() {
            at += at % 2;
            let style = u32::from(units[at + 4]) | (u32::from(units[at + 5]) << 16);
            assert_ne!(style & WS_CHILD.0, 0);
            assert_ne!(style & WS_VISIBLE.0, 0);
            let id = units[at + 10];
            at += 12;
            let class = if units[at] == 0xFFFF {
                at += 2;
                format!("#{:04x}", units[at - 1])
            } else {
                let (name, next) = text_at(&units, at);
                at = next;
                name
            };
            let (text, next) = text_at(&units, at);
            at = next;
            assert_eq!(units[at], 0, "no creation data");
            at += 1;
            seen.push((id, class, text));
        }
        let expected = [
            (0xFFFF, "#0082", "&Name:"),
            (100, "#0081", ""),
            (101, "#0081", ""),
            (102, "msctls_updown32", ""),
            (103, "#0085", ""),
            (104, "#0080", "&Enabled"),
            (id::OK, "#0080", "OK"),
            (id::CANCEL, "#0080", "Cancel"),
        ];
        let seen: Vec<(u16, &str, &str)> = seen
            .iter()
            .map(|(id, class, text)| (*id, class.as_str(), text.as_str()))
            .collect();
        assert_eq!(seen, expected);
        assert_eq!(template.ids(), [0xFFFF, 100, 101, 102, 103, 104, 1, 2]);
        // Nothing is left over but padding.
        assert!(units[at..].iter().all(|unit| *unit == 0));
    }

    #[test]
    fn ok_and_cancel_are_at_the_bottom_right_with_ok_the_default() {
        let template = DialogTemplate::new("x", 200, 100).ok_cancel();
        let ok = &template.items[0];
        let cancel = &template.items[1];
        assert_eq!(cancel.rect, (143, 79, 50, 14));
        assert_eq!(ok.rect, (89, 79, 50, 14));
        assert_eq!(ok.style & 0xF, BS_DEFPUSHBUTTON as u32);
        assert_eq!(cancel.style & 0xF, BS_PUSHBUTTON as u32);
        let close = DialogTemplate::new("x", 200, 100).close_button();
        assert_eq!(close.items[0].id, id::CANCEL);
        assert_eq!(close.items[0].text, "Close");
        assert_eq!((close.width(), close.height()), (200, 100));
    }
}
