// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The plans: what setup would do for an install, an upgrade, a repair
//! and an uninstall, in order and with the undo of each step, and what is
//! undone when a step fails. The machine is a list that writes down what
//! was done to it.

use std::path::{Path, PathBuf};

use setup::plan::{
    Found, Layout, Planned, Step, System, carry_out, carry_out_all, install, uninstall,
};

fn layout() -> Layout {
    Layout::under(Path::new("C:\\Program Files"), Path::new("C:\\ProgramData"))
}

fn names() -> Vec<String> {
    [
        "rustmodem-svc.exe",
        "rustmodem-manager.exe",
        "rustmodem-ctl.exe",
        "rustmodem-setup.exe",
        "driver\\rmvcom.dll",
        "driver\\rmvcom.inf",
        "driver\\rmmodem.inf",
        "driver\\rmvcom.cat",
        "rustmodem.default.toml",
    ]
    .map(String::from)
    .to_vec()
}

/// A machine with RustModem installed and running.
fn installed() -> Found {
    Found {
        installed: Some("0.1.0".to_string()),
        install_dir: true,
        files: names(),
        data_dir: Some(layout().data_dir),
        config: true,
        service: true,
        service_running: true,
        manager_running: false,
        certificate_trusted: true,
        driver_package: true,
        event_source: true,
        firewall_rules: true,
        qos_policy: true,
        shortcuts: true,
        uninstall_entry: true,
    }
}

fn steps(plan: &[Planned]) -> Vec<Step> {
    plan.iter().map(|planned| planned.step.clone()).collect()
}

fn file(name: &str) -> Step {
    Step::InstallFile(name.to_string())
}

/// A machine that writes down what is done to it, and fails where told.
#[derive(Default)]
struct Machine {
    done: Vec<Step>,
    fail_at: Option<Step>,
    /// Undo steps that fail.
    stubborn: Vec<Step>,
    log: Vec<String>,
}

impl System for Machine {
    fn apply(&mut self, step: &Step) -> Result<(), String> {
        if self.fail_at.as_ref() == Some(step) || self.stubborn.contains(step) {
            return Err("the disk is full".to_string());
        }
        self.done.push(step.clone());
        Ok(())
    }

    fn log(&mut self, line: &str) {
        self.log.push(line.to_string());
    }
}

#[test]
fn a_new_install_does_every_step_in_order_and_each_has_its_undo() {
    let layout = layout();
    let plan = install(&Found::default(), &layout, &names(), true);
    let install_dir = layout.install_dir.clone();
    let data_dir = layout.data_dir.clone();
    let expected = [
        (
            Step::MakeDir(install_dir.clone()),
            Some(Step::RemoveDir(install_dir)),
        ),
        (
            file("rustmodem-svc.exe"),
            Some(Step::RestoreFile("rustmodem-svc.exe".into())),
        ),
        (
            file("rustmodem-manager.exe"),
            Some(Step::RestoreFile("rustmodem-manager.exe".into())),
        ),
        (
            file("rustmodem-ctl.exe"),
            Some(Step::RestoreFile("rustmodem-ctl.exe".into())),
        ),
        (
            file("rustmodem-setup.exe"),
            Some(Step::RestoreFile("rustmodem-setup.exe".into())),
        ),
        (
            file("driver\\rmvcom.dll"),
            Some(Step::RestoreFile("driver\\rmvcom.dll".into())),
        ),
        (
            file("driver\\rmvcom.inf"),
            Some(Step::RestoreFile("driver\\rmvcom.inf".into())),
        ),
        (
            file("driver\\rmmodem.inf"),
            Some(Step::RestoreFile("driver\\rmmodem.inf".into())),
        ),
        (
            file("driver\\rmvcom.cat"),
            Some(Step::RestoreFile("driver\\rmvcom.cat".into())),
        ),
        (
            Step::MakeDir(data_dir.clone()),
            Some(Step::RemoveDir(data_dir.clone())),
        ),
        (
            Step::MakeDir(data_dir.join("logs")),
            Some(Step::RemoveDir(data_dir.join("logs"))),
        ),
        (Step::WriteDefaultConfig, Some(Step::RemoveConfig)),
        (Step::TrustCertificate, Some(Step::DistrustCertificate)),
        (Step::AddDriverPackage, Some(Step::RemoveDriverPackage)),
        (Step::CreateService, Some(Step::DeleteService)),
        (Step::ProtectData, None),
        (Step::RegisterEventSource, Some(Step::UnregisterEventSource)),
        (Step::AddFirewallRules, Some(Step::RemoveFirewallRules)),
        (Step::AddQosPolicy, Some(Step::RemoveQosPolicy)),
        (Step::AddShortcuts, Some(Step::RemoveShortcuts)),
        (Step::RegisterUninstall, Some(Step::UnregisterUninstall)),
        (Step::ForgetOldFiles, None),
        (Step::StartService, None),
    ];
    let expected: Vec<Planned> = expected
        .into_iter()
        .map(|(step, undo)| Planned { step, undo })
        .collect();
    assert_eq!(plan, expected);
    // Under the publisher's name and the product's, as a person reads
    // them.
    let ours = |root: &str| Path::new(root).join("Quartz Systems").join("RustModem");
    assert_eq!(layout.install_dir, ours("C:\\Program Files"));
    assert_eq!(layout.data_dir, ours("C:\\ProgramData"));
}

