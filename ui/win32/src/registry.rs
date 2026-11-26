// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A key of the user's registry, for what a program remembers about its
//! window: placement, column widths, the sort and the filter.
//!
//! Everything here is under `HKEY_CURRENT_USER`, which a program may
//! always read and write for its own user. A value that is not there, or
//! is not of the kind asked for, is `None`; a write that fails is given
//! up, because nothing that is kept here is worth an error message.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_BINARY, REG_DWORD, REG_OPTION_NON_VOLATILE,
    REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

use crate::util::{from_wide, pcwstr, wide};

/// The most data read for one value, in bytes.
const MAX_VALUE: usize = 64 * 1024;

/// A key under `HKEY_CURRENT_USER`, closed when dropped.
pub struct UserKey {
    handle: HKEY,
}

impl UserKey {
    /// Opens `HKEY_CURRENT_USER\<path>`, creating it and the keys above
    /// it if need be.
    pub fn open(path: &str) -> Option<UserKey> {
        let path = wide(path);
        let mut handle = HKEY::default();
        // SAFETY: the path is terminated UTF-16 that outlives the call,
        // and `handle` is where the call writes the key.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                pcwstr(&path),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                REG_SAM_FLAGS(KEY_READ.0 | KEY_WRITE.0),
                None,
                &mut handle,
                None,
            )
        };
        (status == ERROR_SUCCESS).then_some(UserKey { handle })
    }

    /// Removes `HKEY_CURRENT_USER\<path>` and everything under it.
    pub fn delete(path: &str) {
        let path = wide(path);
        // SAFETY: the path is terminated UTF-16 that outlives the call.
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, pcwstr(&path));
        }
    }

    /// The data of a value, if it is of the kind asked for.
    fn read(&self, name: &str, kind: REG_VALUE_TYPE) -> Option<Vec<u8>> {
        let name = wide(name);
        let mut found = REG_VALUE_TYPE::default();
        let mut size = 0u32;
        // SAFETY: the name is terminated UTF-16 that outlives the calls.
        // The first call asks only for the kind and the size; the second
        // is given a buffer of that size.
        unsafe {
            let status = RegQueryValueExW(
                self.handle,
                pcwstr(&name),
                None,
                Some(&mut found),
                None,
                Some(&mut size),
            );
            if status != ERROR_SUCCESS || found != kind || size as usize > MAX_VALUE {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            let status = RegQueryValueExW(
                self.handle,
                pcwstr(&name),
                None,
                None,
                Some(data.as_mut_ptr()),
                Some(&mut size),
            );
            if status != ERROR_SUCCESS {
                return None;
            }
            data.truncate(size as usize);
            Some(data)
        }
    }

    fn write(&self, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) {
        let name = wide(name);
        // SAFETY: the name is terminated UTF-16 and the data a slice, both
        // of which outlive the call, which copies them.
        unsafe {
            let _ = RegSetValueExW(self.handle, pcwstr(&name), None, kind, Some(data));
        }
    }

    /// A number.
    pub fn number(&self, name: &str) -> Option<u32> {
        let data = self.read(name, REG_DWORD)?;
        Some(u32::from_le_bytes(data.try_into().ok()?))
    }

    /// Keeps a number.
    pub fn set_number(&self, name: &str, value: u32) {
        self.write(name, REG_DWORD, &value.to_le_bytes());
    }

    /// A string.
    pub fn string(&self, name: &str) -> Option<String> {
        let data = self.read(name, REG_SZ)?;
        let units: Vec<u16> = data
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        Some(from_wide(&units))
    }

    /// Keeps a string.
    pub fn set_string(&self, name: &str, value: &str) {
        let data: Vec<u8> = wide(value)
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect();
        self.write(name, REG_SZ, &data);
    }

    /// Bytes.
    pub fn bytes(&self, name: &str) -> Option<Vec<u8>> {
        self.read(name, REG_BINARY)
    }

    /// Keeps bytes.
    pub fn set_bytes(&self, name: &str, value: &[u8]) {
        self.write(name, REG_BINARY, value);
    }
}

impl Drop for UserKey {
    fn drop(&mut self) {
        // SAFETY: the key was opened by `open` and is closed nowhere else.
        unsafe {
            let _ = RegCloseKey(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of this test's own, removed when the test ends.
    struct Scratch(String);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let path = format!(
                r"Software\Quartz Systems\rustmodem\Tests\{}-{name}",
                std::process::id()
            );
            UserKey::delete(&path);
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            UserKey::delete(&self.0);
        }
    }

    #[test]
    fn what_is_kept_comes_back_and_what_is_not_there_is_none() {
        let scratch = Scratch::new("values");
        let key = UserKey::open(&scratch.0).unwrap();
        assert_eq!(key.number("Sort"), None);
        assert_eq!(key.string("Filter"), None);
        assert_eq!(key.bytes("Placement"), None);

        key.set_number("Sort", 0x8000_0003);
        key.set_string("Filter", "On a call — né");
        key.set_bytes("Placement", &[1, 2, 3, 0, 255]);
        key.set_bytes("Empty", &[]);
        key.set_string("Blank", "");
        assert_eq!(key.number("Sort"), Some(0x8000_0003));
        assert_eq!(key.string("Filter").as_deref(), Some("On a call — né"));
        assert_eq!(key.bytes("Placement"), Some(vec![1, 2, 3, 0, 255]));
        assert_eq!(key.bytes("Empty"), Some(Vec::new()));
        assert_eq!(key.string("Blank").as_deref(), Some(""));

        // A value of another kind than the one asked for is none.
        assert_eq!(key.string("Sort"), None);
        assert_eq!(key.number("Filter"), None);
        assert_eq!(key.bytes("Sort"), None);

        // Another handle on the same key sees the same, as the next run
        // of the program does.
        drop(key);
        let again = UserKey::open(&scratch.0).unwrap();
        assert_eq!(again.number("Sort"), Some(0x8000_0003));
        again.set_number("Sort", 1);
        assert_eq!(again.number("Sort"), Some(1));
    }

    #[test]
    fn a_key_that_is_deleted_is_empty_when_opened_again() {
        let scratch = Scratch::new("delete");
        UserKey::open(&scratch.0).unwrap().set_number("Kept", 7);
        UserKey::delete(&scratch.0);
        assert_eq!(UserKey::open(&scratch.0).unwrap().number("Kept"), None);
    }
}
