// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The steps of a plan, and the device layer, as they are done on
//! Windows.
//!
//! Three ways are used, in this order of preference: Rust's own library,
//! for files; Windows' own command-line tools, for what they do well and
//! every administrator can check by hand (`certutil`, `pnputil`, `netsh`,
//! `icacls`, `taskkill`, the `NetQos` cmdlets); and the wrappers of the
//! `sys` crate, for what has no tool (devices, the registry, shortcuts).
//! Every tool is run with a time limit: a tool that asks a question on a
//! desktop nobody sees would otherwise wait for ever.
//!
//! Each step is written to be done twice as once: what is there is left,
//! what is to be added is replaced.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sys::devices::{self, Class};
use sys::registry::{self, MachineKey};
use windows_service::service::{
    ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceState, ServiceType,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::payload::Entry;
use crate::plan::{Found, Layout, Step, System};
use crate::ports::{Device, Devices};
use crate::{
    LAB_CERTIFICATE, PORT_HARDWARE_ID, PRODUCT, PUBLISHER, SERVICE_ACCOUNT, SERVICE_NAME, VERSION,
    files,
};

/// Where Windows lists an installed program.
const UNINSTALL_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\RustModem";
/// Where an event source of the Application log is registered.
const EVENT_SOURCE_KEY: &str =
    "SYSTEM\\CurrentControlSet\\Services\\EventLog\\Application\\RustModem";
/// The names of the firewall rules and of the QoS policy.
const RULE_SIP: &str = "RustModem SIP";
const RULE_RTP: &str = "RustModem RTP";
const QOS_POLICY: &str = "RustModem RTP";
/// What the service is called in the Services console, and under it.
// Windows drops a display name that is the service's name in other
// capitals, and shows the name instead: so it is more than the name.
const SERVICE_DISPLAY: &str = "RustModem Modem Pool";
const SERVICE_DESCRIPTION: &str = "The RustModem modem pool: answers and places modem calls over SIP and presents each line to Windows as a modem on a COM port.";
/// The Start menu folder and the shortcut in it.
const MENU_FOLDER: &str = "RustModem";
const MENU_MANAGER: &str = "RustModem Manager.lnk";

/// The least build of Windows RustModem runs on: Windows 10 version 1803,
/// which is the first with the driver store directory the driver's INF
/// installs into.
pub const LEAST_BUILD: u32 = 17134;

/// How long a tool is given.
const TOOL_LIMIT: Duration = Duration::from_secs(120);

/// Runs a tool and gives what it printed, if it ended with success.
fn run(program: &str, arguments: &[&str]) -> Result<String, String> {
    let (code, output) = run_code(program, arguments)?;
    if code == 0 {
        Ok(output)
    } else {
        Err(format!("{program} ended with {code}: {}", output.trim()))
    }
}

/// Runs a tool, with a time limit, and gives its exit code and what it
/// printed.
fn run_code(program: &str, arguments: &[&str]) -> Result<(i32, String), String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program} could not be run: {e}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > TOOL_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} did not end in {} seconds and was stopped: it may have been waiting for an answer",
                    TOOL_LIMIT.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(format!("{program}: {e}")),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok((output.status.code().unwrap_or(-1), text))
}

fn io_error(what: &str, error: std::io::Error) -> String {
    format!("{what}: {error}")
}

fn service_error(error: windows_service::Error) -> String {
    match error {
        windows_service::Error::Winapi(inner) => inner.to_string(),
        other => other.to_string(),
    }
}

/// The directories Windows gives: Program Files and ProgramData.
pub fn layout() -> Layout {
    let directory = |name: &str, otherwise: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(otherwise))
    };
    Layout::under(
        &directory("ProgramFiles", "C:\\Program Files"),
        &directory("ProgramData", "C:\\ProgramData"),
    )
}

