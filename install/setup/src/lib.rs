// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The RustModem setup program, `rustmodem-setup.exe`.
//!
//! One program installs, upgrades, repairs and removes RustModem, and
//! makes and removes the ports of its lines. It is the only part of
//! RustModem that does what needs an administrator: the service does none
//! of it, and the manager and `rustmodem-ctl` run this program, elevated,
//! when a port is to be added or removed.
//!
//! - [`payload`]: the files packed into the setup program, and their
//!   checksum.
//! - [`plan`]: what an install, an upgrade, a repair and an uninstall do,
//!   as steps in order, each with the step that undoes it; and the
//!   carrying out of a plan, which undoes what it did if a step fails.
//! - [`ports`]: `port add`, `port remove` and `port list`, over a device
//!   layer.
//! - [`cli`]: the command line.
//! - `windows`: the steps and the device layer as they are done on
//!   Windows.
//!
//! The plans and the port commands know nothing of Windows: they are
//! given a [`plan::System`] and a [`ports::Devices`], which the tests
//! replace, so that what setup would do, in what order, and what it
//! undoes when something fails, is tested on any machine.

#![forbid(unsafe_code)]

pub mod cli;
pub mod payload;
pub mod plan;
pub mod ports;
#[cfg(windows)]
pub mod windows;

/// The product's name, wherever a person reads it.
pub const PRODUCT: &str = "RustModem";
/// The publisher.
pub const PUBLISHER: &str = "Quartz Systems";
/// The version this setup program installs.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The service's short name, which is an identifier and stays as it was.
pub const SERVICE_NAME: &str = "rustmodem";
/// The account the service runs under: its own, which Windows makes for
/// a service of that name and which has no password.
pub const SERVICE_ACCOUNT: &str = "NT SERVICE\\rustmodem";
/// The hardware id of a line's port. The driver's INF matches it.
pub const PORT_HARDWARE_ID: &str = "RUSTMODEM\\VCOM";
/// The common name of the lab certificate, by which it is found in a
/// certificate store.
pub const LAB_CERTIFICATE: &str = "RustModem Lab";

/// The programs and documents of the payload, by their names in it and
/// in the install directory.
pub mod files {
    /// The service.
    pub const SERVICE: &str = "rustmodem-svc.exe";
    /// The manager.
    pub const MANAGER: &str = "rustmodem-manager.exe";
    /// The command-line client.
    pub const CTL: &str = "rustmodem-ctl.exe";
    /// This program, as it is kept in the install directory.
    pub const SETUP: &str = "rustmodem-setup.exe";
    /// The driver package's directory, and what is in it.
    pub const DRIVER_DIR: &str = "driver";
    pub const PORT_INF: &str = "driver\\rmvcom.inf";
    pub const MODEM_INF: &str = "driver\\rmmodem.inf";
    /// The lab certificate. A setup program that has it is a lab build.
    pub const LAB_CERTIFICATE: &str = "driver\\lab.cer";
    /// The configuration a new install starts with.
    pub const DEFAULT_CONFIG: &str = "rustmodem.default.toml";
    /// The configuration file's name in the data directory.
    pub const CONFIG: &str = "rustmodem.toml";
}

/// The exit codes of the setup program.
pub mod exit {
    /// Done.
    pub const OK: i32 = 0;
    /// The arguments were not understood.
    pub const USAGE: i32 = 1;
    /// The payload is missing or damaged.
    pub const DAMAGED: i32 = 2;
    /// Setup needs an administrator and could not become one.
    pub const NEEDS_ADMIN: i32 = 3;
    /// The person said no.
    pub const CANCELLED: i32 = 4;
    /// A step failed. What was done has been undone.
    pub const FAILED: i32 = 5;
    /// This Windows is not one RustModem runs on.
    pub const UNSUPPORTED: i32 = 6;
    /// There is no install to repair, upgrade or remove.
    pub const NOT_INSTALLED: i32 = 7;
}
