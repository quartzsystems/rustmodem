// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The message table the service carries for the Event Viewer.

use glyph::message_table;

fn word(table: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(table[at..at + 4].try_into().unwrap())
}

/// Looks an id up as Windows does: the block that has it, then entry by
/// entry to its place.
fn text_of(table: &[u8], id: u32) -> Option<String> {
    for block in 0..word(table, 0) as usize {
        let at = 4 + block * 12;
        let (low, high, mut entry) = (
            word(table, at),
            word(table, at + 4),
            word(table, at + 8) as usize,
        );
        if !(low..=high).contains(&id) {
            continue;
        }
        for _ in low..id {
            entry += u16::from_le_bytes([table[entry], table[entry + 1]]) as usize;
        }
        let length = u16::from_le_bytes([table[entry], table[entry + 1]]) as usize;
        assert_eq!(length % 4, 0, "an entry is a multiple of four octets");
        assert_eq!(
            u16::from_le_bytes([table[entry + 2], table[entry + 3]]),
            1,
            "Unicode"
        );
        let units: Vec<u16> = table[entry + 4..entry + length]
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|&unit| unit != 0)
            .collect();
        return Some(String::from_utf16(&units).unwrap());
    }
    None
}

#[test]
fn every_id_given_has_the_text_and_no_other_id_has_any() {
    let ids = [1, 2, 3, 7, 3001, 3002, 9500, 2, 9999];
    let table = message_table(&ids, "%1\r\n");
    // Runs with no gap are one block: 1 to 3, 7, 3001 to 3002, 9500, 9999.
    assert_eq!(word(&table, 0), 5);
    for id in ids {
        assert_eq!(text_of(&table, id).as_deref(), Some("%1\r\n"), "id {id}");
    }
    for id in [0, 4, 6, 8, 3000, 3003, 9998, 10_000] {
        assert_eq!(text_of(&table, id), None, "id {id}");
    }
    // Five blocks of twelve octets after the count, then nine entries.
    let entry = (4usize + 2 * 5).next_multiple_of(4);
    assert_eq!(table.len(), 4 + 5 * 12 + 8 * entry);
}

#[test]
fn a_table_of_nothing_is_a_count_of_nothing() {
    assert_eq!(message_table(&[], "%1\r\n"), [0, 0, 0, 0]);
}
