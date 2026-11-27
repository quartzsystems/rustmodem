// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The COM port database: Windows' record of which COM numbers are taken.
//!
//! A port whose name setup chooses is not given its number by the Ports
//! class installer, so setup claims the number itself, or the next port
//! Windows names by itself could be given the same one.

use std::io;

use windows::Win32::Devices::SerialCommunication::{
    CDB_REPORT_BYTES, ComDBClaimPort, ComDBClose, ComDBGetCurrentPortUsage, ComDBOpen,
    ComDBReleasePort, HCOMDB,
};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::core::BOOL;

/// The greatest COM number the database has.
pub const HIGHEST: u32 = 256;

/// The database, closed when dropped.
struct Database(HCOMDB);

impl Database {
    fn open() -> io::Result<Database> {
        let mut handle = HCOMDB::default();
        // SAFETY: a local for the handle.
        let opened = unsafe { ComDBOpen(&mut handle) };
        if opened != ERROR_SUCCESS.0 as i32 {
            return Err(io::Error::from_raw_os_error(opened));
        }
        Ok(Database(handle))
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed once.
        unsafe {
            let _ = ComDBClose(self.0);
        }
    }
}

/// The COM numbers that are taken.
pub fn taken() -> io::Result<Vec<u32>> {
    let database = Database::open()?;
    // One octet for each number: 1 if it is taken.
    let mut usage = [0u8; HIGHEST as usize];
    let mut highest = 0u32;
    // SAFETY: the buffer is a local, given as a slice.
    let read = unsafe {
        ComDBGetCurrentPortUsage(
            database.0,
            Some(&mut usage),
            CDB_REPORT_BYTES,
            Some(&mut highest),
        )
    };
    if read != ERROR_SUCCESS.0 as i32 {
        return Err(io::Error::from_raw_os_error(read));
    }
    Ok(usage
        .iter()
        .enumerate()
        .filter(|(_, used)| **used != 0)
        .map(|(index, _)| index as u32 + 1)
        .collect())
}

/// Marks a COM number as taken. False if it already was.
pub fn claim(number: u32) -> io::Result<bool> {
    let database = Database::open()?;
    let mut forced = BOOL(0);
    // SAFETY: a local for the answer.
    let claimed = unsafe { ComDBClaimPort(database.0, number, false, Some(&mut forced)) };
    match claimed {
        0 => Ok(true),
        // ERROR_SHARING_VIOLATION: the number is in use.
        32 => Ok(false),
        other => Err(io::Error::from_raw_os_error(other)),
    }
}

/// Marks a COM number as free.
pub fn release(number: u32) -> io::Result<()> {
    let database = Database::open()?;
    // SAFETY: a handle this function opened.
    let released = unsafe { ComDBReleasePort(database.0, number) };
    if released != ERROR_SUCCESS.0 as i32 {
        return Err(io::Error::from_raw_os_error(released));
    }
    Ok(())
}
