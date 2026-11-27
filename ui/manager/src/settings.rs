// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! What the manager remembers between runs, per user: the widths of the
//! grid's columns, the sort, the filter, which bars are shown and where
//! the window is.
//!
//! It is kept under `HKEY_CURRENT_USER\Software\Quartz Systems\RustModem\
//! Manager`. Whatever is missing there, or is not what was written, is the
//! default: the registry is never trusted to hold something usable.

use model::{Column, Filter, SortOrder};
use win32::UserKey;

/// Where the settings are kept, under `HKEY_CURRENT_USER`.
pub const KEY: &str = r"Software\Quartz Systems\RustModem\Manager";

/// The narrowest and the widest a saved column may be, at 96 DPI.
const WIDTH_RANGE: std::ops::RangeInclusive<i32> = 10..=4000;

/// What is remembered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The width of each column of the grid, in pixels at 96 DPI.
    pub widths: Vec<i32>,
    /// How the grid is sorted.
    pub sort: SortOrder,
    /// Which lines the grid shows.
    pub filter: Filter,
    /// Whether the toolbar is shown.
    pub toolbar: bool,
    /// Whether the status bar is shown.
    pub status_bar: bool,
    /// Where the window is, as Windows gives it; empty when not known.
    pub placement: Vec<u8>,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            widths: Column::ALL
                .iter()
                .map(|column| column.default_width())
                .collect(),
            sort: SortOrder::default(),
            filter: Filter::All,
            toolbar: true,
            status_bar: true,
            placement: Vec::new(),
        }
    }
}

/// The widths as they are written: numbers with commas between them.
fn widths_text(widths: &[i32]) -> String {
    let parts: Vec<String> = widths.iter().map(|width| width.to_string()).collect();
    parts.join(",")
}

/// The widths that were written, if they are a width for every column.
fn parse_widths(text: &str) -> Option<Vec<i32>> {
    let widths: Vec<i32> = text
        .split(',')
        .map(|part| part.trim().parse::<i32>().ok())
        .collect::<Option<_>>()?;
    (widths.len() == Column::ALL.len() && widths.iter().all(|width| WIDTH_RANGE.contains(width)))
        .then_some(widths)
}

