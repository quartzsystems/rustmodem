// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The codecs against the tables of G.711 (Blue Book, 11/1988).
//!
//! The crate computes its tables from a closed form. These tests rebuild
//! the tables of the Recommendation the way it prints them, by walking the
//! segments of column 2, and hold every figure the tables print in full
//! as a literal.

use g711::{ALAW_OVERLOAD, Law, ULAW_OVERLOAD, alaw_to_ulaw, ulaw_to_alaw};

/// Column 2 of Table 2a/G.711: the number of intervals and their size,
/// segment by segment from the smallest.
const ULAW_SEGMENTS: [(usize, i32); 9] = [
    (1, 1),
    (15, 2),
    (16, 4),
    (16, 8),
    (16, 16),
    (16, 32),
    (16, 64),
    (16, 128),
    (16, 256),
];

/// Column 2 of Table 1a/G.711.
const ALAW_SEGMENTS: [(usize, i32); 7] = [
    (32, 2),
    (16, 4),
    (16, 8),
    (16, 16),
    (16, 32),
    (16, 64),
    (16, 128),
];

/// The 129 decision values a list of segments makes, from zero.
fn decisions(segments: &[(usize, i32)]) -> Vec<i32> {
    let mut values = vec![0];
    for &(count, size) in segments {
        for _ in 0..count {
            values.push(values.last().unwrap() + size);
        }
    }
    assert_eq!(values.len(), 129);
    values
}

/// The quantized values of the u-law, by interval (Note 3 of Table
/// 2a/G.711).
fn ulaw_quantized() -> Vec<i32> {
    let x = decisions(&ULAW_SEGMENTS);
    (0..128)
        .map(|n| if n == 0 { 0 } else { (x[n] + x[n + 1]) / 2 })
        .collect()
}

/// The quantized values of the A-law, by interval (Note 3 of Table
/// 1a/G.711, which numbers them from 1).
fn alaw_quantized() -> Vec<i32> {
    let x = decisions(&ALAW_SEGMENTS);
    (0..128).map(|n| (x[n] + x[n + 1]) / 2).collect()
}

/// The character signal as the tables print it: bit 1 first.
fn bits(text: &str) -> u8 {
    u8::from_str_radix(text, 2).unwrap()
}

#[test]
fn the_walk_gives_the_decision_values_the_tables_print() {
    // Table 2a/G.711, columns 4 and 5.
    let x = decisions(&ULAW_SEGMENTS);
    for (n, value) in [
        (0, 0),
        (1, 1),
        (2, 3),
        (16, 31),
        (17, 35),
        (32, 95),
        (33, 103),
        (48, 223),
        (49, 239),
        (64, 479),
        (65, 511),
        (80, 991),
        (81, 1055),
        (96, 2015),
        (97, 2143),
        (112, 4063),
        (113, 4319),
        (126, 7647),
        (127, 7903),
        (128, 8159),
    ] {
        assert_eq!(x[n], value, "u-law decision value {n}");
    }
    // Table 1a/G.711, columns 4 and 5.
    let x = decisions(&ALAW_SEGMENTS);
    for (n, value) in [
        (0, 0),
        (1, 2),
        (32, 64),
        (33, 68),
        (48, 128),
        (49, 136),
        (64, 256),
        (65, 272),
        (80, 512),
        (81, 544),
        (96, 1024),
        (97, 1088),
        (112, 2048),
        (113, 2176),
        (127, 3968),
        (128, 4096),
    ] {
        assert_eq!(x[n], value, "A-law decision value {n}");
    }
}

#[test]
fn every_ulaw_octet_decodes_to_the_value_of_table_2() {
    let y = ulaw_quantized();
    for (n, &value) in y.iter().enumerate() {
        // Note 2 of Tables 2a/G.711 and 2b/G.711.
        let positive = (255 - n) as u8;
        let negative = (127 - n) as u8;
        assert_eq!(i32::from(Law::Ulaw.decode_uniform(positive)), value, "{n}");
        assert_eq!(i32::from(Law::Ulaw.decode_uniform(negative)), -value, "{n}");
    }
}

