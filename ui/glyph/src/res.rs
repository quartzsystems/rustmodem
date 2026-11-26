// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A compiled Windows resource file (`.res`), written directly.
//!
//! The file holds a program's icon, its version information and its
//! application manifest. The MSVC linker takes a `.res` as an input, so no
//! resource compiler is needed: a build script writes the file with
//! [`link_windows_res`] and tells Cargo to pass it to the linker.

use std::path::PathBuf;

use crate::Glyph;
use crate::ico::{ICON_SIZES, entry, image};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const RT_MANIFEST: u16 = 24;

/// US English.
const LANGUAGE: u16 = 0x0409;

/// The code page of the version strings: Unicode.
const CODE_PAGE: u16 = 0x04B0;

/// What a program's version resource says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    /// The product: `rustmodem`.
    pub product: String,
    /// The company: `Quartz Systems`.
    pub company: String,
    /// What this file is, for example `RustModem Manager`.
    pub description: String,
    /// Major, minor, patch and build.
    pub version: [u16; 4],
    /// The file name without its extension.
    pub internal_name: String,
    /// The file name.
    pub original_filename: String,
    /// The copyright line.
    pub copyright: String,
}

impl VersionInfo {
    /// The version information of a rustmodem program. `version` is the
    /// workspace version, `major.minor.patch`; a part that is not a number
    /// is written as 0.
    pub fn rustmodem(description: &str, file: &str, version: &str) -> VersionInfo {
        let mut parts = version
            .split(['.', '-', '+'])
            .map(|part| part.parse::<u16>().unwrap_or(0));
        let mut next = || parts.next().unwrap_or(0);
        VersionInfo {
            product: "rustmodem".to_string(),
            company: "Quartz Systems".to_string(),
            description: description.to_string(),
            version: [next(), next(), next(), 0],
            internal_name: file.trim_end_matches(".exe").to_string(),
            original_filename: file.to_string(),
            copyright: "Copyright (C) 2026 Quartz Systems".to_string(),
        }
    }

    /// The version as people read it: `major.minor.patch`.
    pub fn text(&self) -> String {
        let [major, minor, patch, _] = self.version;
        format!("{major}.{minor}.{patch}")
    }
}

/// What a program's application manifest asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The name of the assembly, for example
    /// `QuartzSystems.rustmodem.Manager`.
    pub name: String,
    /// What the program is, for people.
    pub description: String,
    /// Major, minor, patch and build.
    pub version: [u16; 4],
    /// Version 6 of the common controls, and per-monitor DPI awareness:
    /// what a program with windows needs to look like the rest of Windows.
    pub gui: bool,
}

