// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The lines' stored profiles, kept in `profiles.toml` beside the
//! configuration file.
//!
//! The file has one table for each line that has given `AT&W`, keyed by
//! the line id:
//!
//! ```toml
//! [line.1]
//! echo = false
//! quiet = false
//! verbose = true
//! x = 4
//! volume = 0
//! speaker = 0
//! dcd_follows_carrier = true
//! dtr_action = 2
//! flow = 3
//! ms = "V21,1,0,0,0,0"
//! es = "3,0,2"
//! ds = "3,0,2048,6"
//! icf = "3,3"
//! s0 = 2
//! s2 = 43
//! ```
//!
//! The service writes the whole file each time a profile is stored or a
//! line is removed. It is the service's file, not the operator's: comments
//! added to it are not kept. A key that is missing takes its factory
//! value; a profile with a value no AT command could have stored is left
//! out, with a warning, and its line starts from its factory settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use msg::{Message, message};
use pool::{LineId, Profile, ProfileStore};
use toml_edit::{DocumentMut, Item, Table, value};

/// The name of the file, beside the configuration file.
pub const PROFILES_FILE: &str = "profiles.toml";

/// What the file starts with.
const HEADER: &str = "\
# rustmodem stored profiles: what AT&W saved for each line.
# Written by the service. Changes made by hand are read when the service
# starts; comments are not kept.

";

/// The highest line id, as the configuration has it.
const MAX_LINE_ID: i64 = 999;

/// The stored profiles and the file they are kept in.
pub struct ProfileFile {
    path: PathBuf,
    profiles: BTreeMap<LineId, Profile>,
}

impl ProfileFile {
    /// Where the profiles of a configuration file are kept.
    pub fn path_beside(config_path: &Path) -> PathBuf {
        config_path.with_file_name(PROFILES_FILE)
    }

    /// Reads the file. A file that is not there is no profiles and no
    /// problem. What is wrong with a file that is there comes back as
    /// warnings, and the profiles that are right are still used.
    pub fn open(path: &Path) -> (ProfileFile, Vec<Message>) {
        let shown = path.display().to_string();
        let (profiles, problems) = match std::fs::read_to_string(path) {
            Ok(text) => parse(&text, &shown),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (BTreeMap::new(), Vec::new())
            }
            Err(error) => (
                BTreeMap::new(),
                vec![message("RMD0024", &[&shown, &error.to_string()])],
            ),
        };
        (
            ProfileFile {
                path: path.to_path_buf(),
                profiles,
            },
            problems,
        )
    }

    /// Writes the file: to a temporary file beside it first, so that a
    /// failed write does not leave half of one.
    fn save(&self) -> Result<(), Message> {
        let failed = |e: std::io::Error| {
            message(
                "RMD0025",
                &[&self.path.display().to_string(), &e.to_string()],
            )
        };
        let mut temporary = self.path.as_os_str().to_owned();
        temporary.push(".new");
        let temporary = PathBuf::from(temporary);
        std::fs::write(&temporary, render(&self.profiles)).map_err(failed)?;
        std::fs::rename(&temporary, &self.path).map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            failed(e)
        })
    }
}

impl ProfileStore for ProfileFile {
    fn load(&mut self, line: LineId) -> Option<Profile> {
        self.profiles.get(&line).cloned()
    }

    fn store(&mut self, line: LineId, profile: &Profile) -> Result<(), Message> {
        // Kept in memory even if the file cannot be written, so that the
        // next successful write has it.
        self.profiles.insert(line, profile.clone());
        self.save()
    }

    fn remove(&mut self, line: LineId) -> Result<(), Message> {
        if self.profiles.remove(&line).is_none() {
            return Ok(());
        }
        self.save()
    }
}

/// The file's text for a set of profiles.
fn render(profiles: &BTreeMap<LineId, Profile>) -> String {
    let mut lines = Table::new();
    // `[line.1]` and so on, with no `[line]` header of its own.
    lines.set_implicit(true);
    for (id, profile) in profiles {
        let mut table = Table::new();
        table["echo"] = value(profile.echo);
        table["quiet"] = value(profile.quiet);
        table["verbose"] = value(profile.verbose);
        table["x"] = value(i64::from(profile.x));
        table["volume"] = value(i64::from(profile.volume));
        table["speaker"] = value(i64::from(profile.speaker));
        table["dcd_follows_carrier"] = value(profile.dcd_follows_carrier);
        table["dtr_action"] = value(i64::from(profile.dtr_action));
        table["flow"] = value(i64::from(profile.flow));
        table["ms"] = value(profile.ms.as_str());
        table["es"] = value(profile.es.as_str());
        table["ds"] = value(profile.ds.as_str());
        table["icf"] = value(profile.icf.as_str());
        for (number, register) in profile.registers() {
            // S1 counts the rings of a call in progress; it is not kept.
            if number != 1 {
                table[&format!("s{number}")] = value(i64::from(register));
            }
        }
        lines[&id.to_string()] = Item::Table(table);
    }
    let mut document = DocumentMut::new();
    if !profiles.is_empty() {
        document["line"] = Item::Table(lines);
    }
    format!("{HEADER}{document}")
}

