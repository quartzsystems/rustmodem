// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What setup does, as plans.
//!
//! A plan is steps in order. Each step is one thing done to the machine,
//! written so that doing it twice is doing it once, and each step of an
//! install has beside it the step that undoes it, which is none where the
//! thing was already there: an upgrade does not remove the service it
//! found if it fails. [`carry_out`] does a plan through a [`System`], and
//! if a step fails it undoes, last first, what the steps before it did.
//!
//! Install, upgrade and repair are one plan, made from what is
//! [`Found`] on the machine: a new install finds nothing, an upgrade and
//! a repair find an install, and every step is one that brings the
//! machine to the installed state from wherever it is.

use std::path::PathBuf;

use msg::{Message, message};

use crate::files;

/// One thing setup does to the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Asks a running manager to close, and ends it if it does not.
    StopManager,
    /// Stops the service, and waits for it to have stopped.
    StopService,
    /// Starts the service.
    StartService,
    /// Gives the data directory the product's capitalisation. To Windows
    /// the two names are one directory.
    RenameDataDir { from: PathBuf, to: PathBuf },
    /// Makes a directory, and those above it.
    MakeDir(PathBuf),
    /// Puts a file of the payload in the install directory. A file that
    /// is there is kept beside it until the plan is done.
    InstallFile(String),
    /// Puts back the file that [`Step::InstallFile`] replaced, or takes
    /// away the one it added.
    RestoreFile(String),
    /// Removes the files kept by [`Step::InstallFile`].
    ForgetOldFiles,
    /// Removes a file of the install directory.
    RemoveFile(String),
    /// Removes a directory if it is empty.
    RemoveDir(PathBuf),
    /// Removes a directory and all that is in it.
    RemoveTree(PathBuf),
    /// Writes the configuration a new install starts with.
    WriteDefaultConfig,
    /// Removes the configuration file.
    RemoveConfig,
    /// Lets only the system, administrators and the service's account
    /// into the data directory.
    ProtectData,
    /// Trusts the lab certificate: the machine's Trusted Root and Trusted
    /// Publishers stores.
    TrustCertificate,
    /// Takes the lab certificate out of both stores.
    DistrustCertificate,
    /// Puts the driver package in the driver store.
    AddDriverPackage,
    /// Takes the driver package out of the driver store.
    RemoveDriverPackage,
    /// Registers the service, under its own account, or brings a
    /// registered one to what it should be.
    CreateService,
    /// Removes the service.
    DeleteService,
    /// Registers the event source, with the service as its message file.
    RegisterEventSource,
    /// Removes the event source.
    UnregisterEventSource,
    /// Lets SIP and RTP in to the service through Windows Firewall.
    AddFirewallRules,
    /// Removes those rules.
    RemoveFirewallRules,
    /// Has Windows mark the service's RTP as expedited forwarding.
    AddQosPolicy,
    /// Removes that policy.
    RemoveQosPolicy,
    /// Puts the manager in the Start menu.
    AddShortcuts,
    /// Takes it out.
    RemoveShortcuts,
    /// Lists RustModem among the installed programs.
    RegisterUninstall,
    /// Takes it off the list.
    UnregisterUninstall,
    /// Removes every RustModem port and its modem.
    RemovePorts,
}

/// What is on the machine before setup does anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The version of the install there is, if there is one.
    pub installed: Option<String>,
    /// The install directory is there.
    pub install_dir: bool,
    /// Which files of the install directory are there, by name.
    pub files: Vec<String>,
    /// The data directory as it is spelt on disk, if it is there.
    pub data_dir: Option<PathBuf>,
    /// There is a configuration file.
    pub config: bool,
    pub service: bool,
    pub service_running: bool,
    pub manager_running: bool,
    pub certificate_trusted: bool,
    pub driver_package: bool,
    pub event_source: bool,
    pub firewall_rules: bool,
    pub qos_policy: bool,
    pub shortcuts: bool,
    pub uninstall_entry: bool,
}