/// The build number of this Windows.
pub fn windows_build() -> u32 {
    MachineKey::open("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
        .and_then(|key| key.string("CurrentBuildNumber"))
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// The published names, `oem12.inf`, of the driver package's two INFs in
/// the driver store. They are found by what is in them, which is the
/// same in every language Windows speaks.
fn published_infs() -> Vec<String> {
    let directory = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
        .join("INF");
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if !name.starts_with("oem") || !name.ends_with(".inf") {
            continue;
        }
        let Ok(octets) = fs::read(entry.path()) else {
            continue;
        };
        // An INF is ASCII or UTF-16; either way its text is found among
        // its octets once the zeroes are taken out.
        let text: String = octets
            .iter()
            .filter(|&&octet| octet != 0)
            .map(|&octet| octet as char)
            .collect();
        if text.contains(PORT_HARDWARE_ID) || text.contains(modeminf::HARDWARE_ID) {
            found.push(name);
        }
    }
    found
}

/// The machine setup is run on.
pub struct Machine {
    pub layout: Layout,
    /// The files of this setup program's payload. Empty if it has none.
    pub payload: Vec<Entry>,
    /// This setup program's own file, which is kept in the install
    /// directory to repair and to remove with.
    own: Vec<u8>,
    log: Option<fs::File>,
}

impl Machine {
    /// The machine, with this run's log written to `log`.
    pub fn new(payload: Vec<Entry>, own: Vec<u8>, log: Option<&Path>) -> Machine {
        let log = log.and_then(|path| {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        });
        Machine {
            layout: layout(),
            payload,
            own,
            log,
        }
    }

    fn installed_path(&self, name: &str) -> PathBuf {
        self.layout.install_dir.join(name)
    }

    fn entry(&self, name: &str) -> Result<&Entry, String> {
        self.payload
            .iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| format!("{name} is not in this setup program"))
    }

    /// What is on the machine now.
    pub fn find(&mut self) -> Found {
        let installed =
            MachineKey::open(UNINSTALL_KEY).and_then(|key| key.string("DisplayVersion"));
        let install_dir = self.layout.install_dir.is_dir();
        let mut files = Vec::new();
        list_files(&self.layout.install_dir, "", &mut files);
        // The data directory as it is spelt, which may be the old way.
        let data_dir = self.layout.data_dir.parent().and_then(|parent| {
            fs::read_dir(parent).ok()?.flatten().find_map(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(PRODUCT)
                    .then(|| entry.path())
            })
        });
        let config = data_dir
            .as_ref()
            .is_some_and(|directory| directory.join(files::CONFIG).is_file());
        let state = service_state();
        Found {
            installed,
            install_dir,
            files,
            data_dir,
            config,
            service: state.is_some(),
            service_running: state.is_some_and(|state| state != ServiceState::Stopped),
            manager_running: run(
                "tasklist",
                &["/FI", "IMAGENAME eq rustmodem-manager.exe", "/NH"],
            )
            .is_ok_and(|output| output.to_ascii_lowercase().contains(files::MANAGER)),
            certificate_trusted: run_code(
                "certutil",
                &["-verifystore", "TrustedPublisher", LAB_CERTIFICATE],
            )
            .is_ok_and(|(code, _)| code == 0),
            driver_package: !published_infs().is_empty(),
            event_source: MachineKey::open(EVENT_SOURCE_KEY).is_some(),
            firewall_rules: run_code(
                "netsh",
                &[
                    "advfirewall",
                    "firewall",
                    "show",
                    "rule",
                    &format!("name={RULE_SIP}"),
                ],
            )
            .is_ok_and(|(code, _)| code == 0),
            qos_policy: MachineKey::open(&format!(
                "SOFTWARE\\Policies\\Microsoft\\Windows\\QoS\\{QOS_POLICY}"
            ))
            .is_some(),
            shortcuts: sys::shortcut::common_programs()
                .is_ok_and(|menu| menu.join(MENU_FOLDER).join(MENU_MANAGER).is_file()),
            uninstall_entry: MachineKey::open(UNINSTALL_KEY).is_some(),
        }
    }

    /// The SIP port and the RTP range of the configuration there is, or
    /// of a new one.
    fn telephony_ports(&self) -> (u16, u16, u16) {
        let parsed = fs::read_to_string(self.layout.config())
            .ok()
            .and_then(|text| config::ConfigFile::parse(&text).ok());
        let defaults = config::Config::default();
        let config = parsed.as_ref().map_or(&defaults, |file| file.config());
        (
            config.sip.listen.port(),
            config.media.rtp_ports.first,
            config.media.rtp_ports.last,
        )
    }

    fn install_file(&self, name: &str) -> Result<(), String> {
        // The one file the payload cannot carry is the program that
        // carries it.
        let data = if name == files::SETUP {
            &self.own
        } else {
            &self.entry(name)?.data
        };
        let to = self.installed_path(name);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(name, e))?;
        }
        let new = with_suffix(&to, ".new");
        let old = with_suffix(&to, ".old");
        fs::write(&new, data).map_err(|e| io_error(name, e))?;
        if to.exists() {
            // Kept until the plan is done, so that it can be put back. A
            // program that is running can be renamed where it cannot be
            // written over.
            remove_file(&old)?;
            fs::rename(&to, &old).map_err(|e| io_error(name, e))?;
        }
        fs::rename(&new, &to).map_err(|e| io_error(name, e))
    }

    fn restore_file(&self, name: &str) -> Result<(), String> {
        let to = self.installed_path(name);
        let old = with_suffix(&to, ".old");
        let _ = fs::remove_file(with_suffix(&to, ".new"));
        if old.exists() {
            let _ = fs::remove_file(&to);
            fs::rename(&old, &to).map_err(|e| io_error(name, e))
        } else {
            remove_file(&to)
        }
    }

    fn create_service(&self) -> Result<(), String> {
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: self.installed_path(files::SERVICE),
            launch_arguments: Vec::new(),
            dependencies: Vec::new(),
            // The service's own account. It has no password: Windows
            // keeps it.
            account_name: Some(OsString::from(SERVICE_ACCOUNT)),
            account_password: None,
        };
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .map_err(service_error)?;
        let service = match manager.open_service(SERVICE_NAME, ServiceAccess::CHANGE_CONFIG) {
            Ok(service) => {
                service.change_config(&info).map_err(service_error)?;
                service
            }
            Err(_) => manager
                .create_service(&info, ServiceAccess::CHANGE_CONFIG)
                .map_err(service_error)?,
        };
        let _ = service.set_description(SERVICE_DESCRIPTION);
        Ok(())
    }

    fn firewall(&self, add: bool) -> Result<(), String> {
        // Taken away first, so that adding twice is adding once, and so
        // that a rule follows a configuration whose ports have changed.
        for rule in [RULE_SIP, RULE_RTP] {
            let _ = run_code(
                "netsh",
                &[
                    "advfirewall",
                    "firewall",
                    "delete",
                    "rule",
                    &format!("name={rule}"),
                ],
            );
        }
        if !add {
            return Ok(());
        }
        let (sip, first, last) = self.telephony_ports();
        let program = format!("program={}", self.installed_path(files::SERVICE).display());
        for (rule, ports) in [
            (RULE_SIP, sip.to_string()),
            (RULE_RTP, format!("{first}-{last}")),
        ] {
            run(
                "netsh",
                &[
                    "advfirewall",
                    "firewall",
                    "add",
                    "rule",
                    &format!("name={rule}"),
                    "dir=in",
                    "action=allow",
                    "protocol=UDP",
                    &format!("localport={ports}"),
                    &program,
                ],
            )?;
        }
        Ok(())
    }

    fn qos(&self, add: bool) -> Result<(), String> {
        // Not there is what was wanted: the removal ends well either way.
        let remove = format!(
            "Remove-NetQosPolicy -Name '{QOS_POLICY}' -Confirm:$false -ErrorAction SilentlyContinue; $global:LASTEXITCODE = 0"
        );
        // DSCP 46, expedited forwarding, on what the service sends by
        // UDP, on every kind of network: a server's is not always a
        // domain's.
        let script = if add {
            format!(
                "{remove}; New-NetQosPolicy -Name '{QOS_POLICY}' -AppPathNameMatchCondition '{}' -IPProtocolMatchCondition UDP -DSCPAction 46 -NetworkProfile All -ErrorAction Stop | Out-Null",
                files::SERVICE
            )
        } else {
            format!("{remove}; exit 0")
        };
        run(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ],
        )
        .map(|_| ())
    }

    fn register_uninstall(&self) -> Result<(), String> {
        let setup = self.installed_path(files::SETUP);
        let size: usize = self.payload.iter().map(|entry| entry.data.len()).sum();
        let write = || -> std::io::Result<()> {
            let key = MachineKey::create(UNINSTALL_KEY)?;
            key.set_string("DisplayName", PRODUCT)?;
            key.set_string("DisplayVersion", VERSION)?;
            key.set_string("Publisher", PUBLISHER)?;
            key.set_string(
                "InstallLocation",
                &self.layout.install_dir.display().to_string(),
            )?;
            key.set_string(
                "DisplayIcon",
                &self.installed_path(files::MANAGER).display().to_string(),
            )?;
            key.set_string(
                "UninstallString",
                &format!("\"{}\" uninstall", setup.display()),
            )?;
            key.set_string(
                "QuietUninstallString",
                &format!("\"{}\" uninstall /quiet", setup.display()),
            )?;
            key.set_string("ModifyPath", &format!("\"{}\" repair", setup.display()))?;
            key.set_number("EstimatedSize", (size / 1024) as u32)?;
            key.set_number("NoModify", 0)?;
            key.set_number("NoRepair", 0)
        };
        write().map_err(|e| io_error("the list of installed programs", e))
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Has Windows remove this program's own file, and the install directory
/// if that leaves it empty, once the program has ended: a program cannot
/// remove itself while it runs.
pub fn remove_after_exit(own: &Path, install_dir: &Path) {
    let script = format!(
        "ping -n 4 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"",
        own.display(),
        install_dir.display()
    );
    // Given to cmd as it is written: quoted as an argument of a program
    // is quoted, cmd would not know its own quotes again. With /s it
    // takes off the first and the last and reads what is between.
    let _ = Command::new("cmd")
        .raw_arg(format!("/d /s /c \"{script}\""))
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// The files under `directory`, by their names relative to it.
fn list_files(directory: &Path, prefix: &str, found: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = format!("{prefix}{}", entry.file_name().to_string_lossy());
        if entry.path().is_dir() {
            list_files(&entry.path(), &format!("{name}\\"), found);
        } else {
            found.push(name);
        }
    }
}

/// Removes a file. One that cannot be removed because a program has it
/// loaded is moved out of the way instead, to the temporary directory,
/// which a file can be while it is loaded: the Event Log keeps the
/// service's program loaded, as its message file, for as long as it
/// likes.
fn remove_file(path: &Path) -> Result<(), String> {
    let removed = match fs::remove_file(path) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => e,
    };
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let aside = std::env::temp_dir().join(format!("rustmodem-{stamp}-{name}"));
    match fs::rename(path, &aside) {
        Ok(()) => Ok(()),
        Err(_) => Err(io_error(&path.display().to_string(), removed)),
    }
}

