// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Starting and stopping a Windows service through the service control
//! manager.

use std::io;
use std::time::{Duration, Instant};

use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
    SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_CONTROL_STOP, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
    SERVICE_START, SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STOP, SERVICE_STOPPED,
    StartServiceW,
};
use windows::core::PCWSTR;

use crate::util::{pcwstr, wide};

/// How long to wait for a service to reach the state it was asked for.
const WAIT: Duration = Duration::from_secs(30);

/// What to do to a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceAction {
    /// Start it.
    Start,
    /// Stop it.
    Stop,
    /// Stop it and start it again.
    Restart,
}

/// A handle of the service control manager, closed when dropped.
struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this module and is closed
        // nowhere else.
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

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

fn open(name: &str, access: u32) -> io::Result<(Handle, Handle)> {
    let name = wide(name);
    // SAFETY: the name is terminated UTF-16 that outlives the calls; null
    // names mean this machine and its active database.
    unsafe {
        let manager = Handle(
            OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT).map_err(io_error)?,
        );
        let service = Handle(OpenServiceW(manager.0, pcwstr(&name), access).map_err(io_error)?);
        Ok((manager, service))
    }
}

fn state(service: &Handle) -> io::Result<SERVICE_STATUS_CURRENT_STATE> {
    let mut status = SERVICE_STATUS::default();
    // SAFETY: the handle is an open service and `status` is valid for the
    // call to fill.
    unsafe { QueryServiceStatus(service.0, &mut status) }.map_err(io_error)?;
    Ok(status.dwCurrentState)
}

fn wait_for(service: &Handle, wanted: SERVICE_STATUS_CURRENT_STATE) -> io::Result<()> {
    let deadline = Instant::now() + WAIT;
    while state(service)? != wanted {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the service did not reach the state asked for in time",
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Whether a service is running. An error if there is no such service, or
/// it may not be asked about.
pub fn service_state(name: &str) -> io::Result<bool> {
    let (_manager, service) = open(name, SERVICE_QUERY_STATUS)?;
    Ok(state(&service)? == SERVICE_RUNNING)
}

/// Starts, stops or restarts a service and waits until it has. Starting a
/// service that is running, and stopping one that is stopped, succeed.
/// Without administrator rights the error is "permission denied".
pub fn service_control(name: &str, action: ServiceAction) -> io::Result<()> {
    let (_manager, service) = open(name, SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS)?;
    if matches!(action, ServiceAction::Stop | ServiceAction::Restart)
        && state(&service)? != SERVICE_STOPPED
    {
        let mut status = SERVICE_STATUS::default();
        // SAFETY: the handle is an open service and `status` is valid for
        // the call to fill.
        unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) }
            .map_err(io_error)?;
        wait_for(&service, SERVICE_STOPPED)?;
    }
    if matches!(action, ServiceAction::Start | ServiceAction::Restart)
        && state(&service)? != SERVICE_RUNNING
    {
        // SAFETY: the handle is an open service; no arguments are given.
        unsafe { StartServiceW(service.0, None) }.map_err(io_error)?;
        wait_for(&service, SERVICE_RUNNING)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_that_does_not_exist_is_an_error() {
        let name = "rustmodem-no-such-service-for-the-test";
        // "The specified service does not exist as an installed service."
        const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
        let error = service_state(name).unwrap_err();
        assert_eq!(
            error.raw_os_error(),
            Some(ERROR_SERVICE_DOES_NOT_EXIST),
            "{error}"
        );
        // Without administrator rights the service cannot even be opened
        // to be started, which is refused before it is found missing or
        // after; either way it is an error and not a wait.
        let started = Instant::now();
        assert!(service_control(name, ServiceAction::Start).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_service_every_windows_has_can_be_asked_about() {
        // The event log service is always there and always running.
        assert_eq!(service_state("EventLog").ok(), Some(true));
    }
}
