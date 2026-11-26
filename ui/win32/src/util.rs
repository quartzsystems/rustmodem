// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Strings to and from UTF-16, and the instance handle.

use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::core::PCWSTR;

/// Text as terminated UTF-16, for a Win32 call. An embedded NUL would end
/// the string early for Windows, so the text is cut there.
pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16()
        .take_while(|unit| *unit != 0)
        .chain(std::iter::once(0))
        .collect()
}

/// The text of a UTF-16 buffer, up to its terminator if it has one.
pub(crate) fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// Copies text into a buffer Windows gave, of `capacity` UTF-16 units,
/// terminator included. Text that does not fit is cut short.
///
/// # Safety
///
/// `buffer` must be valid for writing `capacity` UTF-16 units.
pub(crate) unsafe fn copy_to_buffer(text: &str, buffer: *mut u16, capacity: usize) {
    if buffer.is_null() || capacity == 0 {
        return;
    }
    let mut written = 0;
    for unit in text.encode_utf16().take_while(|unit| *unit != 0) {
        if written + 1 >= capacity {
            break;
        }
        // SAFETY: `written` is below `capacity - 1`, inside the buffer the
        // caller vouches for.
        unsafe { buffer.add(written).write(unit) };
        written += 1;
    }
    // SAFETY: `written` is at most `capacity - 1`.
    unsafe { buffer.add(written).write(0) };
}

/// A pointer to terminated UTF-16 for a Win32 call. The vector must
/// outlive the call.
pub(crate) fn pcwstr(text: &[u16]) -> PCWSTR {
    PCWSTR(text.as_ptr())
}

/// The handle of this program's module.
pub(crate) fn instance() -> HINSTANCE {
    // SAFETY: with no name the call returns the handle of the program
    // itself and cannot fail.
    unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map(|module| HINSTANCE(module.0))
        .unwrap_or_default()
}

/// The low 16 bits.
pub(crate) fn loword(value: usize) -> u16 {
    (value & 0xFFFF) as u16
}

/// Bits 16 to 31.
pub(crate) fn hiword(value: usize) -> u16 {
    ((value >> 16) & 0xFFFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_to_utf16_and_back() {
        assert_eq!(wide("abc"), [97, 98, 99, 0]);
        assert_eq!(wide(""), [0]);
        assert_eq!(wide("a\0b"), [97, 0]);
        assert_eq!(from_wide(&wide("Line 1 — idle")), "Line 1 — idle");
        assert_eq!(from_wide(&[104, 105]), "hi");
        assert_eq!(from_wide(&[104, 0, 105]), "h");
    }

    #[test]
    fn text_copied_to_a_buffer_is_cut_to_fit_and_terminated() {
        let mut buffer = [0xFFFFu16; 6];
        // SAFETY: the buffer is six units long.
        unsafe { copy_to_buffer("abc", buffer.as_mut_ptr(), 6) };
        assert_eq!(buffer, [97, 98, 99, 0, 0xFFFF, 0xFFFF]);
        // SAFETY: as above, with a shorter capacity given.
        unsafe { copy_to_buffer("abcdefgh", buffer.as_mut_ptr(), 4) };
        assert_eq!(buffer[..4], [97, 98, 99, 0]);
        // SAFETY: a capacity of zero writes nothing.
        unsafe { copy_to_buffer("x", buffer.as_mut_ptr(), 0) };
        assert_eq!(buffer[0], 97);
        // SAFETY: a null buffer is ignored.
        unsafe { copy_to_buffer("x", std::ptr::null_mut(), 4) };
    }

    #[test]
    fn words_come_out_of_a_message_parameter() {
        assert_eq!(loword(0x1234_5678), 0x5678);
        assert_eq!(hiword(0x1234_5678), 0x1234);
        assert_eq!(hiword(0xFFFF_0000), 0xFFFF);
    }
}
