// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The file dialogs: the system's common item dialogs for opening a file,
//! saving one, and picking a folder.

use std::path::PathBuf;

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST,
    FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog,
    SIGDN_FILESYSPATH,
};
use windows::core::Interface;

use crate::util::{pcwstr, wide};
use crate::window::Hwnd;

/// One entry of the list of file types: what it is called and the pattern
/// it matches, for example `CSV files` and `*.csv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFilter {
    /// The name, for people.
    pub name: String,
    /// The pattern; several are separated by semicolons.
    pub pattern: String,
}

impl FileFilter {
    /// A filter.
    pub fn new(name: &str, pattern: &str) -> FileFilter {
        FileFilter {
            name: name.to_string(),
            pattern: pattern.to_string(),
        }
    }
}

/// What the three dialogs have in common: the title, the file types and a
/// file name to start with, then showing the dialog and taking the path
/// that was chosen. `None` if the user cancelled or the dialog could not
/// be shown.
fn run(
    dialog: &IFileDialog,
    owner: Option<Hwnd>,
    title: &str,
    filters: &[FileFilter],
    file_name: &str,
    options: u32,
) -> Option<PathBuf> {
    let title = wide(title);
    let file_name = wide(file_name);
    let names: Vec<(Vec<u16>, Vec<u16>)> = filters
        .iter()
        .map(|filter| (wide(&filter.name), wide(&filter.pattern)))
        .collect();
    let specs: Vec<COMDLG_FILTERSPEC> = names
        .iter()
        .map(|(name, pattern)| COMDLG_FILTERSPEC {
            pszName: pcwstr(name),
            pszSpec: pcwstr(pattern),
        })
        .collect();
    // SAFETY: the calls are on a live dialog object; every string and the
    // array of filters live until after the calls that take them, which
    // copy them. `Show` returns when the dialog has closed. The path the
    // result gives is allocated by COM and freed here once copied.
    unsafe {
        let _ = dialog.SetTitle(pcwstr(&title));
        if !specs.is_empty() {
            let _ = dialog.SetFileTypes(&specs);
        }
        if file_name.len() > 1 {
            let _ = dialog.SetFileName(pcwstr(&file_name));
        }
        if let Ok(current) = dialog.GetOptions() {
            // Only what is in the file system: a path is what is wanted.
            let _ = dialog.SetOptions(windows::Win32::UI::Shell::FILEOPENDIALOGOPTIONS(
                current.0 | options | FOS_FORCEFILESYSTEM.0,
            ));
        }
        dialog.Show(owner.map(|owner| owner.0)).ok()?;
        let item = dialog.GetResult().ok()?;
        let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let chosen = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0 as *const _));
        chosen
    }
}

/// Asks where to save a file. `file_name` is the name to start with, and
/// `extension`, without its dot, is added to a name typed without one.
/// The dialog asks before an existing file is replaced.
pub fn save_file(
    owner: Option<Hwnd>,
    title: &str,
    file_name: &str,
    extension: &str,
    filters: &[FileFilter],
) -> Option<PathBuf> {
    // SAFETY: the class and the interface are the system's own; COM was
    // initialised for this thread by `App::new`.
    let dialog: IFileSaveDialog =
        unsafe { CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let extension = wide(extension);
    if extension.len() > 1 {
        // SAFETY: the string is terminated and outlives the call.
        unsafe {
            let _ = dialog.SetDefaultExtension(pcwstr(&extension));
        }
    }
    let dialog: IFileDialog = dialog.cast().ok()?;
    run(
        &dialog,
        owner,
        title,
        filters,
        file_name,
        (FOS_OVERWRITEPROMPT | FOS_PATHMUSTEXIST).0,
    )
}

/// Asks for a file to open, which must exist.
pub fn open_file(owner: Option<Hwnd>, title: &str, filters: &[FileFilter]) -> Option<PathBuf> {
    // SAFETY: as in `save_file`.
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let dialog: IFileDialog = dialog.cast().ok()?;
    run(&dialog, owner, title, filters, "", FOS_FILEMUSTEXIST.0)
}

/// Asks for a folder.
pub fn pick_folder(owner: Option<Hwnd>, title: &str) -> Option<PathBuf> {
    // SAFETY: as in `save_file`.
    let dialog: IFileOpenDialog =
        unsafe { CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let dialog: IFileDialog = dialog.cast().ok()?;
    run(&dialog, owner, title, &[], "", FOS_PICKFOLDERS.0)
}