impl Manifest {
    /// The manifest as XML.
    ///
    /// Every program runs with the rights of whoever starts it
    /// (`asInvoker`): without a manifest Windows guesses from a name such
    /// as `setup.exe` that a program needs administrator rights. Every
    /// program says it knows Windows 10 and later, and long paths. A
    /// program with windows also depends on
    /// `Microsoft.Windows.Common-Controls` version 6.0.0.0 and is
    /// `PerMonitorV2` DPI aware, with `true/pm` for versions of Windows
    /// that do not know that.
    pub fn xml(&self) -> String {
        let [major, minor, patch, build] = self.version;
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n");
        xml.push_str(
            "<assembly xmlns=\"urn:schemas-microsoft-com:asm.v1\" manifestVersion=\"1.0\" \
             xmlns:asmv3=\"urn:schemas-microsoft-com:asm.v3\">\r\n",
        );
        xml.push_str(&format!(
            "  <assemblyIdentity type=\"win32\" name=\"{}\" version=\"{major}.{minor}.{patch}.{build}\" \
             processorArchitecture=\"amd64\"/>\r\n",
            escape(&self.name)
        ));
        xml.push_str(&format!(
            "  <description>{}</description>\r\n",
            escape(&self.description)
        ));
        if self.gui {
            xml.push_str(
                "  <dependency>\r\n    <dependentAssembly>\r\n      <assemblyIdentity type=\"win32\" \
                 name=\"Microsoft.Windows.Common-Controls\" version=\"6.0.0.0\" \
                 processorArchitecture=\"amd64\" publicKeyToken=\"6595b64144ccf1df\" language=\"*\"/>\r\n    \
                 </dependentAssembly>\r\n  </dependency>\r\n",
            );
        }
        xml.push_str(
            "  <trustInfo xmlns=\"urn:schemas-microsoft-com:asm.v3\">\r\n    <security>\r\n      \
             <requestedPrivileges>\r\n        <requestedExecutionLevel level=\"asInvoker\" \
             uiAccess=\"false\"/>\r\n      </requestedPrivileges>\r\n    </security>\r\n  </trustInfo>\r\n",
        );
        // The one identifier that stands for Windows 10 and everything
        // after it.
        xml.push_str(
            "  <compatibility xmlns=\"urn:schemas-microsoft-com:compatibility.v1\">\r\n    \
             <application>\r\n      <supportedOS Id=\"{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}\"/>\r\n    \
             </application>\r\n  </compatibility>\r\n",
        );
        xml.push_str("  <asmv3:application>\r\n    <asmv3:windowsSettings>\r\n");
        if self.gui {
            xml.push_str(
                "      <dpiAware xmlns=\"http://schemas.microsoft.com/SMI/2005/WindowsSettings\">\
                 true/pm</dpiAware>\r\n      <dpiAwareness \
                 xmlns=\"http://schemas.microsoft.com/SMI/2016/WindowsSettings\">PerMonitorV2\
                 </dpiAwareness>\r\n",
            );
        }
        xml.push_str(
            "      <longPathAware xmlns=\"http://schemas.microsoft.com/SMI/2016/WindowsSettings\">\
             true</longPathAware>\r\n    </asmv3:windowsSettings>\r\n  </asmv3:application>\r\n</assembly>\r\n",
        );
        xml
    }
}

/// Text made safe for XML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn pad4(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Text as terminated UTF-16.
fn wide(out: &mut Vec<u8>, text: &str) {
    for unit in text.encode_utf16() {
        put16(out, unit);
    }
    put16(out, 0);
}

/// One resource: a 32-byte header that names the type and the resource by
/// number, then the data, padded to four bytes.
fn resource(out: &mut Vec<u8>, kind: u16, id: u16, flags: u16, language: u16, data: &[u8]) {
    put32(out, data.len() as u32);
    put32(out, 32);
    put16(out, 0xFFFF);
    put16(out, kind);
    put16(out, 0xFFFF);
    put16(out, id);
    put32(out, 0); // data version
    put16(out, flags);
    put16(out, language);
    put32(out, 0); // version
    put32(out, 0); // characteristics
    out.extend_from_slice(data);
    pad4(out);
}

/// One node of a version resource: its length, the length of its value,
/// whether the value is text, its key, then the value and the children,
/// each starting on a four-byte boundary.
fn node(key: &str, value: &[u8], value_length: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    put16(&mut out, 0);
    put16(&mut out, value_length);
    put16(&mut out, u16::from(text));
    wide(&mut out, key);
    pad4(&mut out);
    out.extend_from_slice(value);
    for child in children {
        pad4(&mut out);
        out.extend_from_slice(child);
    }
    let length = out.len() as u16;
    out[0..2].copy_from_slice(&length.to_le_bytes());
    out
}

/// A string of a version resource. The length of its value is counted in
/// characters, terminator included.
fn string(key: &str, value: &str) -> Vec<u8> {
    let mut text = Vec::new();
    wide(&mut text, value);
    node(key, &text, (text.len() / 2) as u16, true, &[])
}

