// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Task dialogs: the system's dialog for a message or a question.
//!
//! This is what a message box is in a program of this century: a title, a
//! main instruction, optional content, optional detail that the user
//! expands, the system's buttons or command links with a line of text
//! each. The manager shows every `RMD` message this way.

use windows::Win32::Foundation::{HWND, LPARAM, S_OK, WPARAM};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_COMMON_BUTTON_FLAGS, TASKDIALOG_FLAGS, TASKDIALOG_NOTIFICATIONS,
    TASKDIALOGCONFIG, TD_ERROR_ICON, TD_INFORMATION_ICON, TD_SHIELD_ICON, TD_WARNING_ICON,
    TDCBF_CANCEL_BUTTON, TDCBF_CLOSE_BUTTON, TDCBF_NO_BUTTON, TDCBF_OK_BUTTON, TDCBF_YES_BUTTON,
    TDF_ALLOW_DIALOG_CANCELLATION, TDF_CALLBACK_TIMER, TDF_POSITION_RELATIVE_TO_WINDOW,
    TDF_USE_COMMAND_LINKS, TDM_CLICK_BUTTON, TDN_TIMER, TaskDialogIndirect,
};
use windows::core::{HRESULT, PCWSTR};

use crate::util::{instance, pcwstr, wide};
use crate::window::{Hwnd, id};

/// The icon beside the main instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TaskIcon {
    /// No icon.
    #[default]
    None,
    /// Something happened.
    Information,
    /// Something is wrong, and the user decides what to do about it.
    Warning,
    /// Something failed.
    Error,
    /// What follows needs administrator rights.
    Shield,
}

/// The system's buttons a task dialog has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TaskButtons {
    /// OK; its id is [`id::OK`].
    pub ok: bool,
    /// Yes; its id is [`id::YES`].
    pub yes: bool,
    /// No; its id is [`id::NO`].
    pub no: bool,
    /// Cancel; its id is [`id::CANCEL`].
    pub cancel: bool,
    /// Close; its id is [`id::CLOSE`].
    pub close: bool,
}

impl TaskButtons {
    /// OK alone.
    pub const OK: TaskButtons = TaskButtons {
        ok: true,
        yes: false,
        no: false,
        cancel: false,
        close: false,
    };

    /// Yes and No.
    pub const YES_NO: TaskButtons = TaskButtons {
        ok: false,
        yes: true,
        no: true,
        cancel: false,
        close: false,
    };

    /// Cancel alone, for under command links.
    pub const CANCEL: TaskButtons = TaskButtons {
        ok: false,
        yes: false,
        no: false,
        cancel: true,
        close: false,
    };

    /// Close alone.
    pub const CLOSE: TaskButtons = TaskButtons {
        ok: false,
        yes: false,
        no: false,
        cancel: false,
        close: true,
    };
}

/// A task dialog.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TaskDialog {
    /// The text of the title bar.
    pub title: String,
    /// The main instruction: one line, in the large type.
    pub instruction: String,
    /// What is said under it, if anything.
    pub content: String,
    /// Detail the user may expand, if any.
    pub expanded: String,
    /// The icon.
    pub icon: TaskIcon,
    /// The system's buttons.
    pub buttons: TaskButtons,
    /// Command links: each its id and its text. A line break in the text
    /// starts the smaller note under the link.
    pub links: Vec<(u16, String)>,
    /// The button or link Enter presses; the first, if `None`.
    pub default: Option<u16>,
}

/// What watches a task dialog while it is up: called a few times a
/// second with the dialog's window and how long it has been up, in
/// milliseconds. An answer presses the button with that id.
pub type TaskWatcher<'a> = &'a mut dyn FnMut(Hwnd, u32) -> Option<u16>;

/// What the dialog's callback is given: the watcher.
struct Watched<'a, 'b> {
    watcher: &'a mut TaskWatcher<'b>,
}

/// The task dialog's callback, for a dialog that is watched.
unsafe extern "system" fn watch(
    window: HWND,
    notification: TASKDIALOG_NOTIFICATIONS,
    wparam: WPARAM,
    _lparam: LPARAM,
    data: isize,
) -> HRESULT {
    if notification == TDN_TIMER && data != 0 {
        // SAFETY: `data` is the address of the `Watched` that
        // `show` keeps on its stack for as long as the dialog is up, and
        // the dialog calls back on the thread that showed it, so nothing
        // else has the watcher.
        let watched = unsafe { &mut *(data as *mut Watched<'_, '_>) };
        let dialog = Hwnd(window);
        if let Some(button) = (watched.watcher)(dialog, wparam.0 as u32) {
            dialog.send(TDM_CLICK_BUTTON.0 as u32, usize::from(button), 0);
        }
    }
    S_OK
}

