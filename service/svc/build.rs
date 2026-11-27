// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! Gives rustmodem-svc.exe its icon, its version information, its manifest and
//! the message table the Event Viewer reads its events through. The
//! resource file is written by `glyph` and handed to the linker; no
//! resource compiler is used.

use glyph::{
    Glyph, LinkInto, Manifest, VersionInfo, link_windows_res, message_table,
    windows_res_with_messages,
};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo sets the package version");
    let info = VersionInfo::rustmodem(
        "RustModem modem pool service",
        "rustmodem-svc.exe",
        &version,
    );
    let manifest = Manifest {
        name: "QuartzSystems.rustmodem.Service".to_string(),
        description: info.description.clone(),
        version: info.version,
        gui: false,
    };
    // The program is its own event message file: the setup program
    // registers it as the event source's. An event's id is its message's
    // number and its first string is its whole text, so every id the
    // catalogue has says "%1".
    let ids: Vec<u32> = msg::entries()
        .filter_map(|entry| entry.id.trim_start_matches("RMD").parse().ok())
        .collect();
    let messages = message_table(&ids, "%1\r\n");
    let res = windows_res_with_messages(Some(Glyph::App), Some(&info), &manifest, Some(&messages));
    link_windows_res("resources.res", &res, &[LinkInto::Bins]);
}