/// Reads the file's text. `shown` is the path, for the warnings.
fn parse(text: &str, shown: &str) -> (BTreeMap<LineId, Profile>, Vec<Message>) {
    let mut profiles = BTreeMap::new();
    let mut problems = Vec::new();
    let document: DocumentMut = match text.parse() {
        Ok(document) => document,
        Err(error) => {
            let reason = error.message().to_string();
            return (profiles, vec![message("RMD0024", &[shown, &reason])]);
        }
    };
    let Some(lines) = document.get("line") else {
        return (profiles, problems);
    };
    let Some(lines) = lines.as_table_like() else {
        problems.push(message("RMD0024", &[shown, "line is not a table"]));
        return (profiles, problems);
    };
    for (key, item) in lines.iter() {
        let outcome = key
            .parse::<i64>()
            .ok()
            .filter(|id| (1..=MAX_LINE_ID).contains(id))
            .ok_or_else(|| "the table is not named by a line id from 1 to 999".to_string())
            .and_then(|id| Ok((id as LineId, profile(item)?)));
        match outcome {
            Ok((id, profile)) => {
                profiles.insert(id, profile);
            }
            Err(reason) => problems.push(message("RMD0026", &[key, shown, &reason])),
        }
    }
    (profiles, problems)
}

/// Reads one line's table. A missing key takes its factory value.
fn profile(item: &Item) -> Result<Profile, String> {
    let table = item
        .as_table_like()
        .ok_or_else(|| "it is not a table".to_string())?;
    let mut profile = Profile::factory(0);
    for (key, item) in table.iter() {
        let boolean = || {
            item.as_bool()
                .ok_or_else(|| format!("{key} is not true or false"))
        };
        let number = || {
            item.as_integer()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| format!("{key} is not a number from 0 to 255"))
        };
        let string = || {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{key} is not a string"))
        };
        match key {
            "echo" => profile.echo = boolean()?,
            "quiet" => profile.quiet = boolean()?,
            "verbose" => profile.verbose = boolean()?,
            "x" => profile.x = number()?,
            "volume" => profile.volume = number()?,
            "speaker" => profile.speaker = number()?,
            "dcd_follows_carrier" => profile.dcd_follows_carrier = boolean()?,
            "dtr_action" => profile.dtr_action = number()?,
            "flow" => profile.flow = number()?,
            "ms" => profile.ms = string()?,
            "es" => profile.es = string()?,
            "ds" => profile.ds = string()?,
            "icf" => profile.icf = string()?,
            register => {
                // `s0`, `s2` and so on: the registers a profile has, as
                // the file spells them. S1 is not a setting.
                let register_number = register
                    .strip_prefix('s')
                    .and_then(|digits| digits.parse::<u32>().ok())
                    .filter(|n| *n != 1 && format!("s{n}") == register);
                let kept = match register_number {
                    Some(n) => profile.set_register(n, number()?),
                    None => false,
                };
                if !kept {
                    return Err(format!("there is no setting {register}"));
                }
            }
        }
    }
    profile
        .check()
        .map_err(|what| format!("{what} has a value that is not allowed"))?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("rustmodem-profiles-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn changed() -> Profile {
        let mut profile = Profile::factory(2);
        profile.echo = false;
        profile.x = 1;
        profile.dtr_action = 3;
        profile.flow = 0;
        profile.ms = "B103,0,300,300,0,0".to_string();
        assert!(profile.set_register(7, 25));
        assert!(profile.set_register(12, 40));
        profile
    }

    #[test]
    fn the_profiles_are_beside_the_configuration_file() {
        let path = ProfileFile::path_beside(Path::new(r"C:\data\rustmodem.toml"));
        assert_eq!(path, Path::new(r"C:\data\profiles.toml"));
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let mut profiles = BTreeMap::new();
        profiles.insert(1, changed());
        profiles.insert(12, Profile::factory(0));
        profiles.insert(999, Profile::factory(255));
        let text = render(&profiles);
        assert!(text.starts_with("# rustmodem stored profiles"), "{text}");
        assert!(text.contains("\n[line.1]\necho = false\n"), "{text}");
        assert!(text.contains("\n[line.12]\n"), "{text}");
        assert!(text.contains("ms = \"B103,0,300,300,0,0\"\n"), "{text}");
        assert!(text.contains("\ns7 = 25\n"), "{text}");
        assert!(!text.contains("s1 ="), "S1 is not kept: {text}");
        assert!(!text.contains("[line]\n"), "{text}");

        let (read, problems) = parse(&text, "profiles.toml");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(read, profiles);

        let (none, problems) = parse(&render(&BTreeMap::new()), "profiles.toml");
        assert!(none.is_empty() && problems.is_empty());
    }

    #[test]
    fn a_missing_key_takes_its_factory_value() {
        let (read, problems) = parse("[line.3]\ns0 = 4\necho = false\n", "p");
        assert!(problems.is_empty(), "{problems:?}");
        let mut expected = Profile::factory(4);
        expected.echo = false;
        assert_eq!(read[&3], expected);
    }

    #[test]
    fn what_is_wrong_is_a_warning_and_what_is_right_is_kept() {
        let text = "\
[line.1]
s0 = 2

[line.2]
x = 9

[line.3]
echo = \"yes\"

[line.4]
s8 = 1

[line.5]
s3 = 200

[line.6]
ms = \"V21\\r\\nOK\"

[line.0]
s0 = 1

[line.front]
s0 = 1

[line.7]
s1 = 3

[line.8]
s07 = 3

[line.9]
turbo = true
";
        let (read, problems) = parse(text, "C:\\p.toml");
        assert_eq!(read.keys().copied().collect::<Vec<_>>(), [1]);
        assert_eq!(read[&1].register(0), Some(2));
        assert_eq!(problems.len(), 10, "{problems:?}");
        for problem in &problems {
            assert_eq!(problem.id, "RMD0026");
            assert_eq!(problem.severity, msg::Severity::Warning);
            assert!(problem.text.contains("C:\\p.toml"));
        }
        let texts: Vec<&str> = problems.iter().map(|p| p.text.as_str()).collect();
        assert!(
            texts[0].contains("line 2") && texts[0].ends_with("X has a value that is not allowed")
        );
        assert!(texts[1].ends_with("echo is not true or false"));
        assert!(texts[2].ends_with("there is no setting s8"));
        assert!(texts[3].ends_with("S3 has a value that is not allowed"));
        assert!(texts[4].ends_with("+MS has a value that is not allowed"));
        assert!(texts[5].contains("line 0") && texts[5].contains("line id from 1 to 999"));
        assert!(texts[6].contains("line front"));
        assert!(texts[7].ends_with("there is no setting s1"));
        assert!(texts[8].ends_with("there is no setting s07"));
        assert!(texts[9].ends_with("there is no setting turbo"));

        let (read, problems) = parse("[line.1]\ns0 = 300\n", "p");
        assert!(read.is_empty());
        assert!(
            problems[0]
                .text
                .ends_with("s0 is not a number from 0 to 255")
        );
    }

    #[test]
    fn a_file_that_is_not_toml_is_one_warning_and_no_profiles() {
        let (read, problems) = parse("[line.1\ns0 = 2\n", "p.toml");
        assert!(read.is_empty());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "RMD0024");
        let (read, problems) = parse("line = 3\n", "p.toml");
        assert!(read.is_empty());
        assert_eq!(problems[0].id, "RMD0024");
        // A file with other things in it and no profiles is no problem.
        let (read, problems) = parse("# nothing yet\nother = 1\n", "p.toml");
        assert!(read.is_empty() && problems.is_empty());
    }

    #[test]
    fn profiles_are_kept_in_the_file_from_one_run_to_the_next() {
        let directory = temp_dir("runs");
        let path = ProfileFile::path_beside(&directory.join("rustmodem.toml"));

        // No file yet: no profiles, no problem, and nothing is written
        // until there is something to keep.
        let (mut file, problems) = ProfileFile::open(&path);
        assert!(problems.is_empty());
        assert!(file.load(1).is_none());
        file.remove(1).unwrap();
        assert!(!path.exists());

        file.store(1, &changed()).unwrap();
        file.store(2, &Profile::factory(0)).unwrap();
        assert!(path.exists());
        assert!(!directory.join("profiles.toml.new").exists());

        let (mut again, problems) = ProfileFile::open(&path);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(again.load(1), Some(changed()));
        assert_eq!(again.load(2), Some(Profile::factory(0)));
        assert_eq!(again.load(3), None);

        again.remove(1).unwrap();
        let (mut last, _) = ProfileFile::open(&path);
        assert_eq!(last.load(1), None);
        assert!(last.load(2).is_some());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_written_is_message_0025_and_the_profile_is_kept() {
        let directory = temp_dir("blocked");
        // A directory where the file should be.
        let path = directory.join("profiles.toml");
        std::fs::create_dir_all(&path).unwrap();
        let (mut file, problems) = ProfileFile::open(&path);
        // Reading a directory as a file fails; that is a warning.
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].id, "RMD0024");
        let problem = file.store(1, &changed()).unwrap_err();
        assert_eq!(problem.id, "RMD0025");
        assert_eq!(file.load(1), Some(changed()));
        assert!(!directory.join("profiles.toml.new").exists());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
