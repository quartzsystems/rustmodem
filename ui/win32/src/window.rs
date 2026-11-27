// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Windows, and the routing of their messages to Rust.
//!
//! A [`Window`] is a top-level window of one window class, registered
//! here. Its messages are turned into [`Event`]s and given to the
//! [`Handler`] held for that window; what the handler does not handle goes
//! to the default window procedure, so that everything a handler does not
//! care about behaves as it does in any other program. Dialogs made from
//! a [`crate::DialogTemplate`] get their events the same way, through the
//! dialog manager.
//!
//! A handler is called with `&self`. Win32 calls a window back while it is
//! already in a call: a handler that shows a dialog is still on the stack
//! when the timer of the window behind the dialog fires. A handler
//! therefore keeps its state in cells and must not hold a borrow across
//! anything that runs a message loop.
//!
//! The only state outside the windows themselves is what Win32 requires:
//! the window class, the registered wake message, and for each thread the
//! list of its windows that take dialog navigation.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_BTNFACE, HBRUSH, InvalidateRect, UpdateWindow};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDDS_SUBITEM, CDRF_DODEFAULT, CDRF_NEWFONT,
    CDRF_NOTIFYITEMDRAW, CDRF_NOTIFYSUBITEMDRAW, CLR_DEFAULT, DRAWITEMSTRUCT, LVIF_IMAGE,
    LVIF_STATE, LVIF_TEXT, LVIS_SELECTED, LVIS_STATEIMAGEMASK, LVN_COLUMNCLICK, LVN_GETDISPINFOW,
    LVN_GETINFOTIPW, LVN_ITEMCHANGED, LVN_KEYDOWN, NM_CUSTOMDRAW, NM_DBLCLK, NM_RETURN, NMHDR,
    NMITEMACTIVATE, NMLISTVIEW, NMLVCUSTOMDRAW, NMLVDISPINFOW, NMLVGETINFOTIPW, NMLVKEYDOWN,
    NMTBGETINFOTIPW, PROPSHEETPAGEW, PSN_APPLY, PSNRET_INVALID, PSNRET_NOERROR, TBN_GETINFOTIPW,
    TCN_SELCHANGE,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    EnableWindow, GetFocus, IsWindowEnabled, SetFocus,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CS_DBLCLKS, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow,
    EnumChildWindows, GWLP_USERDATA, GetClassNameW, GetClientRect, GetDlgCtrlID, GetDlgItem,
    GetParent, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, HICON, HMENU, IDC_ARROW, IMAGE_ICON, IsWindow, IsWindowVisible, KillTimer,
    LR_DEFAULTSIZE, LR_SHARED, LoadCursorW, LoadImageW, MF_POPUP, MINMAXINFO, PostMessageW,
    RegisterClassExW, RegisterWindowMessageW, SIZE_MINIMIZED, SW_HIDE, SW_SHOW, SWP_NOACTIVATE,
    SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetMenu, SetTimer, SetWindowLongPtrW,
    SetWindowPlacement, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_LONG_PTR_INDEX, WINDOW_STYLE, WINDOWPLACEMENT, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU,
    WM_CREATE, WM_DESTROY, WM_DPICHANGED, WM_DRAWITEM, WM_GETMINMAXINFO, WM_INITDIALOG,
    WM_MENUSELECT, WM_NCCREATE, WM_NCDESTROY, WM_NOTIFY, WM_SETFONT, WM_SETTINGCHANGE, WM_SIZE,
    WM_SYSCOLORCHANGE, WM_THEMECHANGED, WM_TIMER, WNDCLASSEXW, WS_CHILD, WS_OVERLAPPEDWINDOW,
    WS_VISIBLE,
};
use windows::core::{BOOL, PCWSTR, w};

use crate::canvas::Canvas;
use crate::font::Font;
use crate::menu::Menu;
use crate::util::{copy_to_buffer, from_wide, hiword, instance, loword, pcwstr, wide};

/// The name of the window class every [`Window`] is of.
const CLASS_NAME: PCWSTR = w!("QuartzSystems.rustmodem.Window");

/// The number of the icon group a program's resource file holds its icon
/// as; see `glyph::windows_res`.
const ICON_RESOURCE: usize = 1;

/// Where a dialog keeps its result for a notification.
const DWLP_MSGRESULT: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(0);

/// Where a dialog keeps a pointer of its owner's: here, its handler. The
/// offset is that of 64-bit Windows, the only one rustmodem is built for.
const DWLP_USER: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(16);

/// Identifiers the dialog manager gives meaning to.
pub mod id {
    /// The default button of a dialog; also what Enter sends.
    pub const OK: u16 = 1;
    /// The cancel button of a dialog; also what Escape and closing send.
    pub const CANCEL: u16 = 2;
    /// A task dialog's Yes button.
    pub const YES: u16 = 6;
    /// A task dialog's No button.
    pub const NO: u16 = 7;
    /// A task dialog's Close button.
    pub const CLOSE: u16 = 8;
}

/// The notification codes of [`super::Event::Command`].
pub mod code {
    /// A menu item was chosen.
    pub const MENU: u16 = 0;
    /// An accelerator was pressed.
    pub const ACCELERATOR: u16 = 1;
    /// A button, check box or radio button was clicked.
    pub const CLICKED: u16 = 0;
    /// The selection of a combo box changed.
    pub const COMBO_SELECTION: u16 = 1;
    /// The text of an editable combo box changed.
    pub const COMBO_EDIT: u16 = 5;
    /// The text of an edit control changed.
    pub const EDIT_CHANGE: u16 = 0x0300;
    /// The entry chosen in a list box changed.
    pub const LIST_SELECTED: u16 = 1;
    /// An entry of a list box was double-clicked.
    pub const LIST_DOUBLE_CLICK: u16 = 2;
}

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    /// The left edge.
    pub x: i32,
    /// The top edge.
    pub y: i32,
    /// The width.
    pub width: i32,
    /// The height.
    pub height: i32,
}

