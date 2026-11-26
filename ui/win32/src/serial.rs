// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A serial port opened directly, for the manager's AT console.
//!
//! The console is a local terminal: it opens a COM port of this machine
//! as any terminal program would, and does not go through the service.
//! A [`SerialPort`] is shared between the thread that reads it and the UI
//! thread that writes to it and drives its signals; reads give up after a
//! short while so that neither waits long for the other.

use std::io;

use windows::Win32::Devices::Communication::{
    CLRDTR, CLRRTS, COMMTIMEOUTS, DCB, EscapeCommFunction, GetCommModemStatus, GetCommState,
    MODEM_STATUS_FLAGS, MS_CTS_ON, MS_DSR_ON, MS_RING_ON, MS_RLSD_ON, NOPARITY, ONESTOPBIT, SETDTR,
    SETRTS, SetCommState, SetCommTimeouts,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegEnumValueW, RegOpenKeyExW,
};
use windows::core::{PWSTR, w};

use crate::util::{from_wide, pcwstr, wide};

/// The rate the console opens a port at, in bit/s. A virtual port carries
/// data at any rate; a real one talks to a modem that finds the rate from
/// the first `AT`.
const BAUD_RATE: u32 = 115_200;

/// How long a read waits for the first byte before giving up, in
/// milliseconds.
const READ_TIMEOUT_MS: u32 = 50;

/// The bits of a DCB's flags: binary mode, which Windows requires, and
/// DTR and RTS under the program's control and on.
const DCB_BINARY: u32 = 0x0001;
const DCB_DTR_ENABLE: u32 = 0x0010;
const DCB_RTS_ENABLE: u32 = 0x1000;

/// The signals a modem drives toward its host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ModemStatus {
    /// Data carrier detect.
    pub dcd: bool,
    /// Data set ready.
    pub dsr: bool,
    /// Clear to send.
    pub cts: bool,
    /// Ring indicator.
    pub ri: bool,
}

/// An open serial port, closed when dropped.
pub struct SerialPort {
    handle: HANDLE,
}

// SAFETY: a file handle may be used from several threads at once; Windows
// serialises the operations on it. The raw pointer inside `HANDLE` is what
// stops the compiler from deriving these.
unsafe impl Send for SerialPort {}
// SAFETY: as above.
unsafe impl Sync for SerialPort {}

/// A Windows error as `std::io` knows it.
fn io_error(error: windows::core::Error) -> io::Error {
    /// What an HRESULT made from a Win32 error code starts with.
    const FACILITY_WIN32: u32 = 0x8007;
    let code = error.code().0 as u32;
    if code >> 16 == FACILITY_WIN32 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        error.into()
    }
}

impl SerialPort {
    /// Opens a port by its name, for example `COM5`, with DTR and RTS on.
    pub fn open(name: &str) -> io::Result<SerialPort> {
        // The device namespace form, which is the one that works for
        // every port name, not only COM1 to COM9.
        let path = wide(&format!(r"\\.\{name}"));
        // SAFETY: the path is terminated UTF-16 that outlives the call;
        // the other arguments are plain values.
        let handle = unsafe {
            CreateFileW(
                pcwstr(&path),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        }
        .map_err(io_error)?;
        // From here the port closes the handle when it is dropped.
        let port = SerialPort { handle };
        let mut settings = DCB {
            DCBlength: size_of::<DCB>() as u32,
            ..DCB::default()
        };
        let timeouts = COMMTIMEOUTS {
            // A read returns as soon as there is a gap after some bytes,
            // or after the timeout with none.
            ReadIntervalTimeout: 10,
            ReadTotalTimeoutMultiplier: 0,
            ReadTotalTimeoutConstant: READ_TIMEOUT_MS,
            WriteTotalTimeoutMultiplier: 0,
            WriteTotalTimeoutConstant: 2000,
        };
        // SAFETY: the handle is an open port, and the structures are
        // whole and valid for the calls to read and fill.
        unsafe {
            GetCommState(port.handle, &mut settings).map_err(io_error)?;
            settings.BaudRate = BAUD_RATE;
            settings.ByteSize = 8;
            settings.Parity = NOPARITY;
            settings.StopBits = ONESTOPBIT;
            settings._bitfield = DCB_BINARY | DCB_DTR_ENABLE | DCB_RTS_ENABLE;
            SetCommState(port.handle, &settings).map_err(io_error)?;
            SetCommTimeouts(port.handle, &timeouts).map_err(io_error)?;
        }
        Ok(port)
    }

    /// Reads what has arrived, waiting a short while if nothing has.
    /// Returns how many bytes were read, which may be none.
    pub fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;
        // SAFETY: the handle is an open port and the buffer a slice that
        // outlives the call, which is synchronous.
        unsafe { ReadFile(self.handle, Some(buffer), Some(&mut read), None) }.map_err(io_error)?;
        Ok(read as usize)
    }

