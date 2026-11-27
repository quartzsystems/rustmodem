// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Keys of the machine's registry, `HKEY_LOCAL_MACHINE`: where a program
//! is listed as installed and where an event source is registered.

use std::io;

use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

use crate::{from_wide, wide_octets};

/// A key under `HKEY_LOCAL_MACHINE`, closed when dropped.
pub struct MachineKey(HKEY);

impl Drop for MachineKey {
    fn drop(&mut self) {
        // SAFETY: the key was opened by this type and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

impl MachineKey {
    /// Opens `path` for writing, making it and what is above it if they
    /// are not there.
    pub fn create(path: &str) -> io::Result<MachineKey> {
        let mut key = HKEY::default();
        // SAFETY: a local for the key; the path lives through the call.
        unsafe {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(path),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut key,
                None,
            )
            .ok()?;
        }
        Ok(MachineKey(key))
    }

    /// Opens `path` for reading, if it is there.
    pub fn open(path: &str) -> Option<MachineKey> {
        let mut key = HKEY::default();
        // SAFETY: as in `create`.
        let opened = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                &HSTRING::from(path),
                None,
                KEY_READ,
                &mut key,
            )
        };
        opened.is_ok().then_some(MachineKey(key))
    }

    fn set(&self, name: &str, kind: REG_VALUE_TYPE, octets: &[u8]) -> io::Result<()> {
        // SAFETY: the octets are a slice that lives through the call.
        unsafe {
            RegSetValueExW(self.0, &HSTRING::from(name), None, kind, Some(octets)).ok()?;
        }
        Ok(())
    }

    /// Sets a string value.
    pub fn set_string(&self, name: &str, value: &str) -> io::Result<()> {
        self.set(name, REG_SZ, &wide_octets(value, 1))
    }

    /// Sets a string value in which `%names%` are expanded when read.
    pub fn set_expandable(&self, name: &str, value: &str) -> io::Result<()> {
        self.set(name, REG_EXPAND_SZ, &wide_octets(value, 1))
    }

    /// Sets a number.
    pub fn set_number(&self, name: &str, value: u32) -> io::Result<()> {
        self.set(name, REG_DWORD, &value.to_le_bytes())
    }

    /// A string value, if it is there.
    pub fn string(&self, name: &str) -> Option<String> {
        let mut buffer = [0u16; 1024];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: the buffer is a local, given with its size in octets.
        let read = unsafe {
            RegGetValueW(
                self.0,
                PCWSTR::null(),
                &HSTRING::from(name),
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        read.is_ok().then(|| from_wide(&buffer))
    }

    /// A number, if it is there.
    pub fn number(&self, name: &str) -> Option<u32> {
        let mut value = 0u32;
        let mut size = 4u32;
        // SAFETY: a local of four octets, given with its size.
        let read = unsafe {
            RegGetValueW(
                self.0,
                PCWSTR::null(),
                &HSTRING::from(name),
                RRF_RT_REG_DWORD,
                None,
                Some((&raw mut value).cast()),
                Some(&mut size),
            )
        };
        read.is_ok().then_some(value)
    }
}

/// Removes `path` and everything under it. A key that is not there is
/// not an error.
pub fn delete(path: &str) -> io::Result<()> {
    if MachineKey::open(path).is_none() {
        return Ok(());
    }
    // SAFETY: the path lives through the call.
    unsafe {
        RegDeleteTreeW(HKEY_LOCAL_MACHINE, &HSTRING::from(path)).ok()?;
    }
    Ok(())
}