impl Rect {
    /// A rectangle from its top left corner and its size.
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// The rectangle with `margin` taken off every side. It does not go
    /// below nothing.
    pub fn inset(self, margin: i32) -> Rect {
        self.inset_by(margin, margin, margin, margin)
    }

    /// The rectangle with a margin taken off each side.
    pub fn inset_by(self, left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            x: self.x + left,
            y: self.y + top,
            width: (self.width - left - right).max(0),
            height: (self.height - top - bottom).max(0),
        }
    }

    pub(crate) fn from_rect(rect: RECT) -> Rect {
        Rect {
            x: rect.left,
            y: rect.top,
            width: rect.right - rect.left,
            height: rect.bottom - rect.top,
        }
    }
}

/// A colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Colour {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

impl Colour {
    /// A colour from its parts.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Colour {
        Colour { r, g, b }
    }

    pub(crate) fn from_colorref(value: u32) -> Colour {
        Colour {
            r: value as u8,
            g: (value >> 8) as u8,
            b: (value >> 16) as u8,
        }
    }

    pub(crate) fn colorref(self) -> COLORREF {
        COLORREF(u32::from(self.r) | (u32::from(self.g) << 8) | (u32::from(self.b) << 16))
    }
}

/// What a cell of a list view shows. The handler fills it in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellText {
    /// The text.
    pub text: String,
    /// The image at the start of the row, as an index into the list's
    /// image list. Only the first column has one.
    pub image: Option<usize>,
}

/// What the highlight in a menu is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuHighlight {
    /// A command, by its id.
    Item(u16),
    /// A submenu or a separator: nothing with an id.
    Other,
    /// The menu has closed.
    Closed,
}

/// Something that happened to a window.
#[derive(Debug)]
pub enum Event<'a> {
    /// The window has been created and has no children yet.
    Create,
    /// The dialog has been created, with its controls.
    InitDialog,
    /// The user asked to close the window. Not handled, the window is
    /// destroyed.
    Close,
    /// The window is being destroyed.
    Destroy,
    /// The client area has a new size.
    Size {
        /// The width of the client area in pixels.
        width: i32,
        /// The height of the client area in pixels.
        height: i32,
        /// The window was minimised.
        minimized: bool,
    },
    /// The window has moved to a monitor of another scaling. The window
    /// has already been moved to the rectangle Windows suggests; what is
    /// left is to make the fonts again and lay out the controls.
    DpiChanged {
        /// The new DPI.
        dpi: u32,
    },
    /// A menu item was chosen, an accelerator pressed, or a control has
    /// something to say. `code` is one of [`code`].
    Command {
        /// The id of the menu item or control.
        id: u16,
        /// What happened.
        code: u16,
    },
    /// A timer of the window fired.
    Timer {
        /// The id the timer was started with.
        id: usize,
    },
    /// A [`crate::UiSender`] for this window has something in its queue.
    Wake,
    /// The highlight in the window's menu moved.
    MenuSelect(MenuHighlight),
    /// The user asked for a context menu.
    ContextMenu {
        /// The control the request is for.
        source: Hwnd,
        /// Where, in screen coordinates, or `None` when the request came
        /// from the keyboard.
        position: Option<(i32, i32)>,
    },
    /// A list view asks what a cell shows.
    ListCell {
        /// The id of the list view.
        id: u16,
        /// The row.
        row: usize,
        /// The column.
        column: usize,
        /// What to fill in.
        cell: &'a mut CellText,
    },
    /// A list view asks what colour the text of a cell is. Left as it is,
    /// the colour is the control's own.
    ListCellColour {
        /// The id of the list view.
        id: u16,
        /// The row.
        row: usize,
        /// The column.
        column: usize,
        /// What to fill in.
        colour: &'a mut Option<Colour>,
    },
    /// A column header of a list view was clicked.
    ListColumnClick {
        /// The id of the list view.
        id: u16,
        /// The column.
        column: usize,
    },
    /// A row of a list view was double-clicked.
    ListActivate {
        /// The id of the list view.
        id: u16,
        /// The row.
        row: usize,
    },
    /// The selection of a list view changed.
    ListSelection {
        /// The id of the list view.
        id: u16,
    },
    /// A key was pressed in a list view.
    ListKey {
        /// The id of the list view.
        id: u16,
        /// The virtual key code.
        key: u16,
    },
    /// The box of a row of a [`crate::CheckList`] was checked or
    /// cleared, by the user or by the program.
    ListCheck {
        /// The id of the list.
        id: u16,
        /// The row.
        row: usize,
        /// Whether the box is checked now.
        checked: bool,
    },
    /// A list view asks for the tip of a row: what is shown while the
    /// pointer rests on it. Left empty, there is no tip.
    ListTip {
        /// The id of the list view.
        id: u16,
        /// The row.
        row: usize,
        /// What to fill in.
        text: &'a mut String,
    },
    /// A toolbar asks for the tip of a button.
    ToolTip {
        /// The command id of the button.
        id: u16,
        /// What to fill in.
        text: &'a mut String,
    },
    /// A control made with [`crate::DialogTemplate::canvas`] is to be
    /// painted. What the handler paints shows when it returns.
    Draw {
        /// The id of the control.
        id: u16,
        /// What to paint on.
        canvas: &'a mut Canvas,
    },
    /// The tab of a tab control changed.
    TabChanged {
        /// The id of the tab control.
        id: u16,
    },
    /// A page of a property sheet is asked to apply what it holds: OK or
    /// Apply was pressed. Answer [`Reply::Value`] of 1 to refuse, which
    /// keeps the sheet open on this page.
    Apply,
    /// The window asks for its smallest size, in pixels.
    MinSize(&'a mut (i32, i32)),
    /// A system setting, colour or theme changed.
    SettingsChanged,
}

/// What a handler says about an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Not handled: Windows does what it does by default.
    Default,
    /// Handled.
    Handled,
    /// Handled, with a result for the message.
    Value(isize),
}