#[test]
fn the_certificate_is_trusted_before_the_driver_store_is_touched() {
    let plan = steps(&install(&Found::default(), &layout(), &names(), true));
    let at = |step: &Step| plan.iter().position(|s| s == step).unwrap();
    assert!(at(&Step::TrustCertificate) < at(&Step::AddDriverPackage));
    // The service's account is there once the service is.
    assert!(at(&Step::CreateService) < at(&Step::ProtectData));
    // And a build signed for distribution has no certificate step at all.
    let plan = steps(&install(&Found::default(), &layout(), &names(), false));
    assert!(!plan.contains(&Step::TrustCertificate));
    assert!(plan.contains(&Step::AddDriverPackage));
}

#[test]
fn an_upgrade_stops_what_runs_keeps_what_is_there_and_undoes_only_what_it_added() {
    let layout = layout();
    let mut found = installed();
    found.manager_running = true;
    let plan = install(&found, &layout, &names(), true);
    let all = steps(&plan);
    assert_eq!(all[..2], [Step::StopManager, Step::StopService]);
    assert_eq!(
        plan[1].undo,
        Some(Step::StartService),
        "a failed upgrade starts it again"
    );
    assert_eq!(all.last(), Some(&Step::StartService));
    // The configuration is kept: none is written.
    assert!(!all.contains(&Step::WriteDefaultConfig));
    // Nothing that was there before is taken away if the upgrade fails.
    for planned in &plan {
        match &planned.step {
            Step::MakeDir(_)
            | Step::TrustCertificate
            | Step::AddDriverPackage
            | Step::CreateService
            | Step::RegisterEventSource
            | Step::AddFirewallRules
            | Step::AddQosPolicy
            | Step::AddShortcuts
            | Step::RegisterUninstall => {
                assert_eq!(planned.undo, None, "{:?}", planned.step);
            }
            // A file is put back as it was.
            Step::InstallFile(name) => {
                assert_eq!(planned.undo, Some(Step::RestoreFile(name.clone())));
            }
            _ => {}
        }
    }
    // No port is touched by an upgrade.
    assert!(!all.contains(&Step::RemovePorts));
    // A repair is the same plan from the same finding.
    assert_eq!(install(&found, &layout, &names(), true), plan);
}

