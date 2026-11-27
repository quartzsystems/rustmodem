// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Shortcuts, for the Start menu, and where the Start menu of all users
//! is.

use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize, IPersistFile,
};
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
};
use windows::core::{HSTRING, Interface};

/// The Programs folder of the Start menu of all users.
pub fn common_programs() -> io::Result<PathBuf> {
    // SAFETY: the string the system gives is read to its NUL and then
    // freed, as the function's contract asks.
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_CommonPrograms, KF_FLAG_DEFAULT, None)?;
        let text = path.to_string();
        CoTaskMemFree(Some(path.0.cast()));
        text.map(PathBuf::from)
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
    }
}

/// Writes a shortcut at `link` to `target`, with arguments and a
/// description.
pub fn create(link: &Path, target: &Path, arguments: &str, description: &str) -> io::Result<()> {
    // SAFETY: COM is initialised for this thread for the length of the
    // call and released after it; every string lives through the call it
    // is given to.
    unsafe {
        let initialised = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let result = (|| -> io::Result<()> {
            let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            shell.SetPath(&HSTRING::from(target))?;
            shell.SetArguments(&HSTRING::from(arguments))?;
            shell.SetDescription(&HSTRING::from(description))?;
            if let Some(directory) = target.parent() {
                shell.SetWorkingDirectory(&HSTRING::from(directory))?;
            }
            let file: IPersistFile = shell.cast()?;
            file.Save(&HSTRING::from(link), true)?;
            Ok(())
        })();
        if initialised {
            CoUninitialize();
        }
        result
    }
}