    /// Writes bytes. Returns how many were written.
    pub fn write(&self, data: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        // SAFETY: as in `read`.
        unsafe { WriteFile(self.handle, Some(data), Some(&mut written), None) }
            .map_err(io_error)?;
        Ok(written as usize)
    }

    /// Drives DTR.
    pub fn set_dtr(&self, on: bool) -> io::Result<()> {
        // SAFETY: the handle is an open port.
        unsafe { EscapeCommFunction(self.handle, if on { SETDTR } else { CLRDTR }) }
            .map_err(io_error)
    }

    /// Drives RTS.
    pub fn set_rts(&self, on: bool) -> io::Result<()> {
        // SAFETY: the handle is an open port.
        unsafe { EscapeCommFunction(self.handle, if on { SETRTS } else { CLRRTS }) }
            .map_err(io_error)
    }

    /// The signals the far side is driving.
    pub fn status(&self) -> io::Result<ModemStatus> {
        let mut flags = MODEM_STATUS_FLAGS::default();
        // SAFETY: the handle is an open port and `flags` is valid for the
        // call to fill.
        unsafe { GetCommModemStatus(self.handle, &mut flags) }.map_err(io_error)?;
        Ok(ModemStatus {
            dcd: flags.contains(MS_RLSD_ON),
            dsr: flags.contains(MS_DSR_ON),
            cts: flags.contains(MS_CTS_ON),
            ri: flags.contains(MS_RING_ON),
        })
    }
}

impl Drop for SerialPort {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed nowhere
        // else.
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// The names of the serial ports of this machine, as the system lists
/// them, sorted so that `COM9` comes before `COM10`.
pub fn serial_ports() -> Vec<String> {
    let mut names = Vec::new();
    let mut key = HKEY::default();
    // SAFETY: the path is a terminated wide literal, and `key` is where
    // the call writes the handle, which is closed below. Each value is
    // read into buffers whose lengths the call is told.
    unsafe {
        let opened = RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DEVICEMAP\SERIALCOMM"),
            None,
            KEY_READ,
            &mut key,
        );
        if opened != ERROR_SUCCESS {
            return names;
        }
        for index in 0.. {
            let mut name = [0u16; 256];
            let mut name_length = name.len() as u32;
            // Kept as u16 so that the data is aligned for UTF-16.
            let mut data = [0u16; 128];
            let mut data_bytes = (data.len() * 2) as u32;
            let mut kind = 0u32;
            let status = RegEnumValueW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_length,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr().cast::<u8>()),
                Some(&mut data_bytes),
            );
            if status != ERROR_SUCCESS {
                break;
            }
            if kind == REG_SZ.0 {
                let units = (data_bytes as usize / 2).min(data.len());
                names.push(from_wide(&data[..units]));
            }
        }
        let _ = RegCloseKey(key);
    }
    names.sort_by_cached_key(|name| {
        let digits = name.bytes().rev().take_while(u8::is_ascii_digit).count();
        let (letters, number) = name.split_at(name.len() - digits);
        (
            letters.to_ascii_uppercase(),
            number.parse::<u64>().unwrap_or(u64::MAX),
        )
    });
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_that_does_not_exist_is_not_found() {
        let Err(error) = SerialPort::open("COM-rustmodem-no-such-port") else {
            panic!("there is no such port");
        };
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    #[test]
    fn the_ports_of_this_machine_can_be_listed() {
        // What this machine has is not known to the test; listing must
        // not fail, and the list is in order with nothing twice.
        let ports = serial_ports();
        let mut sorted = ports.clone();
        sorted.dedup();
        assert_eq!(ports, sorted);
        assert!(ports.iter().all(|name| !name.is_empty()));
    }
}
