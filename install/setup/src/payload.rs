// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The setup program as a file: the program itself, then the payload,
//! then a footer.
//!
//! The payload is the files to install, one after another, each as:
//!
//! | Octets | What |
//! |---|---|
//! | 2 | the length of the name, little-endian |
//! | n | the name, UTF-8, with `\` between its parts |
//! | 8 | the length of the file, little-endian |
//! | n | the file |
//!
//! It is not compressed: there is no compressor here that is not C.
//!
//! The footer is the last 56 octets of the file:
//!
//! | Octets | What |
//! |---|---|
//! | 8 | `RMSETUP1` |
//! | 8 | where the payload starts, from the start of the file |
//! | 8 | the length of the payload |
//! | 32 | the SHA-256 of the payload |
//!
//! The checksum is checked before anything is installed or changed.
//!
//! When a setup program is signed, the signature is put after the footer,
//! so the footer is looked for from the end of the file backwards, at
//! the eight-octet boundaries a signature keeps.

use std::fmt;

use sha2::{Digest, Sha256};

/// What the footer starts with.
pub const MAGIC: &[u8; 8] = b"RMSETUP1";
/// The length of the footer.
pub const FOOTER: usize = 8 + 8 + 8 + 32;
/// How far back from the end of the file the footer is looked for: more
/// than any signature is long.
const SEARCH: usize = 64 * 1024;

/// One file of the payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name, relative to the install directory.
    pub name: String,
    pub data: Vec<u8>,
}

/// Why a setup program's payload cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    /// The file has no payload: it is the setup program alone.
    Missing,
    /// The payload does not match its checksum, or is not made as a
    /// payload is.
    Damaged,
}

impl fmt::Display for Damage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Damage::Missing => "the file has no payload",
            Damage::Damaged => "the payload does not match its checksum",
        })
    }
}

/// Packs files into a payload.
pub fn pack(entries: &[Entry]) -> Vec<u8> {
    let mut payload = Vec::new();
    for entry in entries {
        let name = entry.name.as_bytes();
        payload.extend((name.len() as u16).to_le_bytes());
        payload.extend(name);
        payload.extend((entry.data.len() as u64).to_le_bytes());
        payload.extend(&entry.data);
    }
    payload
}

/// A setup program: `stub`, the program, with the payload and the footer
/// after it. The payload starts at a boundary of eight octets.
pub fn assemble(stub: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut file = stub.to_vec();
    while !file.len().is_multiple_of(8) {
        file.push(0);
    }
    let offset = file.len() as u64;
    file.extend(payload);
    while !file.len().is_multiple_of(8) {
        file.push(0);
    }
    file.extend(MAGIC);
    file.extend(offset.to_le_bytes());
    file.extend((payload.len() as u64).to_le_bytes());
    file.extend(Sha256::digest(payload));
    file
}

/// The payload of a setup program, checked against its checksum.
pub fn payload_of(file: &[u8]) -> Result<&[u8], Damage> {
    let footer = find_footer(file).ok_or(Damage::Missing)?;
    let word = |at: usize| {
        let octets: [u8; 8] = file[footer + at..footer + at + 8]
            .try_into()
            .expect("eight octets");
        u64::from_le_bytes(octets) as usize
    };
    let (offset, length) = (word(8), word(16));
    let end = offset.checked_add(length).ok_or(Damage::Damaged)?;
    if end > footer {
        return Err(Damage::Damaged);
    }
    let payload = &file[offset..end];
    if Sha256::digest(payload).as_slice() != &file[footer + 24..footer + FOOTER] {
        return Err(Damage::Damaged);
    }
    Ok(payload)
}

/// A setup program without what it carries: the program alone, which a
/// new setup program is made from.
pub fn stub_of(file: &[u8]) -> &[u8] {
    match find_footer(file) {
        Some(footer) => {
            let octets: [u8; 8] = file[footer + 8..footer + 16]
                .try_into()
                .expect("eight octets");
            let offset = u64::from_le_bytes(octets) as usize;
            if offset <= footer {
                &file[..offset]
            } else {
                file
            }
        }
        None => file,
    }
}

/// Where the footer starts: the last place, at a boundary of eight
/// octets and near the end of the file, that the magic is.
fn find_footer(file: &[u8]) -> Option<usize> {
    if file.len() < FOOTER {
        return None;
    }
    let last = (file.len() - FOOTER) / 8 * 8;
    let first = last.saturating_sub(SEARCH);
    (first..=last)
        .rev()
        .step_by(8)
        .find(|&at| &file[at..at + 8] == MAGIC)
}