#[test]
fn every_alaw_octet_decodes_to_the_value_of_table_1() {
    let y = alaw_quantized();
    for (n, &value) in y.iter().enumerate() {
        // Note 2 of Tables 1a/G.711 and 1b/G.711: 128 + n and n, and then
        // the even bits inverted.
        let positive = (128 + n) as u8 ^ 0x55;
        let negative = n as u8 ^ 0x55;
        assert_eq!(i32::from(Law::Alaw.decode_uniform(positive)), value, "{n}");
        assert_eq!(i32::from(Law::Alaw.decode_uniform(negative)), -value, "{n}");
    }
}

#[test]
fn the_rows_the_tables_print_in_full_decode_as_printed() {
    // Table 2a/G.711, columns 6 and 7, and Table 2b/G.711.
    for (character, value) in [
        ("10000000", 8031),
        ("10001111", 4191),
        ("10011111", 2079),
        ("10101111", 1023),
        ("10111111", 495),
        ("11001111", 231),
        ("11011111", 99),
        ("11101111", 33),
        ("11111110", 2),
        ("11111111", 0),
        ("01111111", 0),
        ("01111110", -2),
        ("01101111", -33),
        ("01011111", -99),
        ("01001111", -231),
        ("00111111", -495),
        ("00101111", -1023),
        ("00011111", -2079),
        ("00001111", -4191),
        ("00000001", -7775),
        ("00000000", -8031),
    ] {
        assert_eq!(
            Law::Ulaw.decode_uniform(bits(character)),
            value,
            "u-law {character}"
        );
    }
    // Table 1a/G.711, columns 6 and 7, and Table 1b/G.711. Column 6 is the
    // character signal before the inversion of the even bits.
    for (character, value) in [
        ("11111111", 4032),
        ("11110000", 2112),
        ("11100000", 1056),
        ("11010000", 528),
        ("11000000", 264),
        ("10110000", 132),
        ("10100000", 66),
        ("10000000", 1),
        ("00000000", -1),
        ("00100000", -66),
        ("00110000", -132),
        ("01000000", -264),
        ("01010000", -528),
        ("01100000", -1056),
        ("01110000", -2112),
        ("01111111", -4032),
    ] {
        assert_eq!(
            Law::Alaw.decode_uniform(bits(character) ^ 0x55),
            value,
            "A-law {character}"
        );
    }
}

#[test]
fn the_float_scale_has_one_at_the_overload_point() {
    for octet in 0..=255u8 {
        let uniform = f32::from(Law::Ulaw.decode_uniform(octet));
        assert_eq!(Law::Ulaw.decode(octet), uniform / 8159.0);
        let uniform = f32::from(Law::Alaw.decode_uniform(octet));
        assert_eq!(Law::Alaw.decode(octet), uniform / 4096.0);
    }
    assert_eq!(Law::Ulaw.overload(), ULAW_OVERLOAD);
    assert_eq!(Law::Alaw.overload(), ALAW_OVERLOAD);
    assert!(Law::Ulaw.decode(0x80) < 1.0 && Law::Ulaw.decode(0x00) > -1.0);
    assert!(Law::Alaw.decode(0xFF ^ 0x55) < 1.0);
}

/// The octet whose decoded value is nearest to `value`. Of two as near, the
/// one further from zero: a decision value belongs to the interval above
/// it, as "between decision values n and n + 1" numbers the intervals.
fn nearest(law: Law, value: i32) -> i32 {
    (0..=255u8)
        .map(|octet| i32::from(law.decode_uniform(octet)))
        .min_by_key(|&decoded| {
            let distance = (decoded - value).abs();
            // Zero is a positive input.
            let wrong_sign = (decoded < 0) != (value < 0);
            (distance, -decoded.abs(), wrong_sign)
        })
        .unwrap()
}

