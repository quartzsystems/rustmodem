// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Reading the Windows registry: keys under `HKEY_LOCAL_MACHINE`, their
//! subkeys and their string values. Nothing here writes.

use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, RegCloseKey, RegEnumKeyExW,
    RegEnumValueW, RegOpenKeyExW,
};
use windows::core::{PCWSTR, PWSTR};

/// The longest key name the registry allows, in characters, and one for the
/// terminator.
const MAX_KEY_NAME: usize = 256;

/// The longest value name the registry allows, in characters, and one for
/// the terminator.
const MAX_VALUE_NAME: usize = 16_384;

/// The most data read for one value, in bytes. The values read here are
/// port and device names; one longer than this is not one of them.
const MAX_VALUE_DATA: usize = 2048;

/// An open registry key, closed when dropped.
pub struct Key(HKEY);

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The text of a buffer of UTF-16 up to its first terminator.
fn text(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

impl Key {
    /// Opens `HKEY_LOCAL_MACHINE\<path>` for reading. `None` if there is
    /// no such key or it may not be read.
    pub fn open(path: &str) -> Option<Key> {
        Key::open_under(HKEY_LOCAL_MACHINE, path)
    }

    /// Opens a subkey for reading.
    pub fn subkey(&self, name: &str) -> Option<Key> {
        Key::open_under(self.0, name)
    }

    fn open_under(parent: HKEY, path: &str) -> Option<Key> {
        let path = wide(path);
        let mut key = HKEY::default();
        // SAFETY: `path` is a terminated UTF-16 string that outlives the
        // call, and `key` is where the call writes the handle.
        let status =
            unsafe { RegOpenKeyExW(parent, PCWSTR(path.as_ptr()), None, KEY_READ, &mut key) };
        (status == ERROR_SUCCESS).then_some(Key(key))
    }

    /// The names of the key's subkeys.
    pub fn subkeys(&self) -> Vec<String> {
        let mut names = Vec::new();
        for index in 0.. {
            let mut name = [0u16; MAX_KEY_NAME];
            let mut length = name.len() as u32;
            // SAFETY: `name` is a buffer of `length` characters, and the
            // call writes at most that many and sets `length` to what it
            // wrote. The other outputs are not asked for.
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status != ERROR_SUCCESS {
                // ERROR_NO_MORE_ITEMS, or the key went away.
                break;
            }
            names.push(text(&name[..(length as usize).min(name.len())]));
        }
        names
    }

    /// The key's string values: name and data. Values of other types, and
    /// values too long to be a name, are left out.
    pub fn strings(&self) -> Vec<(String, String)> {
        let mut values = Vec::new();
        let mut name = vec![0u16; MAX_VALUE_NAME];
        // Kept as u16 so that the data is aligned for UTF-16.
        let mut data = vec![0u16; MAX_VALUE_DATA / 2];
        for index in 0.. {
            let mut name_length = name.len() as u32;
            let mut data_bytes = (data.len() * 2) as u32;
            let mut kind = 0u32;
            // SAFETY: `name` is a buffer of `name_length` characters and
            // `data` one of `data_bytes` bytes; the call writes within
            // them and sets the lengths to what it wrote.
            let status = unsafe {
                RegEnumValueW(
                    self.0,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut name_length,
                    None,
                    Some(&mut kind),
                    Some(data.as_mut_ptr().cast::<u8>()),
                    Some(&mut data_bytes),
                )
            };
            if status == ERROR_MORE_DATA {
                continue;
            }
            if status != ERROR_SUCCESS {
                break;
            }
            if kind != REG_SZ.0 && kind != REG_EXPAND_SZ.0 {
                continue;
            }
            let characters = (data_bytes as usize / 2).min(data.len());
            values.push((
                text(&name[..(name_length as usize).min(name.len())]),
                text(&data[..characters]),
            ));
        }
        values
    }

    /// One string value of the key. Value names do not distinguish case.
    pub fn string(&self, name: &str) -> Option<String> {
        self.strings()
            .into_iter()
            .find(|(found, _)| found.eq_ignore_ascii_case(name))
            .map(|(_, data)| data)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open_under` and is closed
        // nowhere else.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_every_windows_has_can_be_read() {
        let key = Key::open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion").unwrap();
        // Value names do not distinguish case.
        let build = key.string("currentbuild").unwrap();
        assert!(build.chars().all(|c| c.is_ascii_digit()), "{build}");
        assert!(key.strings().len() > 3);

        let parent = Key::open(r"SOFTWARE\Microsoft").unwrap();
        let subkeys = parent.subkeys();
        assert!(subkeys.iter().any(|name| name == "Windows NT"));
        assert!(parent.subkey("Windows NT").is_some());
    }

    #[test]
    fn a_key_that_does_not_exist_is_none() {
        assert!(Key::open(r"SOFTWARE\Quartz Systems\no such key for the test").is_none());
        let key = Key::open("SOFTWARE").unwrap();
        assert!(key.subkey("no such key for the test").is_none());
        assert_eq!(key.string("no such value for the test"), None);
    }
}