/// The files of a payload.
pub fn unpack(payload: &[u8]) -> Result<Vec<Entry>, Damage> {
    let mut entries = Vec::new();
    let mut rest = payload;
    let mut take = |count: usize| -> Result<&[u8], Damage> {
        if rest.len() < count {
            return Err(Damage::Damaged);
        }
        let (head, tail) = rest.split_at(count);
        rest = tail;
        Ok(head)
    };
    while let Ok(length) = take(2) {
        let length = u16::from_le_bytes([length[0], length[1]]) as usize;
        let name = String::from_utf8(take(length)?.to_vec()).map_err(|_| Damage::Damaged)?;
        // A name that could leave the install directory is not one this
        // program packed.
        if name.is_empty() || name.contains("..") || name.contains(':') || name.starts_with('\\') {
            return Err(Damage::Damaged);
        }
        let size: [u8; 8] = take(8)?.try_into().expect("eight octets");
        let size = usize::try_from(u64::from_le_bytes(size)).map_err(|_| Damage::Damaged)?;
        entries.push(Entry {
            name,
            data: take(size)?.to_vec(),
        });
    }
    // The padding before the footer is zeroes, and nothing else may be
    // left over.
    if rest.iter().any(|&octet| octet != 0) {
        return Err(Damage::Damaged);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<Entry> {
        vec![
            Entry {
                name: "rustmodem-svc.exe".to_string(),
                data: (0..5000u32).map(|i| (i * 7) as u8).collect(),
            },
            Entry {
                name: "driver\\rmvcom.inf".to_string(),
                data: b"[Version]\r\n".to_vec(),
            },
            Entry {
                name: "LICENSE".to_string(),
                data: Vec::new(),
            },
        ]
    }

    #[test]
    fn what_is_packed_is_what_is_unpacked() {
        let stub = vec![0x4D; 1003];
        let file = assemble(&stub, &pack(&entries()));
        assert!(file.starts_with(&stub));
        let payload = payload_of(&file).unwrap();
        assert_eq!(unpack(payload).unwrap(), entries());
        // With nothing in it, too.
        let file = assemble(&stub, &pack(&[]));
        assert_eq!(unpack(payload_of(&file).unwrap()).unwrap(), []);
    }

    #[test]
    fn a_setup_program_can_be_made_from_one_that_already_carries_files() {
        let stub = vec![0x4D; 1000];
        let file = assemble(&stub, &pack(&entries()));
        assert_eq!(stub_of(&file), stub);
        assert_eq!(stub_of(&stub), stub);
        let again = assemble(stub_of(&file), &pack(&entries()[..1]));
        assert_eq!(unpack(payload_of(&again).unwrap()).unwrap(), entries()[..1]);
    }

    #[test]
    fn a_program_with_no_payload_is_said_to_have_none() {
        assert_eq!(payload_of(&[0x4D; 5000]), Err(Damage::Missing));
        assert_eq!(payload_of(&[]), Err(Damage::Missing));
        assert_eq!(payload_of(b"RMSETUP1"), Err(Damage::Missing));
    }

    #[test]
    fn a_changed_octet_anywhere_in_the_payload_is_damage() {
        let stub = vec![0x4D; 1000];
        let file = assemble(&stub, &pack(&entries()));
        for at in (1000..file.len() - FOOTER).step_by(97) {
            let mut changed = file.clone();
            changed[at] ^= 0x10;
            assert_eq!(payload_of(&changed), Err(Damage::Damaged), "octet {at}");
        }
        // And a footer that points outside the file.
        let mut changed = file.clone();
        let footer = file.len() - FOOTER;
        changed[footer + 16..footer + 24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(payload_of(&changed), Err(Damage::Damaged));
    }

    #[test]
    fn a_signature_after_the_footer_does_not_hide_it() {
        let file = assemble(&[0x4D; 1000], &pack(&entries()));
        for signature in [8, 1024, 9000, 16_384] {
            let mut signed = file.clone();
            signed.extend(std::iter::repeat_n(0xA5, signature));
            let payload = payload_of(&signed).unwrap();
            assert_eq!(unpack(payload).unwrap(), entries(), "{signature} octets");
        }
    }

    #[test]
    fn a_name_that_would_leave_the_install_directory_is_damage() {
        for name in [
            "..\\evil.exe",
            "C:\\Windows\\evil.exe",
            "\\evil.exe",
            "a\\..\\..\\b",
            "",
        ] {
            let payload = pack(&[Entry {
                name: name.to_string(),
                data: vec![1],
            }]);
            assert_eq!(unpack(&payload), Err(Damage::Damaged), "{name:?}");
        }
        // And a payload cut short.
        let payload = pack(&entries());
        assert_eq!(unpack(&payload[..payload.len() - 3]), Err(Damage::Damaged));
    }
}