/// The `VS_VERSIONINFO` structure.
pub fn version_resource(info: &VersionInfo) -> Vec<u8> {
    let [major, minor, patch, build] = info.version.map(u32::from);
    // VS_FIXEDFILEINFO.
    let mut fixed = Vec::new();
    put32(&mut fixed, 0xFEEF_04BD); // signature
    put32(&mut fixed, 0x0001_0000); // structure version
    put32(&mut fixed, (major << 16) | minor); // file version
    put32(&mut fixed, (patch << 16) | build);
    put32(&mut fixed, (major << 16) | minor); // product version
    put32(&mut fixed, (patch << 16) | build);
    put32(&mut fixed, 0x3F); // file flags mask
    put32(&mut fixed, 0); // file flags
    put32(&mut fixed, 0x0004_0004); // VOS_NT_WINDOWS32
    put32(&mut fixed, 1); // VFT_APP
    put32(&mut fixed, 0); // subtype
    put32(&mut fixed, 0); // date
    put32(&mut fixed, 0);
    let dotted = format!("{major}.{minor}.{patch}.{build}");
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", info.company.as_str()),
        ("FileDescription", info.description.as_str()),
        ("FileVersion", dotted.as_str()),
        ("InternalName", info.internal_name.as_str()),
        ("LegalCopyright", info.copyright.as_str()),
        ("OriginalFilename", info.original_filename.as_str()),
        ("ProductName", info.product.as_str()),
        ("ProductVersion", &info.text()),
    ]
    .iter()
    .map(|(key, value)| string(key, value))
    .collect();
    // The table is named by its language and code page in hexadecimal.
    let table = node(
        &format!("{LANGUAGE:04x}{CODE_PAGE:04x}"),
        &[],
        0,
        true,
        &strings,
    );
    let string_file_info = node("StringFileInfo", &[], 0, true, &[table]);
    let mut translation = Vec::new();
    put16(&mut translation, LANGUAGE);
    put16(&mut translation, CODE_PAGE);
    let var = node("Translation", &translation, 4, false, &[]);
    let var_file_info = node("VarFileInfo", &[], 0, true, &[var]);
    node(
        "VS_VERSION_INFO",
        &fixed,
        fixed.len() as u16,
        false,
        &[string_file_info, var_file_info],
    )
}

/// A `.res` file. With an icon it is icon group 1, the group Explorer
/// shows for the file and the one a window class loads; the manifest is
/// resource 1, the one Windows reads when it starts the program.
pub fn windows_res(
    icon: Option<Glyph>,
    version: Option<&VersionInfo>,
    manifest: &Manifest,
) -> Vec<u8> {
    let mut out = Vec::new();
    // The empty resource every .res file starts with.
    resource(&mut out, 0, 0, 0, 0, &[]);
    if let Some(glyph) = icon {
        let mut group = Vec::new();
        put16(&mut group, 0);
        put16(&mut group, 1);
        put16(&mut group, ICON_SIZES.len() as u16);
        for (index, px) in ICON_SIZES.iter().enumerate() {
            let data = image(glyph, *px);
            let id = index as u16 + 1;
            resource(&mut out, RT_ICON, id, 0x1010, LANGUAGE, &data);
            // As in an icon file, with the number of the icon resource
            // where the file has the offset of the image.
            entry(&mut group, *px, data.len());
            put16(&mut group, id);
        }
        resource(&mut out, RT_GROUP_ICON, 1, 0x1030, LANGUAGE, &group);
    }
    if let Some(version) = version {
        resource(
            &mut out,
            RT_VERSION,
            1,
            0x0030,
            LANGUAGE,
            &version_resource(version),
        );
    }
    resource(
        &mut out,
        RT_MANIFEST,
        1,
        0x0030,
        LANGUAGE,
        manifest.xml().as_bytes(),
    );
    out
}

/// What a build script asks to have linked into its package's programs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkInto {
    /// The package's binaries: `cargo:rustc-link-arg-bins`.
    Bins,
    /// The package's integration tests: `cargo:rustc-link-arg-tests`.
    Tests,
    /// The package's examples: `cargo:rustc-link-arg-examples`.
    Examples,
    /// Everything the package links: `cargo:rustc-link-arg`.
    Everything,
}