#[test]
fn every_input_encodes_to_the_octet_of_its_interval() {
    // The definition: an input between decision values n and n + 1 is
    // sent as the character signal of interval n. Every whole input of
    // the uniform scale, and some beyond the overload point.
    for (law, x, y) in [
        (Law::Ulaw, decisions(&ULAW_SEGMENTS), ulaw_quantized()),
        (Law::Alaw, decisions(&ALAW_SEGMENTS), alaw_quantized()),
    ] {
        let overload = i32::from(law.overload());
        assert_eq!(x[128], overload);
        let mut not_nearest = 0;
        for value in -overload - 40..=overload + 40 {
            let magnitude = value.abs().min(overload - 1);
            let n = (0..128).find(|&n| magnitude < x[n + 1]).unwrap();
            assert!(x[n] <= magnitude);
            let expected = if value < 0 { -y[n] } else { y[n] };
            let octet = law.encode_uniform(value as i16);
            let decoded = i32::from(law.decode_uniform(octet));
            assert_eq!(decoded, expected, "{law:?} {value}");

            // That is the nearest value there is, but for an input just
            // above the first decision value of a segment: the interval
            // below is half as wide, so its value is the nearer for a
            // little way into the wider one. G.711 defines the law by its
            // decision values, and they are what is followed.
            let clamped = value.clamp(-(overload - 1), overload - 1);
            if decoded != nearest(law, clamped) {
                not_nearest += 1;
                assert!(
                    n > 0 && x[n + 1] - x[n] > x[n] - x[n - 1],
                    "{law:?} {value}"
                );
                assert!(4 * (magnitude - x[n]) < x[n + 1] - x[n], "{law:?} {value}");
            }
        }
        assert!(not_nearest > 0, "{law:?}");
    }
}

#[test]
fn an_input_between_two_decision_values_gets_that_interval() {
    // The definition again, on the float scale, at points inside each
    // interval that are not whole numbers.
    let x = decisions(&ULAW_SEGMENTS);
    for n in 0..128usize {
        for fraction in [0.01f32, 0.25, 0.5, 0.99] {
            let width = (x[n + 1] - x[n]) as f32;
            let input = (x[n] as f32 + fraction * width) / 8159.0;
            assert_eq!(Law::Ulaw.encode(input), (255 - n) as u8, "{n} {fraction}");
            if input > 0.0 {
                assert_eq!(Law::Ulaw.encode(-input), (127 - n) as u8, "{n}");
            }
        }
    }
    let x = decisions(&ALAW_SEGMENTS);
    for n in 0..128usize {
        for fraction in [0.01f32, 0.25, 0.5, 0.99] {
            let width = (x[n + 1] - x[n]) as f32;
            let input = (x[n] as f32 + fraction * width) / 4096.0;
            assert_eq!(Law::Alaw.encode(input), (128 + n) as u8 ^ 0x55, "{n}");
            if input > 0.0 {
                assert_eq!(Law::Alaw.encode(-input), n as u8 ^ 0x55, "{n}");
            }
        }
    }
}

#[test]
fn decode_then_encode_is_the_identity_on_every_octet() {
    for law in [Law::Ulaw, Law::Alaw] {
        for octet in 0..=255u8 {
            assert_eq!(law.encode(law.decode(octet)), octet, "{law:?} {octet:#04x}");
        }
    }
    // On the uniform scale too, but for the u-law's second zero, which an
    // integer cannot tell from the first.
    for law in [Law::Ulaw, Law::Alaw] {
        for octet in 0..=255u8 {
            let back = law.encode_uniform(law.decode_uniform(octet));
            if law == Law::Ulaw && octet == 0x7F {
                assert_eq!(back, 0xFF);
            } else {
                assert_eq!(back, octet, "{law:?} {octet:#04x}");
            }
        }
    }
}

