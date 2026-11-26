// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Gives this crate's own test programs and examples an application
//! manifest.
//!
//! Anything that uses these wrappers needs version 6 of the common
//! controls, and a program gets that only by saying so in its manifest:
//! without one it does not start at all, because the task dialog exists
//! only in version 6. The manager has its manifest from its own build
//! script. This one is for what Cargo builds from this crate: its tests
//! and the sampler.

use glyph::{LinkInto, Manifest, link_windows_res, windows_res};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = Manifest {
        name: "QuartzSystems.rustmodem.Win32Tests".to_string(),
        description: "rustmodem Win32 wrapper tests".to_string(),
        version: [0, 0, 0, 0],
        gui: true,
    };
    let res = windows_res(None, None, &manifest);
    link_windows_res("manifest.res", &res, &[LinkInto::Everything]);
}