/// A placement that does not start the manager minimised: a window that
/// was closed while minimised comes back as it was before that.
///
/// A placement begins with three 32-bit numbers: its length, its flags
/// and how the window is shown.
fn restored(mut placement: Vec<u8>) -> Vec<u8> {
    const SHOW_NORMAL: u32 = 1;
    const SHOW_MINIMIZED: u32 = 2;
    const SHOW_MAXIMIZED: u32 = 3;
    /// The window was maximised before it was minimised.
    const RESTORE_TO_MAXIMIZED: u32 = 2;
    let number = |bytes: &[u8], at: usize| {
        bytes
            .get(at..at + 4)
            .and_then(|four| four.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let (Some(flags), Some(show)) = (number(&placement, 4), number(&placement, 8)) else {
        return placement;
    };
    if show == SHOW_MINIMIZED {
        let show = if flags & RESTORE_TO_MAXIMIZED != 0 {
            SHOW_MAXIMIZED
        } else {
            SHOW_NORMAL
        };
        placement[8..12].copy_from_slice(&show.to_le_bytes());
    }
    placement
}

/// Reads what was saved under `key`. With no key, or nothing saved, the
/// defaults.
pub fn load(key: Option<&str>) -> Saved {
    let mut saved = Saved::default();
    let Some(key) = key.and_then(UserKey::open) else {
        return saved;
    };
    if let Some(widths) = key
        .string("ColumnWidths")
        .and_then(|text| parse_widths(&text))
    {
        saved.widths = widths;
    }
    if let Some(column) = key
        .number("SortColumn")
        .and_then(|index| Column::at(index as usize))
    {
        saved.sort = SortOrder {
            column,
            descending: key.number("SortDescending") == Some(1),
        };
    }
    if let Some(filter) = key
        .number("Filter")
        .and_then(|index| Filter::at(index as usize))
    {
        saved.filter = filter;
    }
    if let Some(shown) = key.number("Toolbar") {
        saved.toolbar = shown != 0;
    }
    if let Some(shown) = key.number("StatusBar") {
        saved.status_bar = shown != 0;
    }
    if let Some(placement) = key.bytes("Placement") {
        saved.placement = restored(placement);
    }
    saved
}

/// Writes what is to be remembered under `key`. With no key, nothing.
pub fn save(key: Option<&str>, saved: &Saved) {
    let Some(key) = key.and_then(UserKey::open) else {
        return;
    };
    key.set_string("ColumnWidths", &widths_text(&saved.widths));
    key.set_number("SortColumn", saved.sort.column.index() as u32);
    key.set_number("SortDescending", u32::from(saved.sort.descending));
    key.set_number("Filter", saved.filter.index() as u32);
    key.set_number("Toolbar", u32::from(saved.toolbar));
    key.set_number("StatusBar", u32::from(saved.status_bar));
    if !saved.placement.is_empty() {
        key.set_bytes("Placement", &saved.placement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of this test's own, removed when the test ends.
    struct TestKey(String);

    impl TestKey {
        fn new(name: &str) -> TestKey {
            let path = format!(
                r"Software\Quartz Systems\RustModem\Test\manager-{name}-{}",
                std::process::id()
            );
            UserKey::delete(&path);
            TestKey(path)
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            UserKey::delete(&self.0);
        }
    }

    #[test]
    fn with_nothing_saved_everything_is_the_default() {
        let key = TestKey::new("empty");
        let saved = load(Some(&key.0));
        assert_eq!(saved, Saved::default());
        assert_eq!(saved.widths[..3], [48, 70, 60]);
        assert_eq!(saved.widths.len(), 13);
        assert!(saved.toolbar && saved.status_bar);
        assert_eq!(load(None), Saved::default());
    }

    #[test]
    fn what_is_saved_is_what_is_loaded() {
        let key = TestKey::new("round-trip");
        let mut saved = Saved::default();
        saved.widths[4] = 180;
        saved.widths[12] = 420;
        saved.sort = SortOrder {
            column: Column::State,
            descending: true,
        };
        saved.filter = Filter::OnCall;
        saved.toolbar = false;
        saved.status_bar = false;
        saved.placement = vec![44, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 9, 9];
        save(Some(&key.0), &saved);
        assert_eq!(load(Some(&key.0)), saved);

        // Saving nowhere changes nothing.
        save(None, &Saved::default());
        assert_eq!(load(Some(&key.0)), saved);
    }

    #[test]
    fn what_was_not_written_by_the_manager_is_not_used() {
        let key = TestKey::new("nonsense");
        let registry = UserKey::open(&key.0).unwrap();
        registry.set_string("ColumnWidths", "30,70,many");
        registry.set_number("SortColumn", 99);
        registry.set_number("SortDescending", 1);
        registry.set_number("Filter", 99);
        let saved = load(Some(&key.0));
        assert_eq!(saved, Saved::default());
    }

    #[test]
    fn widths_are_a_width_for_every_column_or_nothing() {
        let defaults = Saved::default().widths;
        let text = widths_text(&defaults);
        assert_eq!(text, "48,70,60,80,100,110,90,90,70,110,45,40,200");
        assert_eq!(parse_widths(&text), Some(defaults));
        assert_eq!(parse_widths("30, 70"), None);
        assert_eq!(parse_widths(""), None);
        assert_eq!(
            parse_widths("0,70,60,80,100,110,90,90,70,110,45,40,200"),
            None
        );
        assert_eq!(
            parse_widths("30,70,60,80,100,110,90,90,70,110,45,40,99999"),
            None
        );
    }

    #[test]
    fn a_window_closed_while_minimised_does_not_come_back_minimised() {
        let placement = |flags: u32, show: u32| {
            let mut bytes = Vec::new();
            for number in [44u32, flags, show, 7, 7] {
                bytes.extend_from_slice(&number.to_le_bytes());
            }
            bytes
        };
        assert_eq!(restored(placement(0, 2)), placement(0, 1));
        assert_eq!(restored(placement(2, 2)), placement(2, 3));
        assert_eq!(restored(placement(0, 1)), placement(0, 1));
        assert_eq!(restored(placement(0, 3)), placement(0, 3));
        // Too short to be a placement: left alone, and refused later.
        assert_eq!(restored(vec![1, 2, 3]), vec![1, 2, 3]);
        assert_eq!(restored(Vec::new()), Vec::<u8>::new());
    }
}