/// Where things go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// `%ProgramFiles%\Quartz Systems\RustModem`.
    pub install_dir: PathBuf,
    /// `%ProgramData%\Quartz Systems\RustModem`.
    pub data_dir: PathBuf,
}

impl Layout {
    /// The layout under the two directories Windows gives.
    pub fn under(program_files: &std::path::Path, program_data: &std::path::Path) -> Layout {
        let ours = |root: &std::path::Path| root.join(crate::PUBLISHER).join(crate::PRODUCT);
        Layout {
            install_dir: ours(program_files),
            data_dir: ours(program_data),
        }
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn config(&self) -> PathBuf {
        self.data_dir.join(files::CONFIG)
    }
}

/// A step of a plan, and the step that undoes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub step: Step,
    pub undo: Option<Step>,
}

fn planned(step: Step, undo: Option<Step>) -> Planned {
    Planned { step, undo }
}

/// The plan of an install, an upgrade or a repair: every step that brings
/// the machine from what was found to RustModem installed.
///
/// `names` are the files of the payload, and `lab` is whether the
/// payload has the lab certificate, which the person has agreed to trust.
pub fn install(found: &Found, layout: &Layout, names: &[String], lab: bool) -> Vec<Planned> {
    let mut plan = Vec::new();
    let new = |was_there: bool, undo: Step| (!was_there).then_some(undo);

    // Nothing of an install that is running is replaced under it.
    if found.manager_running {
        plan.push(planned(Step::StopManager, None));
    }
    if found.service_running {
        plan.push(planned(Step::StopService, Some(Step::StartService)));
    }

    // The data directory under its old name is the same directory.
    if let Some(as_found) = &found.data_dir
        && *as_found != layout.data_dir
    {
        plan.push(planned(
            Step::RenameDataDir {
                from: as_found.clone(),
                to: layout.data_dir.clone(),
            },
            Some(Step::RenameDataDir {
                from: layout.data_dir.clone(),
                to: as_found.clone(),
            }),
        ));
    }

    plan.push(planned(
        Step::MakeDir(layout.install_dir.clone()),
        new(
            found.install_dir,
            Step::RemoveDir(layout.install_dir.clone()),
        ),
    ));
    for name in names {
        // The default configuration is not a file of the install: it is
        // what WriteDefaultConfig writes from.
        if name == files::DEFAULT_CONFIG {
            continue;
        }
        plan.push(planned(
            Step::InstallFile(name.clone()),
            Some(Step::RestoreFile(name.clone())),
        ));
    }

    let data_there = found.data_dir.is_some();
    plan.push(planned(
        Step::MakeDir(layout.data_dir.clone()),
        new(data_there, Step::RemoveDir(layout.data_dir.clone())),
    ));
    plan.push(planned(
        Step::MakeDir(layout.logs_dir()),
        new(data_there, Step::RemoveDir(layout.logs_dir())),
    ));
    if !found.config {
        plan.push(planned(Step::WriteDefaultConfig, Some(Step::RemoveConfig)));
    }

    // The certificate is trusted before the driver store is touched:
    // Windows otherwise asks who the publisher is, on a desktop that a
    // silent install has not got.
    if lab {
        plan.push(planned(
            Step::TrustCertificate,
            new(found.certificate_trusted, Step::DistrustCertificate),
        ));
    }
    plan.push(planned(
        Step::AddDriverPackage,
        new(found.driver_package, Step::RemoveDriverPackage),
    ));

    plan.push(planned(
        Step::CreateService,
        new(found.service, Step::DeleteService),
    ));
    // The service's account is there once the service is.
    plan.push(planned(Step::ProtectData, None));
    plan.push(planned(
        Step::RegisterEventSource,
        new(found.event_source, Step::UnregisterEventSource),
    ));
    plan.push(planned(
        Step::AddFirewallRules,
        new(found.firewall_rules, Step::RemoveFirewallRules),
    ));
    plan.push(planned(
        Step::AddQosPolicy,
        new(found.qos_policy, Step::RemoveQosPolicy),
    ));
    plan.push(planned(
        Step::AddShortcuts,
        new(found.shortcuts, Step::RemoveShortcuts),
    ));
    plan.push(planned(
        Step::RegisterUninstall,
        new(found.uninstall_entry, Step::UnregisterUninstall),
    ));

    // Everything is in place: from here nothing is undone.
    plan.push(planned(Step::ForgetOldFiles, None));
    plan.push(planned(Step::StartService, None));
    plan
}

