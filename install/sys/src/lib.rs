// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the setup program needs of Win32 that has no safe form elsewhere:
//! making and removing devices, the COM port database, the machine's
//! registry, and shortcuts.
//!
//! Every `unsafe` block of the installer is in this crate, each with its
//! reason. The setup program itself forbids `unsafe` and reaches Windows
//! through these wrappers, through `ui/win32` for its dialogs, and through
//! Windows' own command-line tools.
//!
//! Everything here needs an administrator. Nothing here is called by the
//! service.

#![cfg(windows)]

pub mod comdb;
pub mod devices;
pub mod registry;
pub mod shortcut;

/// Text as the octets of a registry string: UTF-16, with as many NULs
/// after it as the type of value wants.
fn wide_octets(text: &str, terminators: usize) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::repeat_n(0, terminators))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}
