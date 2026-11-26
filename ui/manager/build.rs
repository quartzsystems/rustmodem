// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Gives rustmodem-manager.exe its icon, its version information and its
//! manifest. The resource file is written by `glyph` and handed to the
//! linker; no resource compiler is used.
//!
//! The manifest asks for version 6 of the common controls, per-monitor
//! DPI awareness and `asInvoker`. Everything this package links gets the
//! same resources, the test programs too: a program that uses the task
//! dialog does not start without the manifest.

use glyph::{Glyph, LinkInto, Manifest, VersionInfo, link_windows_res, windows_res};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo sets the package version");
    let info = VersionInfo::rustmodem("RustModem Manager", "rustmodem-manager.exe", &version);
    let manifest = Manifest {
        name: "QuartzSystems.rustmodem.Manager".to_string(),
        description: info.description.clone(),
        version: info.version,
        gui: true,
    };
    let res = windows_res(Some(Glyph::App), Some(&info), &manifest);
    link_windows_res("resources.res", &res, &[LinkInto::Everything]);
}
