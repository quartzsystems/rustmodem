// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Safe wrappers over the Win32 controls the rustmodem manager uses.
//!
//! This is not a GUI framework. It is the stock Windows controls, each
//! behind a small typed wrapper, and exactly as much of each as the
//! manager needs: nothing here draws a widget of its own, picks a colour
//! or names a font. A program built on it looks and behaves like any
//! other program built on the common controls, because it is one.
//!
//! - [`App`] initialises the common controls and runs the message loop,
//!   with accelerators and dialog navigation.
//! - [`Window`] is a top-level window whose messages go, as [`Event`]s, to
//!   a [`Handler`] held for that window.
//! - The controls are in [`controls`], [`ListView`], [`Toolbar`],
//!   [`StatusBar`], [`RichEdit`] and [`Menu`].
//! - [`DialogTemplate`] builds dialogs in memory, in dialog units, for the
//!   dialog manager to run; [`property_sheet`], [`task_dialog`] and the
//!   file dialogs are the system's own.
//! - [`Font`] and the functions of [`metrics`] take every font, size and
//!   colour from the system, for the DPI of the window.
//! - [`layout`] places controls in rows and columns.
//! - [`UiSender`] wakes the UI thread from another thread.
//! - [`capture`] renders a window to pixels, for screenshot tests.
//!
//! Everything that touches a window happens on the thread that made it:
//! the wrappers hold window handles and are not `Send`. This is one of the
//! crates that may use `unsafe`; every block is a Win32 call, or the
//! reading of a structure Windows passed in, and says why it is sound.
//!
//! A window handle cannot be sent to another thread, and so neither can
//! anything that holds one:
//!
//! ```compile_fail
//! fn assert_send<T: Send>() {}
//! assert_send::<win32::Hwnd>();
//! ```
//!
//! ```compile_fail
//! fn assert_send<T: Send>() {}
//! assert_send::<win32::ListView>();
//! ```
//!
//! What crosses threads is a [`UiSender`], which is made for it:
//!
//! ```
//! fn assert_send<T: Send>() {}
//! assert_send::<win32::UiSender<String>>();
//! ```

#![cfg(windows)]

mod app;
mod canvas;
mod capture;
pub mod controls;
mod dialog;
mod elevation;
mod filedialog;
mod font;
mod imagelist;
pub mod layout;
mod lists;
mod listview;
mod menu;
pub mod metrics;
mod registry;
mod richedit;
mod sender;
mod serial;
mod service;
mod sheet;
mod statusbar;
mod taskdialog;
pub mod time;
mod toolbar;
mod util;
mod window;

pub use app::App;
pub use canvas::Canvas;
pub use capture::{Image, capture, gui_handles, move_pointer, pointer};
pub use dialog::{DialogTemplate, ModelessDialog};

/// The sizes the Windows guidelines give for dialogs, in dialog units.
pub mod units {
    pub use crate::dialog::{BUTTON_HEIGHT, BUTTON_WIDTH, FIELD_HEIGHT, GAP, MARGIN, TEXT_HEIGHT};
}
pub use elevation::{is_elevated, open_folder, relaunch_elevated, run_elevated};
pub use filedialog::{FileFilter, open_file, pick_folder, save_file};
pub use font::Font;
pub use imagelist::ImageList;
pub use lists::{CheckList, ListBox};
pub use listview::{Align, Cell, Column, ListView, Sort};
pub use menu::{Accelerators, Key, Menu, MenuItem};
pub use registry::UserKey;
pub use richedit::RichEdit;
pub use sender::{UiReceiver, UiSender, channel};
pub use serial::{ModemStatus, SerialPort, serial_ports};
pub use service::{ServiceAction, service_control, service_state};
pub use sheet::property_sheet;
pub use statusbar::StatusBar;
pub use taskdialog::{
    TaskButtons, TaskDialog, TaskIcon, TaskWatcher, task_dialog, task_dialog_watched,
};
pub use toolbar::{ToolButton, Toolbar};
pub use window::{
    Colour, Event, Handler, Hwnd, MenuHighlight, Rect, Reply, Window, WindowOptions, code, focused,
    id,
};