/// Takes the events of a window or a dialog.
pub trait Handler {
    /// Something happened to `window`.
    fn event(&self, window: Hwnd, event: Event<'_>) -> Reply;
}

/// A window handle: a top-level window, a dialog or a control. Copying it
/// copies the handle, not the window. It is not `Send`: a window belongs
/// to the thread that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hwnd(pub(crate) HWND);

impl Hwnd {
    /// The handle as a number, for a [`crate::UiSender`] and for tests.
    pub fn raw(self) -> isize {
        self.0.0 as isize
    }

    /// The window with this handle number. Nothing is checked: a number
    /// that is not a window gives a handle on which every call fails
    /// harmlessly.
    pub fn from_raw(raw: isize) -> Hwnd {
        Hwnd(HWND(raw as *mut _))
    }

    /// Whether the window still exists.
    pub fn exists(self) -> bool {
        // SAFETY: the call takes any value and says whether it is a
        // window.
        unsafe { IsWindow(Some(self.0)) }.as_bool()
    }

    pub(crate) fn send(self, message: u32, wparam: usize, lparam: isize) -> isize {
        // SAFETY: sending a message is sound for any handle; what the
        // parameters must point at depends on the message, and each caller
        // in this crate passes what its message documents.
        unsafe { SendMessageW(self.0, message, Some(WPARAM(wparam)), Some(LPARAM(lparam))) }.0
    }

    /// The id of a control: the one it was made with.
    pub(crate) fn control_id(self) -> usize {
        // SAFETY: the call takes any handle and returns a number.
        unsafe { GetDlgCtrlID(self.0) }.max(0) as usize
    }

    /// The control of a dialog or window with this id. A handle on which
    /// every call fails harmlessly if there is none.
    pub fn item(self, id: u16) -> Hwnd {
        // SAFETY: the call takes any handle and id and returns a handle or
        // an error.
        Hwnd(unsafe { GetDlgItem(Some(self.0), i32::from(id)) }.unwrap_or_default())
    }

    /// The window's parent or owner.
    pub fn parent(self) -> Option<Hwnd> {
        // SAFETY: the call takes any handle.
        unsafe { GetParent(self.0) }.ok().map(Hwnd)
    }

    /// The window's text: a title, a label, or what an edit control holds.
    pub fn text(self) -> String {
        // SAFETY: the call takes any handle and returns a length.
        let length = unsafe { GetWindowTextLengthW(self.0) }.max(0) as usize;
        let mut buffer = vec![0u16; length + 1];
        // SAFETY: the buffer is as long as the slice says.
        let copied = unsafe { GetWindowTextW(self.0, &mut buffer) }.max(0) as usize;
        from_wide(&buffer[..copied.min(length)])
    }

    /// Sets the window's text.
    pub fn set_text(self, text: &str) {
        let text = wide(text);
        // SAFETY: `text` is terminated UTF-16 that outlives the call.
        unsafe {
            let _ = SetWindowTextW(self.0, pcwstr(&text));
        }
    }

    /// Sets the window's text if it is not that already, so that a label
    /// that is set every second does not flicker.
    pub fn set_text_if_changed(self, text: &str) {
        if self.text() != text {
            self.set_text(text);
        }
    }

    /// Enables or disables the window.
    pub fn set_enabled(self, enabled: bool) {
        // SAFETY: the call takes any handle.
        unsafe {
            let _ = EnableWindow(self.0, enabled);
        }
    }

    /// Whether the window is enabled.
    pub fn enabled(self) -> bool {
        // SAFETY: the call takes any handle.
        unsafe { IsWindowEnabled(self.0) }.as_bool()
    }

    /// Shows or hides the window.
    pub fn set_visible(self, visible: bool) {
        // SAFETY: the call takes any handle.
        unsafe {
            let _ = ShowWindow(self.0, if visible { SW_SHOW } else { SW_HIDE });
        }
    }

    /// Whether the window is shown.
    pub fn visible(self) -> bool {
        // SAFETY: the call takes any handle.
        unsafe { IsWindowVisible(self.0) }.as_bool()
    }

    /// Gives the window the keyboard focus.
    pub fn focus(self) {
        // SAFETY: the call takes any handle.
        unsafe {
            let _ = SetFocus(Some(self.0));
        }
    }

    /// Brings the window to the front.
    pub fn bring_to_front(self) {
        // SAFETY: the call takes any handle.
        unsafe {
            let _ = SetForegroundWindow(self.0);
        }
    }