#[test]
fn inputs_beyond_the_overload_point_and_not_numbers_are_safe() {
    for law in [Law::Ulaw, Law::Alaw] {
        let top = law.encode(0.9999);
        let bottom = law.encode(-0.9999);
        assert_eq!(law.encode(1.0), top);
        assert_eq!(law.encode(7.5), top);
        assert_eq!(law.encode(f32::INFINITY), top);
        assert_eq!(law.encode(f32::MAX), top);
        assert_eq!(law.encode(-1.0), bottom);
        assert_eq!(law.encode(f32::NEG_INFINITY), bottom);
        assert_eq!(law.encode(f32::NAN), law.silence());
        assert_eq!(law.encode_uniform(i16::MAX), top);
        assert_eq!(law.encode_uniform(i16::MIN), bottom);
    }
    assert_eq!(Law::Ulaw.encode(1.0), 0x80);
    assert_eq!(Law::Ulaw.encode(-1.0), 0x00);
    assert_eq!(Law::Alaw.encode(1.0), 0xFF ^ 0x55);
    assert_eq!(Law::Alaw.encode(-1.0), 0x7F ^ 0x55);
}

#[test]
fn the_idle_octets_of_the_rtp_layer_are_silence() {
    // 0xFF and 0xD5 are what `voip/rtp` fills a lost packet with.
    assert_eq!(Law::Ulaw.silence(), 0xFF);
    assert_eq!(Law::Alaw.silence(), 0xD5);
    assert_eq!(Law::Ulaw.decode(0xFF), 0.0);
    assert_eq!(Law::Ulaw.decode_uniform(0xFF), 0);
    assert_eq!(Law::Ulaw.encode(0.0), 0xFF);
    assert_eq!(Law::Alaw.encode(0.0), 0xD5);
    // The A-law has no zero. Its idle octet is the smallest value there
    // is: one unit of 4096, which is 72 dB below the overload point.
    assert_eq!(Law::Alaw.decode_uniform(0xD5), 1);
    assert_eq!(Law::Alaw.decode(0xD5), 1.0 / 4096.0);
    for octet in 0..=255u8 {
        assert!(Law::Alaw.decode(octet).abs() >= Law::Alaw.decode(0xD5));
    }
}

#[test]
fn slices_are_coded_octet_by_octet() {
    let octets: Vec<u8> = (0..=255).collect();
    for law in [Law::Ulaw, Law::Alaw] {
        let mut samples = [0f32; 256];
        law.decode_slice(&octets, &mut samples);
        let mut back = [0u8; 256];
        law.encode_slice(&samples, &mut back);
        assert_eq!(&back[..], &octets[..]);
    }
}

/// The level in dB, relative to a sine wave with its peaks at the overload
/// point, of a periodic sequence of octets.
fn level_db(law: Law, sequence: &[u8]) -> f64 {
    let power: f64 = sequence
        .iter()
        .map(|&octet| f64::from(law.decode(octet)).powi(2))
        .sum::<f64>()
        / sequence.len() as f64;
    10.0 * (power / 0.5).log10()
}

#[test]
fn the_reference_sequences_are_at_0_dbm0() {
    // Tables 5/G.711 and 6/G.711: the periodic sequences that are a 1 kHz
    // sine wave at 0 dBm0. Against a sine wave at the overload point they
    // are therefore Tmax down (G.711 clause 5).
    let alaw = [
        "00110100", "00100001", "00100001", "00110100", "10110100", "10100001", "10100001",
        "10110100",
    ]
    .map(bits);
    let ulaw = [
        "00011110", "00001011", "00001011", "00011110", "10011110", "10001011", "10001011",
        "10011110",
    ]
    .map(bits);
    let a = level_db(Law::Alaw, &alaw);
    let u = level_db(Law::Ulaw, &ulaw);
    assert!(
        (a + f64::from(Law::Alaw.overload_dbm0())).abs() < 0.005,
        "{a}"
    );
    assert!(
        (u + f64::from(Law::Ulaw.overload_dbm0())).abs() < 0.005,
        "{u}"
    );
    // The figure of the Recommendation, which is not pi.
    #[allow(clippy::approx_constant)]
    let alaw_tmax = 3.14;
    assert_eq!(Law::Alaw.overload_dbm0(), alaw_tmax);
    assert_eq!(Law::Ulaw.overload_dbm0(), 3.17);
}

