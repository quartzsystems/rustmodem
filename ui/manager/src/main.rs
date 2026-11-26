// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! `rustmodem-gui.exe`, the rustmodem Manager.

#![forbid(unsafe_code)]
// A program of the Windows subsystem: no console window opens behind it.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    std::process::exit(manager::main());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The rustmodem Manager runs on Windows only.");
    std::process::exit(1);
}
