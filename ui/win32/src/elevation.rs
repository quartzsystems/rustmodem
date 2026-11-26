// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Administrator rights: whether the program has them, and starting it
//! again with them.

use std::io;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::w;

use crate::util::{pcwstr, wide};
use crate::window::Hwnd;

/// Whether the program is running with administrator rights.
pub fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: the process handle is the pseudo-handle of this process;
    // `token` is where the call writes a handle, which is closed below;
    // and `elevation` is a buffer of the size given.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let asked = GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        );
        let _ = CloseHandle(token);
        asked.is_ok() && elevation.TokenIsElevated != 0
    }
}

/// One argument as the Windows command line spells it: in quotes if it
/// has a space or is empty, with the backslashes before a quote doubled.
pub(crate) fn quote_argument(argument: &str) -> String {
    let plain = !argument.is_empty() && !argument.chars().any(|c| matches!(c, ' ' | '\t' | '"'));
    if plain {
        return argument.to_string();
    }
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in argument.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // The backslashes before a quote are doubled, and the
                // quote itself gets one of its own.
                quoted.extend(std::iter::repeat_n('\\', backslashes + 1));
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
        quoted.push(c);
    }
    // The backslashes before the closing quote are doubled too.
    quoted.extend(std::iter::repeat_n('\\', backslashes));
    quoted.push('"');
    quoted
}

/// Starts this program again with administrator rights and the arguments
/// given. Windows asks the user first; if they refuse, the error is
/// "cancelled". The caller exits when this succeeds.
pub fn relaunch_elevated(owner: Option<Hwnd>, arguments: &[String]) -> io::Result<()> {
    let program = std::env::current_exe()?;
    let program = wide(&program.display().to_string());
    let parameters: Vec<String> = arguments.iter().map(|a| quote_argument(a)).collect();
    let parameters = wide(&parameters.join(" "));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // The call returns only when the new process has been started,
        // which this process, about to exit, needs.
        fMask: SEE_MASK_NOASYNC,
        hwnd: owner.map(|owner| owner.0).unwrap_or_default(),
        // The verb that asks for elevation.
        lpVerb: w!("runas"),
        lpFile: pcwstr(&program),
        lpParameters: pcwstr(&parameters),
        nShow: SW_SHOWNORMAL.0,
        ..SHELLEXECUTEINFOW::default()
    };
    // SAFETY: `info` is a whole SHELLEXECUTEINFOW with its size filled in,
    // and every string it points at outlives the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|error| {
        /// What an HRESULT made from a Win32 error code starts with.
        const FACILITY_WIN32: u32 = 0x8007;
        let code = error.code().0 as u32;
        if code >> 16 == FACILITY_WIN32 {
            io::Error::from_raw_os_error((code & 0xFFFF) as i32)
        } else {
            error.into()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_as_the_command_line_wants() {
        assert_eq!(quote_argument("--demo"), "--demo");
        assert_eq!(quote_argument("rustmodem-control"), "rustmodem-control");
        assert_eq!(quote_argument(""), "\"\"");
        assert_eq!(quote_argument("two words"), "\"two words\"");
        assert_eq!(
            quote_argument(r"C:\Program Files\x"),
            r#""C:\Program Files\x""#
        );
        // A backslash at the end of a quoted argument must not swallow
        // the closing quote.
        assert_eq!(quote_argument(r"C:\My Dir\"), r#""C:\My Dir\\""#);
        assert_eq!(quote_argument(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_argument(r#"a\"b"#), r#""a\\\"b""#);
    }

    #[test]
    fn whether_the_program_is_elevated_can_be_asked() {
        // Either answer is right; asking must not fail, and must give the
        // same answer twice.
        assert_eq!(is_elevated(), is_elevated());
    }
}