/// Table 3/G.711 as printed: u-law decoder output value number to A-law.
fn table_3(ulaw: u8) -> u8 {
    const PRINTED: [(u8, u8); 88] = [
        (0, 1),
        (1, 1),
        (2, 2),
        (3, 2),
        (4, 3),
        (5, 3),
        (6, 4),
        (7, 4),
        (8, 5),
        (9, 5),
        (10, 6),
        (11, 6),
        (12, 7),
        (13, 7),
        (14, 8),
        (15, 8),
        (16, 9),
        (17, 10),
        (18, 11),
        (19, 12),
        (20, 13),
        (21, 14),
        (22, 15),
        (23, 16),
        (24, 17),
        (25, 18),
        (26, 19),
        (27, 20),
        (28, 21),
        (29, 22),
        (30, 23),
        (31, 24),
        (32, 25),
        (33, 27),
        (34, 29),
        (35, 31),
        (36, 33),
        (37, 34),
        (38, 35),
        (39, 36),
        (40, 37),
        (41, 38),
        (42, 39),
        (43, 40),
        (44, 41),
        (45, 42),
        (46, 43),
        (47, 44),
        (48, 46),
        (49, 48),
        (50, 49),
        (51, 50),
        (52, 51),
        (53, 52),
        (54, 53),
        (55, 54),
        (56, 55),
        (57, 56),
        (58, 57),
        (59, 58),
        (60, 59),
        (61, 60),
        (62, 61),
        (63, 62),
        (64, 64),
        (65, 65),
        (66, 66),
        (67, 67),
        (68, 68),
        (69, 69),
        (70, 70),
        (71, 71),
        (72, 72),
        (73, 73),
        (74, 74),
        (75, 75),
        (76, 76),
        (77, 77),
        (78, 78),
        (79, 79),
        (80, 81),
        (81, 82),
        (82, 83),
        (83, 84),
        (84, 85),
        (85, 86),
        (86, 87),
        (87, 88),
    ];
    match PRINTED.iter().find(|row| row.0 == ulaw) {
        Some(row) => row.1,
        // "... 127 to 128".
        None => ulaw + 1,
    }
}

/// Table 4/G.711 as printed: A-law decoder output value number to u-law.
fn table_4(alaw: u8) -> u8 {
    const PRINTED: [(u8, u8); 98] = [
        (1, 1),
        (2, 3),
        (3, 5),
        (4, 7),
        (5, 9),
        (6, 11),
        (7, 13),
        (8, 15),
        (9, 16),
        (10, 17),
        (11, 18),
        (12, 19),
        (13, 20),
        (14, 21),
        (15, 22),
        (16, 23),
        (17, 24),
        (18, 25),
        (19, 26),
        (20, 27),
        (21, 28),
        (22, 29),
        (23, 30),
        (24, 31),
        (25, 32),
        (26, 32),
        (27, 33),
        (28, 33),
        (29, 34),
        (30, 34),
        (31, 35),
        (32, 35),
        (33, 36),
        (34, 37),
        (35, 38),
        (36, 39),
        (37, 40),
        (38, 41),
        (39, 42),
        (40, 43),
        (41, 44),
        (42, 45),
        (43, 46),
        (44, 47),
        (45, 48),
        (46, 48),
        (47, 49),
        (48, 49),
        (49, 50),
        (50, 51),
        (51, 52),
        (52, 53),
        (53, 54),
        (54, 55),
        (55, 56),
        (56, 57),
        (57, 58),
        (58, 59),
        (59, 60),
        (60, 61),
        (61, 62),
        (62, 63),
        (63, 64),
        (64, 64),
        (65, 65),
        (66, 66),
        (67, 67),
        (68, 68),
        (69, 69),
        (70, 70),
        (71, 71),
        (72, 72),
        (73, 73),
        (74, 74),
        (75, 75),
        (76, 76),
        (77, 77),
        (78, 78),
        (79, 79),
        (80, 79),
        (81, 80),
        (82, 81),
        (83, 82),
        (84, 83),
        (85, 84),
        (86, 85),
        (87, 86),
        (88, 87),
        (89, 88),
        (90, 89),
        (91, 90),
        (92, 91),
        (93, 92),
        (94, 93),
        (95, 94),
        (96, 95),
        (97, 96),
        (98, 97),
    ];
    match PRINTED.iter().find(|row| row.0 == alaw) {
        Some(row) => row.1,
        // "... 128 to 127".
        None => alaw - 1,
    }
}

