// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The application: what is set up once, and the message loop.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::UI::Controls::{
    ICC_BAR_CLASSES, ICC_LISTVIEW_CLASSES, ICC_PROGRESS_CLASS, ICC_STANDARD_CLASSES,
    ICC_TAB_CLASSES, ICC_UPDOWN_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IsChild, IsDialogMessageW, MSG, PM_REMOVE, PeekMessageW,
    PostQuitMessage, TranslateAcceleratorW, TranslateMessage, WM_QUIT,
};
use windows::core::w;

use crate::menu::Accelerators;
use crate::window::{Hwnd, dialog_windows};

/// The application. Make one before any window. Copying it copies
/// nothing: it stands for the set-up having been done.
#[derive(Clone, Copy)]
pub struct App {
    _private: (),
}

impl App {
    /// Sets up what the controls need: the classes of the common controls
    /// this crate wraps, the rich edit control, and COM for the file
    /// dialogs.
    pub fn new() -> App {
        let classes = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            // The list view, the toolbar and status bar, the progress bar,
            // the tab control, the up-down control, and the standard
            // controls in their themed form.
            dwICC: ICC_LISTVIEW_CLASSES
                | ICC_BAR_CLASSES
                | ICC_PROGRESS_CLASS
                | ICC_TAB_CLASSES
                | ICC_UPDOWN_CLASS
                | ICC_STANDARD_CLASSES,
        };
        // SAFETY: `classes` is a whole INITCOMMONCONTROLSEX with its size
        // filled in. The library is the one that registers RICHEDIT50W;
        // it stays loaded for the life of the program, which is what a
        // program that uses the class wants. COM is initialised for this
        // thread, which is the one that shows the file dialogs; a thread
        // that already has COM in another mode keeps it, and the file
        // dialogs then report an error instead of showing.
        unsafe {
            let _ = InitCommonControlsEx(&classes);
            let _ = LoadLibraryW(w!("Msftedit.dll"));
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        App { _private: () }
    }

    /// Runs the message loop until [`App::quit`]. Keys go first to the
    /// accelerators of `main`, then to dialog navigation in the windows
    /// that asked for it, and then to whichever control has the focus.
    /// Returns the exit code given to `quit`.
    pub fn run(&self, main: Hwnd, accelerators: Option<&Accelerators>) -> i32 {
        let mut message = MSG::default();
        loop {
            // SAFETY: `message` is valid for the call to fill. With no
            // window given, messages for every window of the thread come.
            let got = unsafe { GetMessageW(&mut message, None, 0, 0) };
            if got.0 <= 0 {
                // Zero is WM_QUIT; minus one is an error, which with these
                // arguments cannot happen and must not spin if it does.
                return message.wParam.0 as i32;
            }
            handle(&message, main.0, accelerators);
        }
    }

    /// Handles the messages that are waiting and returns. For tests, which
    /// drive a window and then look at it. Returns `false` once the loop
    /// has been asked to end.
    pub fn pump(&self, main: Hwnd, accelerators: Option<&Accelerators>) -> bool {
        let mut message = MSG::default();
        // SAFETY: `message` is valid for the call to fill.
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            if message.message == WM_QUIT {
                return false;
            }
            handle(&message, main.0, accelerators);
        }
        true
    }

    /// Ends the message loop.
    pub fn quit(code: i32) {
        // SAFETY: the call posts WM_QUIT to this thread.
        unsafe { PostQuitMessage(code) };
    }
}

impl Default for App {
    fn default() -> Self {
        App::new()
    }
}

/// One message: an accelerator, a key of dialog navigation, or a message
/// for a window.
fn handle(message: &MSG, main: HWND, accelerators: Option<&Accelerators>) {
    // SAFETY: `message` is a message this thread just took off its queue;
    // the handles are windows of this thread or stale, and each of these
    // calls does nothing for a handle that is not a window.
    unsafe {
        // The accelerators are the main window's: a key pressed in another
        // window of the thread, such as a modeless dialog, is that
        // window's own.
        if let Some(accelerators) = accelerators
            && (message.hwnd == main || IsChild(main, message.hwnd).as_bool())
            && TranslateAcceleratorW(main, accelerators.handle(), message) != 0
        {
            return;
        }
        for window in dialog_windows() {
            if IsDialogMessageW(window, message).as_bool() {
                return;
            }
        }
        let _ = TranslateMessage(message);
        DispatchMessageW(message);
    }
}