fn service_state() -> Option<ServiceState> {
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
        .ok()?;
    Some(service.query_status().ok()?.current_state)
}

fn open_service(
    access: ServiceAccess,
) -> Result<Option<windows_service::service::Service>, String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(service_error)?;
    Ok(manager.open_service(SERVICE_NAME, access).ok())
}

fn stop_service() -> Result<(), String> {
    let Some(service) = open_service(ServiceAccess::QUERY_STATUS | ServiceAccess::STOP)? else {
        return Ok(());
    };
    let stopped = || {
        service
            .query_status()
            .is_ok_and(|status| status.current_state == ServiceState::Stopped)
    };
    if stopped() {
        return Ok(());
    }
    // It may be stopping already, in which case this fails and the wait
    // still sees it stop.
    let _ = service.stop();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !stopped() {
        if Instant::now() > deadline {
            return Err("the service did not stop in a minute".to_string());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

impl System for Machine {
    fn apply(&mut self, step: &Step) -> Result<(), String> {
        match step {
            Step::StopManager => {
                // Asked first, as closing its window asks; then ended.
                let _ = run_code("taskkill", &["/IM", files::MANAGER]);
                std::thread::sleep(Duration::from_secs(2));
                let _ = run_code("taskkill", &["/F", "/IM", files::MANAGER]);
                Ok(())
            }
            Step::StopService => stop_service(),
            Step::StartService => {
                let Some(service) = open_service(ServiceAccess::START)? else {
                    return Err("the service is not registered".to_string());
                };
                match service.start::<&str>(&[]) {
                    Ok(()) => Ok(()),
                    // ERROR_SERVICE_ALREADY_RUNNING.
                    Err(windows_service::Error::Winapi(e)) if e.raw_os_error() == Some(1056) => {
                        Ok(())
                    }
                    Err(e) => Err(service_error(e)),
                }
            }
            Step::RenameDataDir { from, to } => {
                fs::rename(from, to).map_err(|e| io_error("the data directory", e))
            }
            Step::MakeDir(directory) => fs::create_dir_all(directory)
                .map_err(|e| io_error(&directory.display().to_string(), e)),
            Step::InstallFile(name) => self.install_file(name),
            Step::RestoreFile(name) => self.restore_file(name),
            Step::ForgetOldFiles => {
                let mut all = Vec::new();
                list_files(&self.layout.install_dir, "", &mut all);
                for name in all.iter().filter(|name| name.ends_with(".old")) {
                    let _ = remove_file(&self.installed_path(name));
                }
                Ok(())
            }
            Step::RemoveFile(name) => remove_file(&self.installed_path(name)),
            Step::RemoveDir(directory) => match fs::remove_dir(directory) {
                Ok(()) => Ok(()),
                // Not there, or not empty: what is in it is not setup's
                // to take.
                Err(_) => Ok(()),
            },
            Step::RemoveTree(directory) => match fs::remove_dir_all(directory) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_error(&directory.display().to_string(), e)),
            },
            Step::WriteDefaultConfig => {
                let entry = self.entry(files::DEFAULT_CONFIG)?;
                fs::write(self.layout.config(), &entry.data)
                    .map_err(|e| io_error("the configuration", e))
            }
            Step::RemoveConfig => remove_file(&self.layout.config()),
            Step::ProtectData => {
                let directory = self.layout.data_dir.display().to_string();
                // The system and administrators by their identifiers,
                // which are the same in every language; the service's
                // account by its name, which is too. The service may
                // change what is there and may not change who may.
                run(
                    "icacls",
                    &[
                        &directory,
                        "/inheritance:r",
                        "/grant:r",
                        "*S-1-5-18:(OI)(CI)F",
                        "*S-1-5-32-544:(OI)(CI)F",
                        &format!("{SERVICE_ACCOUNT}:(OI)(CI)M"),
                    ],
                )
                .map(|_| ())
            }
            Step::TrustCertificate => {
                let entry = self.entry(files::LAB_CERTIFICATE)?;
                let file = std::env::temp_dir().join("rustmodem-lab.cer");
                fs::write(&file, &entry.data).map_err(|e| io_error("the certificate", e))?;
                let path = file.display().to_string();
                let added = run("certutil", &["-addstore", "-f", "Root", &path])
                    .and_then(|_| run("certutil", &["-addstore", "-f", "TrustedPublisher", &path]));
                let _ = fs::remove_file(&file);
                added.map(|_| ())
            }
            Step::DistrustCertificate => {
                for store in ["TrustedPublisher", "Root"] {
                    // Not there is what was wanted.
                    let _ = run_code("certutil", &["-delstore", store, LAB_CERTIFICATE]);
                }
                Ok(())
            }
            Step::AddDriverPackage => {
                for inf in [files::PORT_INF, files::MODEM_INF] {
                    let path = self.installed_path(inf).display().to_string();
                    run("pnputil", &["/add-driver", &path])?;
                }
                Ok(())
            }
            Step::RemoveDriverPackage => {
                for published in published_infs() {
                    run(
                        "pnputil",
                        &["/delete-driver", &published, "/uninstall", "/force"],
                    )?;
                }
                Ok(())
            }
            Step::CreateService => self.create_service(),
            Step::DeleteService => {
                stop_service()?;
                match open_service(ServiceAccess::DELETE)? {
                    Some(service) => service.delete().map_err(service_error),
                    None => Ok(()),
                }
            }
            Step::RegisterEventSource => (|| -> std::io::Result<()> {
                let key = MachineKey::create(EVENT_SOURCE_KEY)?;
                key.set_expandable(
                    "EventMessageFile",
                    &self.installed_path(files::SERVICE).display().to_string(),
                )?;
                // Errors, warnings and information.
                key.set_number("TypesSupported", 7)
            })()
            .map_err(|e| io_error("the event source", e)),
            Step::UnregisterEventSource => {
                registry::delete(EVENT_SOURCE_KEY).map_err(|e| io_error("the event source", e))
            }
            Step::AddFirewallRules => self.firewall(true),
            Step::RemoveFirewallRules => self.firewall(false),
            Step::AddQosPolicy => self.qos(true),
            Step::RemoveQosPolicy => self.qos(false),
            Step::AddShortcuts => {
                let menu = sys::shortcut::common_programs()
                    .map_err(|e| io_error("the Start menu", e))?
                    .join(MENU_FOLDER);
                fs::create_dir_all(&menu).map_err(|e| io_error("the Start menu", e))?;
                sys::shortcut::create(
                    &menu.join(MENU_MANAGER),
                    &self.installed_path(files::MANAGER),
                    "",
                    "Manage the RustModem modem pool",
                )
                .map_err(|e| io_error("the Start menu", e))
            }
            Step::RemoveShortcuts => {
                if let Ok(menu) = sys::shortcut::common_programs() {
                    let folder = menu.join(MENU_FOLDER);
                    let _ = fs::remove_file(folder.join(MENU_MANAGER));
                    let _ = fs::remove_dir(folder);
                }
                Ok(())
            }
            Step::RegisterUninstall => self.register_uninstall(),
            Step::UnregisterUninstall => registry::delete(UNINSTALL_KEY)
                .map_err(|e| io_error("the list of installed programs", e)),
            Step::RemovePorts => crate::ports::remove_all(&mut SetupApi),
        }
    }

    fn log(&mut self, line: &str) {
        if let Some(log) = &mut self.log {
            let _ = writeln!(log, "{line}");
        }
    }
}

/// The device layer of Windows.
pub struct SetupApi;

fn device(device: devices::Device) -> Device {
    Device {
        instance: device.instance,
        name: device.name,
        com: device.com,
        driver_version: device.driver_version,
        started: device.started,
    }
}

impl Devices for SetupApi {
    fn driver_installed(&mut self) -> bool {
        !published_infs().is_empty()
    }

    fn ports(&mut self) -> Result<Vec<Device>, String> {
        devices::list(Class::Ports, PORT_HARDWARE_ID)
            .map(|all| all.into_iter().map(device).collect())
            .map_err(|e| e.to_string())
    }

    fn modems(&mut self) -> Result<Vec<Device>, String> {
        devices::list(Class::Modem, modeminf::HARDWARE_ID)
            .map(|all| all.into_iter().map(device).collect())
            .map_err(|e| e.to_string())
    }

    fn taken(&mut self) -> Result<Vec<u32>, String> {
        sys::comdb::taken().map_err(|e| e.to_string())
    }

    fn claim(&mut self, number: u32) -> Result<bool, String> {
        sys::comdb::claim(number).map_err(|e| e.to_string())
    }

    fn release(&mut self, number: u32) -> Result<(), String> {
        sys::comdb::release(number).map_err(|e| e.to_string())
    }

    fn create_port(&mut self, com: &str) -> Result<String, String> {
        devices::create(Class::Ports, PORT_HARDWARE_ID, com).map_err(|e| e.to_string())
    }

    fn create_modem(&mut self, com: &str) -> Result<String, String> {
        devices::create(Class::Modem, modeminf::HARDWARE_ID, com).map_err(|e| e.to_string())
    }

    fn remove(&mut self, instance: &str) -> Result<(), String> {
        devices::remove(instance).map_err(|e| e.to_string())
    }
}