    /// Moves and sizes the window. For a control the rectangle is in its
    /// parent's client area; for a top-level window it is on the screen.
    pub fn set_rect(self, rect: Rect) {
        // SAFETY: the call takes any handle and plain numbers.
        unsafe {
            let _ = SetWindowPos(
                self.0,
                None,
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// The client area: its size, with the origin at zero.
    pub fn client_rect(self) -> Rect {
        let mut rect = RECT::default();
        // SAFETY: `rect` is valid for the call to fill.
        unsafe {
            let _ = GetClientRect(self.0, &mut rect);
        }
        Rect::from_rect(rect)
    }

    /// The window's rectangle on the screen.
    pub fn screen_rect(self) -> Rect {
        let mut rect = RECT::default();
        // SAFETY: `rect` is valid for the call to fill.
        unsafe {
            let _ = GetWindowRect(self.0, &mut rect);
        }
        Rect::from_rect(rect)
    }

    /// The DPI of the monitor the window is on.
    pub fn dpi(self) -> u32 {
        // SAFETY: the call takes any handle; for one that is not a window
        // it returns 0.
        match unsafe { GetDpiForWindow(self.0) } {
            0 => crate::metrics::BASE_DPI,
            dpi => dpi,
        }
    }

    /// Gives the window a font. The font must outlive the window's use of
    /// it: set another before dropping this one.
    pub fn set_font(self, font: &Font) {
        self.send(WM_SETFONT, font.handle().0 as usize, 1);
    }

    /// Gives the window and every control inside it a font.
    pub fn set_font_all(self, font: &Font) {
        unsafe extern "system" fn each(child: HWND, font: LPARAM) -> BOOL {
            // SAFETY: called by EnumChildWindows with a child window; the
            // parameter is the font handle passed below.
            unsafe {
                SendMessageW(
                    child,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                )
            };
            true.into()
        }
        self.set_font(font);
        // SAFETY: `each` matches the callback the call wants, and the
        // parameter is a plain number.
        unsafe {
            let _ = EnumChildWindows(Some(self.0), Some(each), LPARAM(font.handle().0 as isize));
        }
    }

    /// Has the window drawn again.
    pub fn redraw(self) {
        // SAFETY: the calls take any handle.
        unsafe {
            let _ = InvalidateRect(Some(self.0), None, true);
            let _ = UpdateWindow(self.0);
        }
    }

    /// Starts a timer that fires every `milliseconds`, or changes the one
    /// with this id.
    pub fn set_timer(self, id: usize, milliseconds: u32) {
        // SAFETY: the call takes any handle; with no callback the timer
        // posts WM_TIMER to the window.
        unsafe {
            SetTimer(Some(self.0), id, milliseconds, None);
        }
    }

    /// Stops a timer.
    pub fn kill_timer(self, id: usize) {
        // SAFETY: the call takes any handle and id.
        unsafe {
            let _ = KillTimer(Some(self.0), id);
        }
    }

    /// Sends the window a command, as choosing a menu item or pressing a
    /// button with that id does. It arrives after this returns.
    pub fn command(self, id: u16) {
        // SAFETY: posting a message is sound for any handle; this one
        // carries no pointers.
        unsafe {
            let _ = PostMessageW(Some(self.0), WM_COMMAND, WPARAM(usize::from(id)), LPARAM(0));
        }
    }

    /// Sends the window a command with a code, as a control with that
    /// id does when something happens in it: one of [`code`]. It arrives
    /// after this returns.
    pub fn command_code(self, id: u16, code: u16) {
        let wparam = (usize::from(code) << 16) | usize::from(id);
        // SAFETY: posting a message is sound for any handle; this one
        // carries no pointers.
        unsafe {
            let _ = PostMessageW(Some(self.0), WM_COMMAND, WPARAM(wparam), LPARAM(0));
        }
    }

    /// Asks the window to close, as the close button does.
    pub fn close(self) {
        // SAFETY: posting a message is sound for any handle.
        unsafe {
            let _ = PostMessageW(Some(self.0), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// Destroys the window.
    pub fn destroy(self) {
        // SAFETY: the call takes any handle; destroying a window of
        // another thread fails harmlessly.
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }

    /// Sets or replaces the window's menu bar. The window owns the menu
    /// from now on.
    pub fn set_menu(self, menu: Menu) {
        // SAFETY: the menu handle is valid, and ownership passes to the
        // window, which destroys the menu with itself.
        unsafe {
            let _ = SetMenu(self.0, Some(menu.into_handle()));
        }
    }

    /// The window's placement: where it is when restored, and whether it
    /// is maximised. As bytes, to be saved and given back.
    pub fn placement(self) -> Vec<u8> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..WINDOWPLACEMENT::default()
        };
        // SAFETY: `placement` has its length filled in and is valid for
        // the call to fill.
        if unsafe { GetWindowPlacement(self.0, &mut placement) }.is_err() {
            return Vec::new();
        }
        // SAFETY: WINDOWPLACEMENT is plain numbers with no padding that
        // matters; its bytes are read, not interpreted.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&raw const placement).cast::<u8>(),
                size_of::<WINDOWPLACEMENT>(),
            )
        };
        bytes.to_vec()
    }

    /// Puts the window where a saved placement says, if the bytes are a
    /// placement. Windows moves a placement that is off every monitor
    /// back onto one. Returns whether the placement was used.
    pub fn set_placement(self, bytes: &[u8]) -> bool {
        if bytes.len() != size_of::<WINDOWPLACEMENT>() {
            return false;
        }
        let mut placement = WINDOWPLACEMENT::default();
        // SAFETY: the source is exactly as long as the structure, which
        // is plain numbers: any bytes are a valid value of it.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                (&raw mut placement).cast::<u8>(),
                size_of::<WINDOWPLACEMENT>(),
            );
        }
        if placement.length != size_of::<WINDOWPLACEMENT>() as u32 {
            return false;
        }
        // SAFETY: `placement` is a whole WINDOWPLACEMENT with its length
        // checked.
        unsafe { SetWindowPlacement(self.0, &placement) }.is_ok()
    }

    /// Puts the program's icon, at the size of a large icon, in an icon
    /// control: one made with [`crate::DialogTemplate::icon`]. A program
    /// with no icon in its resources shows none.
    pub fn show_program_icon(self) {
        /// STM_SETICON: gives a static control of the icon style its icon.
        const STM_SETICON: u32 = 0x0170;
        if let Some(icon) = program_icon(false) {
            // The icon is shared with the window class and is not the
            // control's to destroy.
            self.send(STM_SETICON, icon.0 as usize, 0);
        }
    }

    /// The name of the window's class.
    pub fn class_name(self) -> String {
        let mut buffer = [0u16; 128];
        // SAFETY: the buffer is as long as the slice says.
        let length = unsafe { GetClassNameW(self.0, &mut buffer) }.max(0) as usize;
        from_wide(&buffer[..length.min(buffer.len())])
    }
}

/// The window or control of this thread that has the keyboard focus.
pub fn focused() -> Option<Hwnd> {
    // SAFETY: the call takes nothing and returns a handle or null.
    let window = unsafe { GetFocus() };
    (!window.is_invalid()).then_some(Hwnd(window))
}

/// How a top-level window is made.
pub struct WindowOptions<'a> {
    /// The title.
    pub title: &'a str,
    /// The size of the window in pixels, or `None` for the size Windows
    /// picks.
    pub size: Option<(i32, i32)>,
    /// The menu bar.
    pub menu: Option<Menu>,
    /// The window this one belongs to: it stays above it and is closed
    /// with it.
    pub owner: Option<Hwnd>,
    /// Whether the window takes part in dialog navigation: Tab, the arrow
    /// keys, mnemonics, Enter and Escape.
    pub dialog_keys: bool,
}

