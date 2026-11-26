// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The Windows service: running under the service control manager, and
//! registering and removing the service.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use msg::{Message, message};
use svc::shutdown;
use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::runner::RunMode;
use crate::{SERVICE_DISPLAY_NAME, SERVICE_NAME, eventlog, runner};

/// What the service says about itself in the Services console.
const DESCRIPTION: &str = "Software modem pool: answers calls with in-house softmodems and presents each line to \
     Windows as a modem on a COM port.";

/// How long the service control manager is told to wait for a start or a
/// stop before it decides the service has hung.
const WAIT_HINT: Duration = Duration::from_secs(30);

/// How long `--uninstall` waits for a running service to stop.
const UNINSTALL_STOP_WAIT: Duration = Duration::from_secs(30);

/// The configuration path the service was installed with, if any. The
/// service control manager calls `service_main` with the start parameters
/// of the service, not the arguments of the process, so the path is kept
/// here on the way from `main`.
static CONFIG: OnceLock<Option<PathBuf>> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

/// A `windows-service` error with the Windows error underneath it.
fn describe(error: &windows_service::Error) -> String {
    match std::error::Error::source(error) {
        Some(source) => format!("{error}: {source}"),
        None => error.to_string(),
    }
}

/// Hands the process to the service control manager. Returns when the
/// service has stopped.
pub fn run(config: Option<PathBuf>) -> Result<(), Vec<Message>> {
    let _ = CONFIG.set(config);
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|e| {
        let problem = message("RMD0008", &[&describe(&e)]);
        vec![problem]
    })
}

fn service_main(_start_parameters: Vec<OsString>) {
    // There is nobody to return an error to. What goes wrong is reported
    // to the Event Log where it happens.
    if let Err(error) = serve() {
        eventlog::report(&message("RMD0003", &[&describe(&error)]));
    }
}

fn serve() -> windows_service::Result<()> {
    // The runtime is built here, on the thread the service control
    // manager started the service on, which then waits on it.
    let runtime = svc::runtime::build().map_err(windows_service::Error::Winapi)?;
    let (handler_stop, mut stopping) = shutdown::channel();
    let status = service_control_handler::register(SERVICE_NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            handler_stop.trigger();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let report = |state: ServiceState, exit_code: ServiceExitCode| {
        let pending = matches!(
            state,
            ServiceState::StartPending | ServiceState::StopPending
        );
        status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: if state == ServiceState::Running {
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
            } else {
                ServiceControlAccept::empty()
            },
            exit_code,
            checkpoint: u32::from(pending),
            wait_hint: if pending { WAIT_HINT } else { Duration::ZERO },
            process_id: None,
        })
    };

    report(ServiceState::StartPending, ServiceExitCode::Win32(0))?;
    let config = CONFIG.get().cloned().flatten();
    match runner::start(config, RunMode::Service) {
        Ok(running) => {
            report(ServiceState::Running, ServiceExitCode::Win32(0))?;
            runtime.block_on(stopping.wait());
            report(ServiceState::StopPending, ServiceExitCode::Win32(0))?;
            running.stop();
            report(ServiceState::Stopped, ServiceExitCode::Win32(0))
        }
        // The reasons are already in the log and the Event Log.
        Err(_) => report(ServiceState::Stopped, ServiceExitCode::ServiceSpecific(1)),
    }
}

/// Registers the service: automatic start, running as LocalSystem.
pub fn install(config: Option<PathBuf>) -> Result<(), Vec<Message>> {
    let refused = |detail: String| vec![message("RMD0006", &[SERVICE_NAME, &detail])];
    let executable = std::env::current_exe().map_err(|e| refused(e.to_string()))?;
    // The service starts in the system directory, so a relative path given
    // now would mean something else then.
    let launch_arguments = match config {
        Some(path) => {
            let path = std::path::absolute(&path).map_err(|e| refused(e.to_string()))?;
            vec![OsString::from("--service"), path.into_os_string()]
        }
        None => Vec::new(),
    };
    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: executable,
        launch_arguments,
        dependencies: Vec::new(),
        // BLOCKED(phase5): a dedicated service account. In phase 0 the
        // service runs as LocalSystem, which is what no account name means.
        account_name: None,
        account_password: None,
    };
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| refused(describe(&e)))?;
    let service = manager
        .create_service(&info, ServiceAccess::CHANGE_CONFIG)
        .map_err(|e| refused(describe(&e)))?;
    // The description is a nicety; the service is installed without it.
    let _ = service.set_description(DESCRIPTION);

    let installed = message("RMD0004", &[SERVICE_NAME]);
    println!("{installed}");
    println!("{}", installed.recovery);
    Ok(())
}

/// Removes the service, stopping it first if it is running.
pub fn uninstall() -> Result<(), Vec<Message>> {
    let refused =
        |e: windows_service::Error| vec![message("RMD0007", &[SERVICE_NAME, &describe(&e)])];
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(refused)?;
    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
        )
        .map_err(refused)?;

    if service.query_status().map_err(refused)?.current_state != ServiceState::Stopped {
        // It may be stopping already, in which case this fails and the wait
        // below still sees it stop.
        let _ = service.stop();
        let deadline = Instant::now() + UNINSTALL_STOP_WAIT;
        while service.query_status().map_err(refused)?.current_state != ServiceState::Stopped
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    service.delete().map_err(refused)?;
    println!("{}", message("RMD0005", &[SERVICE_NAME]));
    Ok(())
}