/// For build scripts: writes `name` into `OUT_DIR` and tells Cargo to pass
/// it to the linker for the targets given. Does nothing unless the target
/// is Windows with the MSVC linker, which is the one that takes a `.res`.
pub fn link_windows_res(name: &str, res: &[u8], into: &[LinkInto]) {
    let target = |key: &str| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") != "windows" || target("CARGO_CFG_TARGET_ENV") != "msvc" {
        return;
    }
    let directory =
        PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set for build scripts"));
    let path = directory.join(name);
    std::fs::write(&path, res).expect("the resource file can be written to OUT_DIR");
    for target in into {
        let key = match target {
            LinkInto::Bins => "rustc-link-arg-bins",
            LinkInto::Tests => "rustc-link-arg-tests",
            LinkInto::Examples => "rustc-link-arg-examples",
            LinkInto::Everything => "rustc-link-arg",
        };
        println!("cargo:{key}={}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits a `.res` into type, name and data.
    fn resources(res: &[u8]) -> Vec<(u16, u16, Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 0;
        let u16_at = |at: usize| u16::from_le_bytes([res[at], res[at + 1]]);
        let u32_at = |at: usize| u32::from_le_bytes(res[at..at + 4].try_into().unwrap());
        while at < res.len() {
            let size = u32_at(at) as usize;
            assert_eq!(u32_at(at + 4), 32, "the header is 32 bytes");
            assert_eq!((u16_at(at + 8), u16_at(at + 12)), (0xFFFF, 0xFFFF));
            out.push((
                u16_at(at + 10),
                u16_at(at + 14),
                res[at + 32..at + 32 + size].to_vec(),
            ));
            at = (at + 32 + size).div_ceil(4) * 4;
        }
        assert_eq!(at, res.len(), "the file ends on a four-byte boundary");
        out
    }

    /// The text of UTF-16 data, for looking for strings in it.
    fn utf16(data: &[u8]) -> String {
        data.as_chunks::<2>()
            .0
            .iter()
            .map(|c| char::from_u32(u32::from(u16::from_le_bytes(*c))).unwrap_or('?'))
            .collect()
    }

    fn manager() -> (VersionInfo, Manifest) {
        let version = VersionInfo::rustmodem("RustModem Manager", "rustmodem-manager.exe", "1.2.3");
        let manifest = Manifest {
            name: "QuartzSystems.rustmodem.Manager".to_string(),
            description: "RustModem Manager".to_string(),
            version: version.version,
            gui: true,
        };
        (version, manifest)
    }

    #[test]
    fn the_version_comes_from_the_workspace_version() {
        let (version, _) = manager();
        assert_eq!(version.version, [1, 2, 3, 0]);
        assert_eq!(version.text(), "1.2.3");
        assert_eq!(version.internal_name, "rustmodem-manager");
        assert_eq!(version.original_filename, "rustmodem-manager.exe");
        assert_eq!(version.company, "Quartz Systems");
        assert_eq!(version.product, "rustmodem");
        let odd = VersionInfo::rustmodem("x", "x.exe", "0.0.2-rc.1");
        assert_eq!(odd.version, [0, 0, 2, 0]);
        assert_eq!(
            VersionInfo::rustmodem("x", "x.exe", "7").version,
            [7, 0, 0, 0]
        );
    }

    #[test]
    fn a_resource_file_holds_the_icon_the_version_and_the_manifest() {
        let (version, manifest) = manager();
        let res = windows_res(Some(Glyph::App), Some(&version), &manifest);
        let all = resources(&res);
        let kinds: Vec<(u16, u16)> = all.iter().map(|r| (r.0, r.1)).collect();
        assert_eq!(
            kinds,
            [
                (0, 0),
                (RT_ICON, 1),
                (RT_ICON, 2),
                (RT_ICON, 3),
                (RT_ICON, 4),
                (RT_ICON, 5),
                (RT_ICON, 6),
                (RT_GROUP_ICON, 1),
                (RT_VERSION, 1),
                (RT_MANIFEST, 1),
            ]
        );
        // The group names each image: its size, and the number of its
        // icon resource.
        let group = &all[7].2;
        assert_eq!(group[..6], [0, 0, 1, 0, 6, 0]);
        assert_eq!(group.len(), 6 + 14 * ICON_SIZES.len());
        for (index, px) in ICON_SIZES.iter().enumerate() {
            let at = 6 + 14 * index;
            let side = if *px == 256 { 0 } else { *px as u8 };
            assert_eq!(group[at], side);
            let bytes = u32::from_le_bytes(group[at + 8..at + 12].try_into().unwrap()) as usize;
            let id = u16::from_le_bytes([group[at + 12], group[at + 13]]);
            assert_eq!(id as usize, index + 1);
            assert_eq!(all[index + 1].2.len(), bytes);
            assert_eq!(all[index + 1].2, image(Glyph::App, *px));
        }
    }

    #[test]
    fn the_version_resource_is_well_formed_and_says_what_it_should() {
        let (version, _) = manager();
        let data = version_resource(&version);
        // The root node is as long as the whole resource, and its value is
        // the fixed file information with its signature.
        assert_eq!(u16::from_le_bytes([data[0], data[1]]) as usize, data.len());
        assert_eq!(u16::from_le_bytes([data[2], data[3]]), 52);
        let fixed = 6 + ("VS_VERSION_INFO".len() + 1) * 2;
        let fixed = fixed.div_ceil(4) * 4;
        assert_eq!(data[fixed..fixed + 4], 0xFEEF_04BDu32.to_le_bytes());
        // File version 1.2.3.0.
        assert_eq!(data[fixed + 8..fixed + 12], 0x0001_0002u32.to_le_bytes());
        assert_eq!(data[fixed + 12..fixed + 16], 0x0003_0000u32.to_le_bytes());
        let text = utf16(&data);
        for wanted in [
            "VS_VERSION_INFO",
            "StringFileInfo",
            "040904b0",
            "CompanyName",
            "Quartz Systems",
            "FileDescription",
            "RustModem Manager",
            "1.2.3.0",
            "ProductName",
            "rustmodem",
            "ProductVersion",
            "OriginalFilename",
            "rustmodem-manager.exe",
            "Copyright (C) 2026 Quartz Systems",
            "VarFileInfo",
            "Translation",
        ] {
            assert!(text.contains(wanted), "{wanted} is missing");
        }
    }

    #[test]
    fn the_manifest_of_the_manager_has_exactly_what_the_handoff_asks_for() {
        let (_, manifest) = manager();
        let xml = manifest.xml();
        for wanted in [
            // The common controls, version 6.
            "name=\"Microsoft.Windows.Common-Controls\" version=\"6.0.0.0\"",
            "publicKeyToken=\"6595b64144ccf1df\"",
            // Per-monitor DPI awareness, with the fallback.
            ">PerMonitorV2</dpiAwareness>",
            ">true/pm</dpiAware>",
            // Windows 10 and later.
            "<supportedOS Id=\"{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}\"/>",
            // Run as whoever starts it.
            "<requestedExecutionLevel level=\"asInvoker\" uiAccess=\"false\"/>",
            ">true</longPathAware>",
            "name=\"QuartzSystems.rustmodem.Manager\" version=\"1.2.3.0\"",
        ] {
            assert!(xml.contains(wanted), "{wanted} is missing from\n{xml}");
        }
        // Every element that is opened is closed.
        for element in [
            "assembly",
            "dependency",
            "dependentAssembly",
            "trustInfo",
            "security",
            "requestedPrivileges",
            "compatibility",
            "application",
            "asmv3:application",
            "asmv3:windowsSettings",
            "dpiAware",
            "dpiAwareness",
            "longPathAware",
            "description",
        ] {
            let opened = xml.matches(&format!("<{element}>")).count()
                + xml.matches(&format!("<{element} ")).count();
            let closed = xml.matches(&format!("</{element}>")).count();
            assert_eq!(opened, closed, "{element}");
            assert!(closed >= 1, "{element}");
        }
    }

    #[test]
    fn a_program_with_no_windows_asks_for_no_controls_and_no_dpi_awareness() {
        let manifest = Manifest {
            name: "QuartzSystems.rustmodem.Service & <more>".to_string(),
            description: "the \"service\"".to_string(),
            version: [0, 0, 2, 0],
            gui: false,
        };
        let xml = manifest.xml();
        assert!(!xml.contains("Common-Controls"));
        assert!(!xml.contains("dpiAware"));
        assert!(xml.contains("asInvoker"));
        assert!(xml.contains("longPathAware"));
        assert!(xml.contains("supportedOS"));
        assert!(xml.contains("Service &amp; &lt;more&gt;"));
        assert!(xml.contains("the &quot;service&quot;"));

        // With no icon and no version, the file is the manifest alone.
        let res = windows_res(None, None, &manifest);
        let all = resources(&res);
        assert_eq!(all.len(), 2);
        assert_eq!((all[1].0, all[1].1), (RT_MANIFEST, 1));
        assert_eq!(all[1].2, xml.as_bytes());
    }
}