/// A top-level window.
pub struct Window;

/// The message a [`crate::UiSender`] posts to wake its window.
pub(crate) fn wake_message() -> u32 {
    static MESSAGE: OnceLock<u32> = OnceLock::new();
    // SAFETY: the name is a terminated wide literal. Registering the same
    // name twice gives the same number.
    *MESSAGE.get_or_init(|| unsafe { RegisterWindowMessageW(w!("QuartzSystems.rustmodem.Wake")) })
}

thread_local! {
    /// The windows of this thread that take dialog navigation, for the
    /// message loop.
    static DIALOG_WINDOWS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
}

/// Adds a window to the ones the message loop offers keys to for dialog
/// navigation.
pub(crate) fn add_dialog_window(window: HWND) {
    DIALOG_WINDOWS.with(|windows| windows.borrow_mut().push(window));
}

fn remove_dialog_window(window: HWND) {
    DIALOG_WINDOWS.with(|windows| windows.borrow_mut().retain(|w| *w != window));
}

/// The windows of this thread that take dialog navigation.
pub(crate) fn dialog_windows() -> Vec<HWND> {
    DIALOG_WINDOWS.with(|windows| windows.borrow().clone())
}

/// The program's icon at the size Windows wants for `small` or large
/// icons, from the resource file, if the program has one.
fn program_icon(small: bool) -> Option<HICON> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXICON, SM_CXSMICON};
    // SAFETY: the calls take an index and return a number.
    let size = unsafe { GetSystemMetrics(if small { SM_CXSMICON } else { SM_CXICON }) };
    // SAFETY: the name is an integer resource id, which this form of
    // pointer is how Win32 spells; the handle is shared and not to be
    // destroyed.
    let icon = unsafe {
        LoadImageW(
            Some(instance()),
            PCWSTR(ICON_RESOURCE as *const u16),
            IMAGE_ICON,
            size,
            size,
            LR_SHARED
                | if size == 0 {
                    LR_DEFAULTSIZE
                } else {
                    Default::default()
                },
        )
    };
    icon.ok().map(|handle| HICON(handle.0))
}

/// Registers the window class, once.
fn register_class() {
    static REGISTERED: OnceLock<u16> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            // Double clicks are wanted by the controls inside, not by the
            // window, but a class without the style swallows them.
            style: CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: instance(),
            hIcon: program_icon(false).unwrap_or_default(),
            hIconSm: program_icon(true).unwrap_or_default(),
            // SAFETY: loading a stock cursor takes no instance.
            hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default(),
            // The face colour, as a dialog has: the window is a surface
            // for controls. A system colour index plus one is how a class
            // names a system colour as its background.
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as usize as *mut _),
            lpszClassName: CLASS_NAME,
            ..WNDCLASSEXW::default()
        };
        // SAFETY: `class` is a whole WNDCLASSEXW whose strings are
        // literals and whose procedure lives for the whole program.
        unsafe { RegisterClassExW(&class) }
    });
}

impl Window {
    /// Makes a top-level window, not yet shown. Its events go to
    /// `handler`, starting with [`Event::Create`] before this returns.
    pub fn create(options: WindowOptions<'_>, handler: Rc<dyn Handler>) -> Option<Hwnd> {
        register_class();
        let title = wide(options.title);
        let (width, height) = options.size.unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));
        // The window procedure takes this over at WM_NCCREATE and frees it
        // at WM_NCDESTROY.
        let state = Box::into_raw(Box::new(handler));
        // SAFETY: the class is registered, the strings are terminated and
        // outlive the call, and `state` is a valid pointer that the window
        // procedure takes ownership of.
        let created = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                pcwstr(&title),
                // Not WS_CLIPCHILDREN: a group box does not paint its own
                // inside and counts on its parent having painted there,
                // as the parent of a dialog's group boxes does.
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width,
                height,
                options.owner.map(|owner| owner.0),
                options.menu.map(Menu::into_handle),
                Some(instance()),
                Some(state.cast()),
            )
        };
        match created {
            Ok(window) => {
                if options.dialog_keys {
                    add_dialog_window(window);
                }
                Some(Hwnd(window))
            }
            Err(_) => {
                // If the window got as far as WM_NCCREATE it has freed the
                // state at WM_NCDESTROY; if it did not, nothing has, and
                // which it was cannot be told from here. Leaking one
                // pointer when a window cannot be made is the safe side.
                None
            }
        }
    }

    /// Makes a control: a child window of a system class.
    pub(crate) fn control(
        parent: Hwnd,
        class: PCWSTR,
        text: &str,
        style: u32,
        ex_style: u32,
        id: u16,
    ) -> Hwnd {
        let text = wide(text);
        // SAFETY: the class is a terminated wide string of a class the
        // system or the common controls registered, the text is terminated
        // and outlives the call, and for a child window the menu parameter
        // is its id.
        let created = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(ex_style),
                class,
                pcwstr(&text),
                WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
                0,
                0,
                0,
                0,
                Some(parent.0),
                Some(HMENU(usize::from(id) as *mut _)),
                Some(instance()),
                None,
            )
        };
        Hwnd(created.unwrap_or_default())
    }
}

/// The handler of a window, from where the window keeps it.
///
/// # Safety
///
/// `index` must be where this crate put a `Box<Rc<dyn Handler>>` for the
/// window, or hold zero.
unsafe fn handler_of(window: HWND, index: WINDOW_LONG_PTR_INDEX) -> Option<Rc<dyn Handler>> {
    // SAFETY: reading a window's own data is sound for any handle.
    let pointer = unsafe { GetWindowLongPtrW(window, index) } as *const Rc<dyn Handler>;
    if pointer.is_null() {
        return None;
    }
    // SAFETY: by the contract of this function the pointer is a live
    // `Rc<dyn Handler>`. It is cloned, so that the handler outlives the
    // call even if the window is destroyed during it.
    Some(unsafe { (*pointer).clone() })
}