/// The plan of an uninstall. Configuration and logs stay unless `purge`.
/// Nothing of it is undone: a step that fails is reported, and the rest
/// are done all the same.
pub fn uninstall(found: &Found, layout: &Layout, lab: bool, purge: bool) -> Vec<Step> {
    let mut plan = vec![
        Step::StopManager,
        Step::StopService,
        // Ports first: a port is removed while its driver is there to
        // be told.
        Step::RemovePorts,
        Step::RemoveDriverPackage,
        Step::DeleteService,
        Step::UnregisterEventSource,
        Step::RemoveFirewallRules,
        Step::RemoveQosPolicy,
        Step::RemoveShortcuts,
        Step::UnregisterUninstall,
    ];
    if lab || found.certificate_trusted {
        plan.push(Step::DistrustCertificate);
    }
    for name in &found.files {
        plan.push(Step::RemoveFile(name.clone()));
    }
    // The directories the files were in, the deepest first, and then the
    // install directory itself.
    let mut directories: Vec<&str> = found
        .files
        .iter()
        .flat_map(|name| name.match_indices('\\').map(move |(at, _)| &name[..at]))
        .collect();
    directories.push(files::DRIVER_DIR);
    directories
        .sort_unstable_by_key(|directory| std::cmp::Reverse(directory.matches('\\').count()));
    directories.dedup();
    let mut seen = Vec::new();
    for directory in directories {
        if !seen.contains(&directory) {
            seen.push(directory);
            plan.push(Step::RemoveDir(layout.install_dir.join(directory)));
        }
    }
    plan.push(Step::RemoveDir(layout.install_dir.clone()));
    if purge {
        plan.push(Step::RemoveTree(layout.data_dir.clone()));
    }
    plan
}

/// The machine, as a plan is carried out on it.
pub trait System {
    /// Does one step. The error is what went wrong, in words for the log
    /// and the message.
    fn apply(&mut self, step: &Step) -> Result<(), String>;

    /// A line for the log of this run.
    fn log(&mut self, line: &str);
}

/// Carries out the plan of an install. If a step fails, what the steps
/// before it did is undone, last first, and the failure is the message.
pub fn carry_out(plan: &[Planned], system: &mut dyn System) -> Result<(), Message> {
    for (done, planned) in plan.iter().enumerate() {
        system.log(&format!("step: {:?}", planned.step));
        let Err(detail) = system.apply(&planned.step) else {
            continue;
        };
        system.log(&format!("failed: {detail}"));
        for earlier in plan[..done].iter().rev() {
            let Some(undo) = &earlier.undo else { continue };
            system.log(&format!("undo: {undo:?}"));
            if let Err(detail) = system.apply(undo) {
                // What cannot be undone is said, and the rest is undone.
                let warning = message("RMD9511", &[&format!("{undo:?}"), &detail]);
                system.log(&warning.to_string());
            }
        }
        return Err(message(
            "RMD9507",
            &[&format!("{:?}", planned.step), &detail],
        ));
    }
    Ok(())
}

/// Carries out the plan of an uninstall: every step, whatever the one
/// before it did. Gives what could not be done.
pub fn carry_out_all(plan: &[Step], system: &mut dyn System) -> Vec<Message> {
    let mut failures = Vec::new();
    for step in plan {
        system.log(&format!("step: {step:?}"));
        if let Err(detail) = system.apply(step) {
            system.log(&format!("failed: {detail}"));
            failures.push(message("RMD9513", &[&format!("{step:?}"), &detail]));
        }
    }
    failures
}