#[test]
fn a_data_directory_under_the_old_name_is_renamed_and_back_if_it_fails() {
    let layout = layout();
    let old = PathBuf::from("C:\\ProgramData\\Quartz Systems\\rustmodem");
    let mut found = installed();
    found.data_dir = Some(old.clone());
    let plan = install(&found, &layout, &names(), false);
    let rename = plan
        .iter()
        .find(|planned| matches!(planned.step, Step::RenameDataDir { .. }))
        .expect("a rename");
    assert_eq!(
        rename.step,
        Step::RenameDataDir {
            from: old.clone(),
            to: layout.data_dir.clone()
        }
    );
    assert_eq!(
        rename.undo,
        Some(Step::RenameDataDir {
            from: layout.data_dir.clone(),
            to: old
        })
    );
    // Under the right name there is nothing to rename.
    let plan = install(&installed(), &layout, &names(), false);
    assert!(
        !steps(&plan)
            .iter()
            .any(|s| matches!(s, Step::RenameDataDir { .. }))
    );
}

#[test]
fn a_step_that_fails_undoes_what_was_done_last_first() {
    let layout = layout();
    let plan = install(&Found::default(), &layout, &names(), true);
    let mut machine = Machine {
        fail_at: Some(Step::CreateService),
        ..Machine::default()
    };
    let failure = carry_out(&plan, &mut machine).unwrap_err();
    assert_eq!(failure.id, "RMD9507");
    assert!(failure.text.contains("CreateService") && failure.text.contains("the disk is full"));

    // What was done: every step before the service, then their undos,
    // the last done first.
    let before: Vec<Planned> = plan
        .iter()
        .take_while(|planned| planned.step != Step::CreateService)
        .cloned()
        .collect();
    let mut expected = steps(&before);
    expected.extend(
        before
            .iter()
            .rev()
            .filter_map(|planned| planned.undo.clone()),
    );
    assert_eq!(machine.done, expected);
    // In particular: the driver package out, then the certificate, then
    // the configuration, the directories and the files.
    let undone = &machine.done[before.len()..];
    assert_eq!(
        undone[..3],
        [
            Step::RemoveDriverPackage,
            Step::DistrustCertificate,
            Step::RemoveConfig
        ]
    );
    assert_eq!(
        undone.last(),
        Some(&Step::RemoveDir(layout.install_dir.clone()))
    );
    // Nothing after the failure was tried.
    assert!(!machine.done.contains(&Step::StartService));
}

#[test]
fn a_failed_upgrade_starts_the_service_it_stopped_and_removes_nothing() {
    let plan = install(&installed(), &layout(), &names(), true);
    let mut machine = Machine {
        fail_at: Some(Step::AddDriverPackage),
        ..Machine::default()
    };
    assert!(carry_out(&plan, &mut machine).is_err());
    assert_eq!(machine.done.last(), Some(&Step::StartService));
    for removed in [
        Step::DeleteService,
        Step::RemoveDriverPackage,
        Step::DistrustCertificate,
        Step::RemoveConfig,
        Step::UnregisterUninstall,
        Step::RemovePorts,
    ] {
        assert!(!machine.done.contains(&removed), "{removed:?}");
    }
    // Every file that was replaced was put back.
    let restored = machine
        .done
        .iter()
        .filter(|step| matches!(step, Step::RestoreFile(_)))
        .count();
    assert_eq!(restored, 8);
}

#[test]
fn an_undo_that_fails_is_said_and_the_rest_are_still_undone() {
    let layout = layout();
    let plan = install(&Found::default(), &layout, &names(), true);
    let mut machine = Machine {
        fail_at: Some(Step::CreateService),
        stubborn: vec![Step::RemoveDriverPackage],
        ..Machine::default()
    };
    let failure = carry_out(&plan, &mut machine).unwrap_err();
    assert_eq!(failure.id, "RMD9507");
    assert!(machine.log.iter().any(|line| line.contains("RMD9511")));
    assert!(machine.done.contains(&Step::DistrustCertificate));
    assert!(machine.done.contains(&Step::RemoveDir(layout.install_dir)));
}