/// Frees the handler a window keeps.
///
/// # Safety
///
/// As [`handler_of`], and the window must not use the handler again.
unsafe fn free_handler(window: HWND, index: WINDOW_LONG_PTR_INDEX) {
    // SAFETY: as in `handler_of`; the slot is cleared first, so that
    // nothing can read the pointer after it is freed.
    unsafe {
        let pointer = SetWindowLongPtrW(window, index, 0) as *mut Rc<dyn Handler>;
        if !pointer.is_null() {
            drop(Box::from_raw(pointer));
        }
    }
}

/// The window procedure of [`Window`].
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: for this message `lparam` points at the CREATESTRUCTW of
        // the CreateWindowExW call, whose parameter is the boxed handler.
        unsafe {
            let create = lparam.0 as *const CREATESTRUCTW;
            SetWindowLongPtrW(window, GWLP_USERDATA, (*create).lpCreateParams as isize);
        }
    }
    if message == WM_NCDESTROY {
        remove_dialog_window(window);
        // SAFETY: this is the last message a window gets.
        unsafe { free_handler(window, GWLP_USERDATA) };
    }
    // SAFETY: GWLP_USERDATA is where WM_NCCREATE put the handler.
    let handled = match unsafe { handler_of(window, GWLP_USERDATA) } {
        Some(handler) => dispatch(&*handler, window, message, wparam, lparam, false),
        None => None,
    };
    match handled {
        Some(result) => LRESULT(result),
        // SAFETY: the parameters are the ones this procedure was given.
        None => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// The dialog procedure of the dialogs this crate shows. The parameter a
/// dialog is created with is its boxed handler.
pub(crate) unsafe extern "system" fn dialog_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    // SAFETY: the parameters are the ones the dialog manager gave.
    unsafe { dialog_message(window, message, wparam, lparam, false) }
}

/// The dialog procedure of the pages of a property sheet. The parameter a
/// page is created with is its `PROPSHEETPAGEW`, whose own parameter is
/// the handler, which the sheet owns.
pub(crate) unsafe extern "system" fn page_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    // SAFETY: the parameters are the ones the dialog manager gave.
    unsafe { dialog_message(window, message, wparam, lparam, true) }
}

/// One message of a dialog or a page.
///
/// # Safety
///
/// The parameters must be those of a call of a dialog procedure, for a
/// dialog created with [`dialog_proc`] or a page created with
/// [`page_proc`], as `page` says.
unsafe fn dialog_message(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    page: bool,
) -> isize {
    if message == WM_INITDIALOG {
        let state = if page {
            // SAFETY: for a page the parameter of this message points at
            // the PROPSHEETPAGEW the page was described with.
            unsafe { (*(lparam.0 as *const PROPSHEETPAGEW)).lParam.0 }
        } else {
            lparam.0
        };
        // SAFETY: storing a number in the dialog's own data.
        unsafe { SetWindowLongPtrW(window, DWLP_USER, state) };
    }
    if message == WM_NCDESTROY {
        remove_dialog_window(window);
        if page {
            // The sheet owns the handler of a page, because a page that
            // is never looked at is never created or destroyed. The page
            // only forgets it.
            // SAFETY: storing a number in the dialog's own data.
            unsafe { SetWindowLongPtrW(window, DWLP_USER, 0) };
        } else {
            // SAFETY: this is the last message a dialog gets.
            unsafe { free_handler(window, DWLP_USER) };
        }
        return 0;
    }
    // SAFETY: DWLP_USER is where WM_INITDIALOG put the handler.
    let Some(handler) = (unsafe { handler_of(window, DWLP_USER) }) else {
        return 0;
    };
    let result = dispatch(&*handler, window, message, wparam, lparam, true);
    if message == WM_INITDIALOG {
        if !page {
            Hwnd(window).centre_on_owner();
        }
        // The result says whether the dialog manager sets the focus: it
        // does unless the handler set it itself.
        return result.unwrap_or(1);
    }
    match result {
        // A dialog procedure says whether it handled the message; the
        // result of the message itself goes in the dialog's data.
        Some(result) => {
            // SAFETY: storing a number in the dialog's own data.
            unsafe { SetWindowLongPtrW(window, DWLP_MSGRESULT, result) };
            1
        }
        None => {
            // OK and Cancel that the handler left alone end the dialog.
            // In a page they belong to the sheet.
            let button = loword(wparam.0);
            let pressed = message == WM_COMMAND && hiword(wparam.0) == code::CLICKED;
            if pressed && !page && (button == id::OK || button == id::CANCEL) {
                Hwnd(window).end_dialog(button);
                return 1;
            }
            0
        }
    }
}

/// Whether a window is one the message loop offers keys to for dialog
/// navigation: the main window or a modeless dialog, and not a modal one.
pub(crate) fn is_dialog_window(window: HWND) -> bool {
    DIALOG_WINDOWS.with(|windows| windows.borrow().contains(&window))
}