/// Shows a task dialog and returns the id of the button or link that
/// closed it. Closing the dialog any other way is [`id::CANCEL`], and so
/// is a dialog that could not be shown.
pub fn task_dialog(owner: Option<Hwnd>, dialog: &TaskDialog) -> u16 {
    show(owner, dialog, None)
}

/// As [`task_dialog`], with something watching the dialog while it is
/// up. For tests, which take a picture of the dialog and then press one
/// of its buttons.
pub fn task_dialog_watched(
    owner: Option<Hwnd>,
    dialog: &TaskDialog,
    mut watcher: TaskWatcher<'_>,
) -> u16 {
    show(owner, dialog, Some(&mut watcher))
}

fn show(owner: Option<Hwnd>, dialog: &TaskDialog, watcher: Option<&mut TaskWatcher<'_>>) -> u16 {
    let title = wide(&dialog.title);
    let instruction = wide(&dialog.instruction);
    let content = wide(&dialog.content);
    let expanded = wide(&dialog.expanded);
    let link_texts: Vec<Vec<u16>> = dialog.links.iter().map(|(_, text)| wide(text)).collect();
    let links: Vec<TASKDIALOG_BUTTON> = dialog
        .links
        .iter()
        .zip(&link_texts)
        .map(|((link, _), text)| TASKDIALOG_BUTTON {
            nButtonID: i32::from(*link),
            pszButtonText: pcwstr(text),
        })
        .collect();

    let mut common = 0;
    for (wanted, flag) in [
        (dialog.buttons.ok, TDCBF_OK_BUTTON),
        (dialog.buttons.yes, TDCBF_YES_BUTTON),
        (dialog.buttons.no, TDCBF_NO_BUTTON),
        (dialog.buttons.cancel, TDCBF_CANCEL_BUTTON),
        (dialog.buttons.close, TDCBF_CLOSE_BUTTON),
    ] {
        if wanted {
            common |= flag.0;
        }
    }
    // A dialog with nothing to press gets OK, as the system would give it.
    if common == 0 && links.is_empty() {
        common = TDCBF_OK_BUTTON.0;
    }
    let mut flags = TDF_POSITION_RELATIVE_TO_WINDOW.0;
    if !links.is_empty() {
        flags |= TDF_USE_COMMAND_LINKS.0;
    }
    // Escape and the close button work when there is a Cancel or a Close
    // to stand for them; this lets them work with OK alone too.
    if dialog.buttons.ok && !dialog.buttons.cancel && links.is_empty() {
        flags |= TDF_ALLOW_DIALOG_CANCELLATION.0;
    }
    let mut watched = watcher.map(|watcher| Watched { watcher });
    if watched.is_some() {
        flags |= TDF_CALLBACK_TIMER.0;
    }

    let optional = |text: &Vec<u16>| {
        if text.len() > 1 {
            pcwstr(text)
        } else {
            PCWSTR::null()
        }
    };
    let mut config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner.map(|owner| owner.0).unwrap_or_default(),
        hInstance: instance(),
        dwFlags: TASKDIALOG_FLAGS(flags),
        dwCommonButtons: TASKDIALOG_COMMON_BUTTON_FLAGS(common),
        pszWindowTitle: pcwstr(&title),
        pszMainInstruction: optional(&instruction),
        pszContent: optional(&content),
        cButtons: links.len() as u32,
        pButtons: if links.is_empty() {
            std::ptr::null()
        } else {
            links.as_ptr()
        },
        nDefaultButton: dialog.default.map_or(0, i32::from),
        pszExpandedInformation: optional(&expanded),
        ..TASKDIALOGCONFIG::default()
    };
    if let Some(watched) = watched.as_mut() {
        config.pfCallback = Some(watch);
        config.lpCallbackData = std::ptr::from_mut(watched) as isize;
    }
    config.Anonymous1.pszMainIcon = match dialog.icon {
        TaskIcon::None => PCWSTR::null(),
        TaskIcon::Information => TD_INFORMATION_ICON,
        TaskIcon::Warning => TD_WARNING_ICON,
        TaskIcon::Error => TD_ERROR_ICON,
        TaskIcon::Shield => TD_SHIELD_ICON,
    };
    let mut pressed = 0;
    // SAFETY: `config` is a whole TASKDIALOGCONFIG with its size filled
    // in; every string, the array of links and the watcher it points at
    // live until after the call, which returns when the dialog has
    // closed.
    let shown = unsafe { TaskDialogIndirect(&config, Some(&mut pressed), None, None) };
    match shown {
        Ok(()) => u16::try_from(pressed).unwrap_or(id::CANCEL),
        Err(_) => id::CANCEL,
    }
}