/// The decoder output value number and sign of a u-law octet (column 8 of
/// Tables 2a/G.711 and 2b/G.711).
fn ulaw_number(octet: u8) -> (bool, u8) {
    if octet >= 128 {
        (false, 255 - octet)
    } else {
        (true, 127 - octet)
    }
}

fn ulaw_octet(negative: bool, number: u8) -> u8 {
    if negative { 127 - number } else { 255 - number }
}

/// The decoder output value number and sign of an A-law octet (column 8 of
/// Tables 1a/G.711 and 1b/G.711).
fn alaw_number(octet: u8) -> (bool, u8) {
    let plain = octet ^ 0x55;
    if plain >= 128 {
        (false, plain - 128 + 1)
    } else {
        (true, plain + 1)
    }
}

fn alaw_octet(negative: bool, number: u8) -> u8 {
    (if negative { number - 1 } else { 127 + number }) ^ 0x55
}

#[test]
fn ulaw_to_alaw_is_table_3() {
    for octet in 0..=255u8 {
        let (negative, number) = ulaw_number(octet);
        assert_eq!(
            ulaw_to_alaw(octet),
            alaw_octet(negative, table_3(number)),
            "{octet:#04x}"
        );
    }
}

#[test]
fn alaw_to_ulaw_is_table_4() {
    for octet in 0..=255u8 {
        let (negative, number) = alaw_number(octet);
        assert_eq!(
            alaw_to_ulaw(octet),
            ulaw_octet(negative, table_4(number)),
            "{octet:#04x}"
        );
    }
}

#[test]
fn a_conversion_there_and_back_changes_what_note_2_says_and_nothing_else() {
    // Note 2 of Table 3/G.711: after u-law to A-law to u-law only the
    // u-law decoder output value numbers 0, 2, 4, 6, 8, 10, 12 and 14 are
    // changed, each increased by 1, and only bit 8 of the octet differs.
    for octet in 0..=255u8 {
        let back = alaw_to_ulaw(ulaw_to_alaw(octet));
        let (negative, number) = ulaw_number(octet);
        if number <= 14 && number % 2 == 0 {
            assert_eq!(ulaw_number(back), (negative, number + 1), "{octet:#04x}");
            assert_eq!(back ^ octet, 0x01);
        } else {
            assert_eq!(back, octet, "{octet:#04x}");
        }
    }
    // And after A-law to u-law to A-law only the A-law numbers 26, 28, 30,
    // 32, 45, 47, 63 and 80, again in bit 8 alone.
    for octet in 0..=255u8 {
        let back = ulaw_to_alaw(alaw_to_ulaw(octet));
        let (_, number) = alaw_number(octet);
        if [26, 28, 30, 32, 45, 47, 63, 80].contains(&number) {
            assert_eq!(back ^ octet, 0x01, "{octet:#04x}");
        } else {
            assert_eq!(back, octet, "{octet:#04x}");
        }
    }
}

#[test]
fn a_conversion_keeps_the_sign_and_stays_near_the_level() {
    for octet in 0..=255u8 {
        let before = Law::Ulaw.decode(octet);
        let after = Law::Alaw.decode(ulaw_to_alaw(octet));
        // The u-law's zero becomes the A-law's smallest value.
        assert!(before == 0.0 || (before < 0.0) == (after < 0.0));
        // The two scales differ by the 0.03 dB between the two overload
        // points, and a conversion lands within an interval of the value.
        assert!(
            (before - after).abs() <= 0.035 * before.abs() + 2.0 / 4096.0,
            "{octet:#04x}: {before} to {after}"
        );
    }
}