/// Turns a message into an event, gives it to the handler, and turns the
/// reply into the message's result. `None` means the default handling.
fn dispatch(
    handler: &dyn Handler,
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    dialog: bool,
) -> Option<isize> {
    let hwnd = Hwnd(window);
    let result = |reply: Reply, handled: isize| match reply {
        Reply::Default => None,
        Reply::Handled => Some(handled),
        Reply::Value(value) => Some(value),
    };
    match message {
        WM_CREATE => result(handler.event(hwnd, Event::Create), 0),
        WM_INITDIALOG => {
            // Handled means the handler set the focus itself.
            match handler.event(hwnd, Event::InitDialog) {
                Reply::Handled => Some(0),
                _ => Some(1),
            }
        }
        WM_CLOSE => result(handler.event(hwnd, Event::Close), 0),
        WM_DESTROY => result(handler.event(hwnd, Event::Destroy), 0),
        WM_SIZE => {
            let event = Event::Size {
                width: i32::from(loword(lparam.0 as usize)),
                height: i32::from(hiword(lparam.0 as usize)),
                minimized: wparam.0 as u32 == SIZE_MINIMIZED,
            };
            result(handler.event(hwnd, event), 0)
        }
        WM_DPICHANGED if !dialog => {
            // SAFETY: for this message `lparam` points at the rectangle
            // Windows suggests for the window at its new DPI.
            let suggested = unsafe { *(lparam.0 as *const RECT) };
            hwnd.set_rect(Rect::from_rect(suggested));
            let dpi = u32::from(loword(wparam.0));
            result(handler.event(hwnd, Event::DpiChanged { dpi }), 0)
        }
        WM_COMMAND => {
            let event = Event::Command {
                id: loword(wparam.0),
                code: hiword(wparam.0),
            };
            result(handler.event(hwnd, event), 0)
        }
        WM_TIMER => result(handler.event(hwnd, Event::Timer { id: wparam.0 }), 0),
        WM_DRAWITEM => {
            // SAFETY: for this message `lparam` points at the
            // DRAWITEMSTRUCT of the control to be painted, valid until
            // the message is answered.
            let item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
            let mut canvas = Canvas::of(item)?;
            let event = Event::Draw {
                id: item.CtlID as u16,
                canvas: &mut canvas,
            };
            // The canvas is copied to the screen as it goes.
            result(handler.event(hwnd, event), 1)
        }
        WM_MENUSELECT => {
            let flags = u32::from(hiword(wparam.0));
            let highlight = if flags == 0xFFFF && lparam.0 == 0 {
                MenuHighlight::Closed
            } else if flags & MF_POPUP.0 != 0 {
                MenuHighlight::Other
            } else {
                match loword(wparam.0) {
                    0 => MenuHighlight::Other,
                    id => MenuHighlight::Item(id),
                }
            };
            result(handler.event(hwnd, Event::MenuSelect(highlight)), 0)
        }
        WM_CONTEXTMENU => {
            let (x, y) = (
                loword(lparam.0 as usize) as i16,
                hiword(lparam.0 as usize) as i16,
            );
            let event = Event::ContextMenu {
                source: Hwnd(HWND(wparam.0 as *mut _)),
                // Minus one for both means the keyboard asked.
                position: (x != -1 || y != -1).then_some((i32::from(x), i32::from(y))),
            };
            result(handler.event(hwnd, event), 0)
        }
        WM_GETMINMAXINFO if !dialog => {
            let mut minimum = (0, 0);
            let reply = handler.event(hwnd, Event::MinSize(&mut minimum));
            if reply == Reply::Default {
                return None;
            }
            // SAFETY: for this message `lparam` points at a MINMAXINFO
            // for the handler to change.
            unsafe {
                let info = lparam.0 as *mut MINMAXINFO;
                (*info).ptMinTrackSize = POINT {
                    x: minimum.0,
                    y: minimum.1,
                };
            }
            Some(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE => {
            handler.event(hwnd, Event::SettingsChanged);
            // Always passed on: the controls want to hear of it too.
            None
        }
        WM_NOTIFY => notify(handler, hwnd, lparam),
        other if other == wake_message() => result(handler.event(hwnd, Event::Wake), 0),
        _ => None,
    }
}

/// Turns a notification from a control into an event.
fn notify(handler: &dyn Handler, hwnd: Hwnd, lparam: LPARAM) -> Option<isize> {
    // SAFETY: for WM_NOTIFY `lparam` points at a structure that starts
    // with an NMHDR, valid for the length of the message. Each arm below
    // reads it as the larger structure its code documents.
    let header = unsafe { &*(lparam.0 as *const NMHDR) };
    let id = header.idFrom as u16;
    let reply = |reply: Reply, handled: isize| match reply {
        Reply::Default => None,
        Reply::Handled => Some(handled),
        Reply::Value(value) => Some(value),
    };
    match header.code {
        LVN_GETDISPINFOW => {
            // SAFETY: the code says the structure is an NMLVDISPINFOW,
            // which the list view lets the receiver write to.
            let info = unsafe { &mut *(lparam.0 as *mut NMLVDISPINFOW) };
            let mut cell = CellText::default();
            handler.event(
                hwnd,
                Event::ListCell {
                    id,
                    row: info.item.iItem.max(0) as usize,
                    column: info.item.iSubItem.max(0) as usize,
                    cell: &mut cell,
                },
            );
            if info.item.mask.contains(LVIF_TEXT) {
                // SAFETY: the list view gives a buffer of `cchTextMax`
                // characters for the text.
                unsafe {
                    copy_to_buffer(
                        &cell.text,
                        info.item.pszText.0,
                        info.item.cchTextMax.max(0) as usize,
                    );
                }
            }
            if info.item.mask.contains(LVIF_IMAGE) {
                // Minus two is no image at all.
                info.item.iImage = cell.image.map_or(-2, |image| image as i32);
            }
            Some(0)
        }
        NM_CUSTOMDRAW if Hwnd(header.hwndFrom).class_name() == "SysListView32" => {
            // SAFETY: from a list view the structure is an NMLVCUSTOMDRAW,
            // which the receiver may write the colours of.
            let draw = unsafe { &mut *(lparam.0 as *mut NMLVCUSTOMDRAW) };
            let stage = draw.nmcd.dwDrawStage;
            if stage == CDDS_PREPAINT {
                Some(CDRF_NOTIFYITEMDRAW as isize)
            } else if stage == CDDS_ITEMPREPAINT {
                Some(CDRF_NOTIFYSUBITEMDRAW as isize)
            } else if stage.0 == CDDS_ITEMPREPAINT.0 | CDDS_SUBITEM.0 {
                let mut colour = None;
                handler.event(
                    hwnd,
                    Event::ListCellColour {
                        id,
                        row: draw.nmcd.dwItemSpec,
                        column: draw.iSubItem.max(0) as usize,
                        colour: &mut colour,
                    },
                );
                // Set for every cell: the structure is carried from one
                // cell of the row to the next.
                draw.clrText = colour.map_or(COLORREF(CLR_DEFAULT as u32), Colour::colorref);
                Some(CDRF_NEWFONT as isize)
            } else {
                Some(CDRF_DODEFAULT as isize)
            }
        }
        LVN_COLUMNCLICK => {
            // SAFETY: the code says the structure is an NMLISTVIEW.
            let info = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
            let event = Event::ListColumnClick {
                id,
                column: info.iSubItem.max(0) as usize,
            };
            reply(handler.event(hwnd, event), 0)
        }
        NM_DBLCLK if Hwnd(header.hwndFrom).class_name() == "SysListView32" => {
            // SAFETY: from a list view the structure is an NMITEMACTIVATE.
            let info = unsafe { &*(lparam.0 as *const NMITEMACTIVATE) };
            if info.iItem < 0 {
                return None;
            }
            let event = Event::ListActivate {
                id,
                row: info.iItem as usize,
            };
            reply(handler.event(hwnd, event), 0)
        }
        NM_RETURN if Hwnd(header.hwndFrom).class_name() == "SysListView32" => {
            let row = crate::listview::ListView(Hwnd(header.hwndFrom)).selected()?;
            reply(handler.event(hwnd, Event::ListActivate { id, row }), 0)
        }
        LVN_ITEMCHANGED => {
            // SAFETY: the code says the structure is an NMLISTVIEW.
            let info = unsafe { &*(lparam.0 as *const NMLISTVIEW) };
            if !info.uChanged.contains(LVIF_STATE) {
                return None;
            }
            // The state image is the check box of a list that has them:
            // none while the row is being made, then the empty box or the
            // checked one.
            let (old_box, new_box) = (
                (info.uOldState & LVIS_STATEIMAGEMASK.0) >> 12,
                (info.uNewState & LVIS_STATEIMAGEMASK.0) >> 12,
            );
            if old_box != 0 && new_box != 0 && old_box != new_box && info.iItem >= 0 {
                let event = Event::ListCheck {
                    id,
                    row: info.iItem as usize,
                    checked: new_box == 2,
                };
                return reply(handler.event(hwnd, event), 0);
            }
            let selection = (info.uNewState ^ info.uOldState) & LVIS_SELECTED.0 != 0;
            if !selection {
                return None;
            }
            reply(handler.event(hwnd, Event::ListSelection { id }), 0)
        }
        LVN_GETINFOTIPW => {
            // SAFETY: the code says the structure is an NMLVGETINFOTIPW,
            // with a buffer for the receiver to write the tip to.
            let info = unsafe { &mut *(lparam.0 as *mut NMLVGETINFOTIPW) };
            let mut text = String::new();
            handler.event(
                hwnd,
                Event::ListTip {
                    id,
                    row: info.iItem.max(0) as usize,
                    text: &mut text,
                },
            );
            // SAFETY: the list view gives a buffer of `cchTextMax`
            // characters for the tip.
            unsafe { copy_to_buffer(&text, info.pszText.0, info.cchTextMax.max(0) as usize) };
            Some(0)
        }
        LVN_KEYDOWN => {
            // SAFETY: the code says the structure is an NMLVKEYDOWN.
            let info = unsafe { &*(lparam.0 as *const NMLVKEYDOWN) };
            reply(
                handler.event(
                    hwnd,
                    Event::ListKey {
                        id,
                        key: info.wVKey,
                    },
                ),
                0,
            )
        }
        TBN_GETINFOTIPW => {
            // SAFETY: the code says the structure is an NMTBGETINFOTIPW,
            // with a buffer for the receiver to write the tip to.
            let info = unsafe { &mut *(lparam.0 as *mut NMTBGETINFOTIPW) };
            let mut text = String::new();
            handler.event(
                hwnd,
                Event::ToolTip {
                    id: info.iItem as u16,
                    text: &mut text,
                },
            );
            // SAFETY: the toolbar gives a buffer of `cchTextMax`
            // characters for the tip.
            unsafe { copy_to_buffer(&text, info.pszText.0, info.cchTextMax.max(0) as usize) };
            Some(0)
        }
        TCN_SELCHANGE => reply(handler.event(hwnd, Event::TabChanged { id }), 0),
        PSN_APPLY => match handler.event(hwnd, Event::Apply) {
            Reply::Value(value) if value != 0 => Some(PSNRET_INVALID as isize),
            _ => Some(PSNRET_NOERROR as isize),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rectangle_is_inset_and_never_goes_below_nothing() {
        let rect = Rect::new(10, 20, 100, 50);
        assert_eq!(rect.inset(5), Rect::new(15, 25, 90, 40));
        assert_eq!(rect.inset_by(1, 2, 3, 4), Rect::new(11, 22, 96, 44));
        assert_eq!(rect.inset(60), Rect::new(70, 80, 0, 0));
        let win = RECT {
            left: 3,
            top: 4,
            right: 13,
            bottom: 24,
        };
        assert_eq!(Rect::from_rect(win), Rect::new(3, 4, 10, 20));
    }

    #[test]
    fn a_colour_is_what_windows_calls_a_colorref() {
        let colour = Colour::rgb(0x12, 0x34, 0x56);
        assert_eq!(colour.colorref().0, 0x0056_3412);
        assert_eq!(Colour::from_colorref(0x0056_3412), colour);
    }

    #[test]
    fn a_handle_that_is_not_a_window_fails_harmlessly() {
        let nothing = Hwnd::from_raw(0);
        assert!(!nothing.exists());
        assert_eq!(nothing.text(), "");
        nothing.set_text("x");
        nothing.set_enabled(false);
        assert_eq!(nothing.client_rect(), Rect::default());
        assert_eq!(nothing.dpi(), 96);
        assert!(nothing.placement().is_empty());
        assert!(!nothing.set_placement(&[0; 4]));
        assert!(!nothing.item(1).exists());
        assert!(nothing.parent().is_none());
        assert_eq!(nothing.raw(), 0);
    }
}