#[test]
fn a_plan_that_goes_through_does_every_step_once() {
    let plan = install(&Found::default(), &layout(), &names(), true);
    let mut machine = Machine::default();
    carry_out(&plan, &mut machine).unwrap();
    assert_eq!(machine.done, steps(&plan));
    assert!(machine.log.iter().any(|line| line.contains("StartService")));
}

#[test]
fn an_uninstall_removes_everything_and_keeps_the_data_unless_purged() {
    let layout = layout();
    let plan = uninstall(&installed(), &layout, true, false);
    assert_eq!(
        plan[..11],
        [
            Step::StopManager,
            Step::StopService,
            Step::RemovePorts,
            Step::RemoveDriverPackage,
            Step::DeleteService,
            Step::UnregisterEventSource,
            Step::RemoveFirewallRules,
            Step::RemoveQosPolicy,
            Step::RemoveShortcuts,
            Step::UnregisterUninstall,
            Step::DistrustCertificate,
        ]
    );
    for name in names() {
        assert!(plan.contains(&Step::RemoveFile(name.clone())), "{name}");
    }
    assert_eq!(
        plan.last(),
        Some(&Step::RemoveDir(layout.install_dir.clone()))
    );
    assert!(!plan.iter().any(|step| matches!(step, Step::RemoveTree(_))));
    // Every directory a file was in goes too, after its files and before
    // the directory it is in.
    let mut found = installed();
    found.files.push("docs\\AT.md".to_string());
    found.files.push("docs\\pictures\\line.png".to_string());
    let deeper = uninstall(&found, &layout, true, false);
    let at = |step: Step| deeper.iter().position(|s| *s == step).expect("in the plan");
    let directory = |name: &str| Step::RemoveDir(layout.install_dir.join(name));
    assert!(
        at(Step::RemoveFile("docs\\pictures\\line.png".into())) < at(directory("docs\\pictures"))
    );
    assert!(at(directory("docs\\pictures")) < at(directory("docs")));
    assert!(at(directory("driver")) < at(Step::RemoveDir(layout.install_dir.clone())));
    assert!(at(directory("docs")) < at(Step::RemoveDir(layout.install_dir.clone())));
    assert_eq!(
        deeper.iter().filter(|s| **s == directory("docs")).count(),
        1
    );

    let purged = uninstall(&installed(), &layout, true, true);
    assert_eq!(
        purged.last(),
        Some(&Step::RemoveTree(layout.data_dir.clone()))
    );
    assert_eq!(purged[..purged.len() - 1], plan[..]);

    // A build signed for distribution trusted no certificate, and an
    // uninstall by it removes one only if a lab build left one.
    let mut found = installed();
    found.certificate_trusted = false;
    assert!(!uninstall(&found, &layout, false, false).contains(&Step::DistrustCertificate));
    assert!(uninstall(&installed(), &layout, false, false).contains(&Step::DistrustCertificate));
}

#[test]
fn an_uninstall_goes_on_past_a_step_that_fails_and_says_which() {
    let plan = uninstall(&installed(), &layout(), true, true);
    let mut machine = Machine {
        fail_at: Some(Step::RemovePorts),
        ..Machine::default()
    };
    let failures = carry_out_all(&plan, &mut machine);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].id, "RMD9513");
    assert!(failures[0].text.contains("RemovePorts"));
    assert_eq!(machine.done.len(), plan.len() - 1);
    assert_eq!(machine.done.last(), plan.last());
}

#[test]
fn the_configuration_a_new_install_starts_with_is_one_the_service_takes() {
    let text = include_str!("../rustmodem.default.toml");
    let file = config::ConfigFile::parse(text).expect("the default configuration is valid");
    let config = file.config();
    assert!(config.lines.is_empty(), "a new install has no lines");
    assert!(
        !config.sip.enabled,
        "and no telephony until it is given a gateway"
    );
}
