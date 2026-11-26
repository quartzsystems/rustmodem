// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! A PNG writer, enough for icons: 8-bit RGBA, no interlace.
//!
//! It is here so that this crate has no dependencies. The compression is
//! deliberately simple and still good for what icons are made of: each row
//! is stored as the difference from the pixel before (the PNG "Sub"
//! filter), which turns flat areas into runs of zeros, and the runs are
//! written as DEFLATE matches at distance one with the fixed Huffman
//! codes (RFC 1951, clause 3.2.6).

/// The eight bytes every PNG file starts with.
pub const SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";

/// Bytes per pixel.
const BPP: usize = 4;

/// Encodes `width * height * 4` bytes of red, green, blue and alpha as a
/// PNG file.
pub fn encode(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    assert_eq!(
        rgba.len(),
        width as usize * height as usize * BPP,
        "the pixels are width * height * 4 bytes"
    );
    let mut out = SIGNATURE.to_vec();
    let mut header = Vec::new();
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // Bit depth 8, colour type 6 (RGBA), deflate, adaptive filtering, no
    // interlace.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib(&filtered(rgba, width as usize)));
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The rows with the Sub filter applied: each row is a filter type byte,
/// then every byte less the byte one pixel to its left.
fn filtered(rgba: &[u8], width: usize) -> Vec<u8> {
    let row_bytes = width * BPP;
    let mut out = Vec::with_capacity(rgba.len() + rgba.len() / row_bytes.max(1));
    for row in rgba.chunks(row_bytes.max(1)) {
        out.push(1);
        for (index, byte) in row.iter().enumerate() {
            let left = if index >= BPP { row[index - BPP] } else { 0 };
            out.push(byte.wrapping_sub(left));
        }
    }
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let check = crc32(&out[start..]);
    out.extend_from_slice(&check.to_be_bytes());
}

/// The CRC of PNG chunks (ISO 3309, as the PNG specification gives it).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The checksum of a zlib stream (RFC 1950).
fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for byte in data {
        a = (a + u32::from(*byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// A zlib stream holding `data`.
fn zlib(data: &[u8]) -> Vec<u8> {
    // Deflate with a 32 K window; the check bits make the pair a multiple
    // of 31.
    let mut out = vec![0x78, 0x01];
    out.extend_from_slice(&deflate(data));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Bits written least significant first, as DEFLATE packs them.
struct Bits {
    out: Vec<u8>,
    held: u32,
    count: u32,
}

impl Bits {
    /// Writes the low `count` bits of `value`, lowest first.
    fn put(&mut self, value: u32, count: u32) {
        self.held |= value << self.count;
        self.count += count;
        while self.count >= 8 {
            self.out.push(self.held as u8);
            self.held >>= 8;
            self.count -= 8;
        }
    }

    /// Writes a Huffman code, which goes highest bit first.
    fn code(&mut self, code: u32, length: u32) {
        let reversed = code.reverse_bits() >> (32 - length);
        self.put(reversed, length);
    }

    /// A literal byte or the end-of-block symbol, in the fixed code.
    fn symbol(&mut self, symbol: u32) {
        match symbol {
            0..=143 => self.code(0b0011_0000 + symbol, 8),
            144..=255 => self.code(0b1_1001_0000 + (symbol - 144), 9),
            256..=279 => self.code(symbol - 256, 7),
            _ => self.code(0b1100_0000 + (symbol - 280), 8),
        }
    }

    /// A match of `length` bytes, 3 to 258, at distance one.
    fn run(&mut self, length: u32) {
        // The length codes: the first length of each, and its extra bits.
        const BASE: [(u32, u32); 29] = [
            (3, 0),
            (4, 0),
            (5, 0),
            (6, 0),
            (7, 0),
            (8, 0),
            (9, 0),
            (10, 0),
            (11, 1),
            (13, 1),
            (15, 1),
            (17, 1),
            (19, 2),
            (23, 2),
            (27, 2),
            (31, 2),
            (35, 3),
            (43, 3),
            (51, 3),
            (59, 3),
            (67, 4),
            (83, 4),
            (99, 4),
            (115, 4),
            (131, 5),
            (163, 5),
            (195, 5),
            (227, 5),
            (258, 0),
        ];
        let index = BASE
            .iter()
            .rposition(|(first, _)| *first <= length)
            .unwrap_or(0);
        let (first, extra) = BASE[index];
        self.symbol(257 + index as u32);
        self.put(length - first, extra);
        // Distance code 0 is distance one; the fixed distance codes are
        // five bits.
        self.code(0, 5);
    }
}

/// One DEFLATE block of `data` with the fixed codes: literals, and runs of
/// a repeated byte as matches at distance one.
fn deflate(data: &[u8]) -> Vec<u8> {
    let mut bits = Bits {
        out: Vec::new(),
        held: 0,
        count: 0,
    };
    // The last block, compressed with the fixed codes.
    bits.put(1, 1);
    bits.put(1, 2);
    let mut at = 0;
    while at < data.len() {
        let byte = data[at];
        bits.symbol(u32::from(byte));
        at += 1;
        let mut run = data[at..].iter().take_while(|b| **b == byte).count();
        while run >= 3 {
            let length = run.min(258);
            bits.run(length as u32);
            at += length;
            run -= length;
        }
    }
    bits.symbol(256);
    // Flush what is held.
    bits.put(0, 7);
    bits.out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Reads DEFLATE bits, for checking what the writer wrote.
    struct Reader<'a> {
        data: &'a [u8],
        at: usize,
    }

    impl Reader<'_> {
        fn bit(&mut self) -> u32 {
            let bit = (self.data[self.at / 8] >> (self.at % 8)) & 1;
            self.at += 1;
            u32::from(bit)
        }

        fn bits(&mut self, count: u32) -> u32 {
            (0..count).fold(0, |value, index| value | (self.bit() << index))
        }

        /// A symbol of the fixed literal and length code.
        fn symbol(&mut self) -> u32 {
            let mut code = 0;
            for length in 1..=9 {
                code = (code << 1) | self.bit();
                match length {
                    7 if code <= 0b001_0111 => return 256 + code,
                    8 if (0b0011_0000..=0b1011_1111).contains(&code) => return code - 0b0011_0000,
                    8 if (0b1100_0000..=0b1100_0111).contains(&code) => {
                        return 280 + code - 0b1100_0000;
                    }
                    9 => return 144 + code - 0b1_1001_0000,
                    _ => {}
                }
            }
            unreachable!("nine bits are always a symbol")
        }
    }

    /// Inflates one block of fixed codes with matches at distance one,
    /// which is all the writer writes.
    fn inflate(data: &[u8]) -> Vec<u8> {
        const BASE: [(u32, u32); 29] = [
            (3, 0),
            (4, 0),
            (5, 0),
            (6, 0),
            (7, 0),
            (8, 0),
            (9, 0),
            (10, 0),
            (11, 1),
            (13, 1),
            (15, 1),
            (17, 1),
            (19, 2),
            (23, 2),
            (27, 2),
            (31, 2),
            (35, 3),
            (43, 3),
            (51, 3),
            (59, 3),
            (67, 4),
            (83, 4),
            (99, 4),
            (115, 4),
            (131, 5),
            (163, 5),
            (195, 5),
            (227, 5),
            (258, 0),
        ];
        let mut reader = Reader { data, at: 0 };
        assert_eq!(reader.bits(1), 1, "the last block");
        assert_eq!(reader.bits(2), 1, "the fixed codes");
        let mut out = Vec::new();
        loop {
            match reader.symbol() {
                256 => return out,
                literal @ 0..=255 => out.push(literal as u8),
                length => {
                    let (first, extra) = BASE[(length - 257) as usize];
                    let count = first + reader.bits(extra);
                    assert_eq!(reader.bits(5), 0, "distance one");
                    let byte = *out.last().expect("a match follows a literal");
                    out.extend(std::iter::repeat_n(byte, count as usize));
                }
            }
        }
    }

    /// Decodes a PNG this crate wrote back to its pixels and size.
    pub(crate) fn decode(png: &[u8]) -> (Vec<u8>, u32, u32) {
        assert_eq!(png[..8], SIGNATURE);
        let mut at = 8;
        let (mut width, mut height) = (0, 0);
        let mut compressed = Vec::new();
        let mut kinds = Vec::new();
        while at < png.len() {
            let length = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png[at + 4..at + 8];
            let data = &png[at + 8..at + 8 + length];
            let check =
                u32::from_be_bytes(png[at + 8 + length..at + 12 + length].try_into().unwrap());
            assert_eq!(
                check,
                crc32(&png[at + 4..at + 8 + length]),
                "the chunk's CRC"
            );
            kinds.push(String::from_utf8_lossy(kind).into_owned());
            match kind {
                b"IHDR" => {
                    width = u32::from_be_bytes(data[..4].try_into().unwrap());
                    height = u32::from_be_bytes(data[4..8].try_into().unwrap());
                    assert_eq!(data[8..], [8, 6, 0, 0, 0]);
                }
                b"IDAT" => compressed.extend_from_slice(data),
                _ => {}
            }
            at += 12 + length;
        }
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
        assert_eq!(compressed[..2], [0x78, 0x01]);
        assert_eq!(u16::from_be_bytes([compressed[0], compressed[1]]) % 31, 0);
        let body = &compressed[2..compressed.len() - 4];
        let raw = inflate(body);
        let check = u32::from_be_bytes(compressed[compressed.len() - 4..].try_into().unwrap());
        assert_eq!(check, adler32(&raw), "the zlib checksum");

        // Undo the Sub filter.
        let row_bytes = width as usize * BPP;
        let mut pixels = Vec::new();
        for row in raw.chunks(row_bytes + 1) {
            assert_eq!(row[0], 1, "the Sub filter");
            let start = pixels.len();
            for (index, byte) in row[1..].iter().enumerate() {
                let left = if index >= BPP {
                    pixels[start + index - BPP]
                } else {
                    0
                };
                pixels.push(byte.wrapping_add(left));
            }
        }
        (pixels, width, height)
    }

    #[test]
    fn the_checksums_are_the_standard_ones() {
        // The check values the two specifications give.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn what_is_deflated_inflates_to_the_same() {
        let mut data: Vec<Vec<u8>> = vec![
            Vec::new(),
            vec![7],
            vec![0; 2],
            vec![0; 3],
            vec![9; 258],
            vec![9; 259],
            vec![9; 260],
            vec![9; 262],
            vec![0; 100_000],
            (0..=255).collect(),
            (0..2000u32).map(|i| (i * 7 % 251) as u8).collect(),
        ];
        // Every run length the length codes can spell, each after a
        // different byte.
        let mut runs = Vec::new();
        for length in 1..=300usize {
            runs.extend(std::iter::repeat_n((length % 250) as u8, length));
            runs.push(251);
        }
        data.push(runs);
        for bytes in data {
            assert_eq!(inflate(&deflate(&bytes)), bytes, "{} bytes", bytes.len());
        }
        // Flat data comes out much smaller than it went in.
        assert!(deflate(&[0; 100_000]).len() < 1000);
    }

    #[test]
    fn a_picture_comes_back_pixel_for_pixel() {
        // A gradient, a flat area and a transparent area.
        let (width, height) = (37u32, 21u32);
        let mut rgba = Vec::new();
        for y in 0..height {
            for x in 0..width {
                if x < 12 {
                    rgba.extend_from_slice(&[x as u8 * 20, y as u8 * 12, 200, 255]);
                } else if y < 10 {
                    rgba.extend_from_slice(&[30, 144, 255, 255]);
                } else {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        let png = encode(&rgba, width, height);
        assert_eq!(decode(&png), (rgba.clone(), width, height));
        assert!(png.len() < rgba.len(), "{} of {}", png.len(), rgba.len());
        // One pixel.
        let one = encode(&[1, 2, 3, 4], 1, 1);
        assert_eq!(decode(&one), (vec![1, 2, 3, 4], 1, 1));
    }
}
