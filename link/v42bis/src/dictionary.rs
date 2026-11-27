// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.

//! The dictionary (V.42bis clause 6): the strings both ends know, as a
//! set of trees in arrays of a fixed size, and the procedure that walks
//! them as the characters of the data come.
//!
//! Each entry is a node of a tree: a character, the entry of the string
//! it lengthens, and the entries that lengthen it in their turn, as a
//! list. The entry's place in the arrays is its codeword. The first
//! three codewords are the control codewords and have no entry; the next
//! 256 are the strings of one character, the roots, which are there
//! from the start and for good (clause 6.2).
//!
//! The encoder and the decoder each have one of these, and the two are
//! kept the same by doing the same things to them in the same order:
//! that is all the agreement there is between the two ends.

use crate::{MAX_CODEWORDS, N4, N5, N6};

/// No entry: the end of a list, or no parent.
pub(crate) const NONE: u16 = u16::MAX;

/// The dictionary of one direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dictionary {
    // N2: the number of codewords, and so of entries with the control
    // codewords counted.
    size: u16,
    // N7: the longest string.
    longest: u8,
    // For each entry: the entry of the string without its last
    // character, NONE for a root and for an entry not in use; the last
    // character; the first entry that lengthens it; the next entry that
    // lengthens the same string; and the string's length.
    parent: [u16; MAX_CODEWORDS],
    character: [u8; MAX_CODEWORDS],
    child: [u16; MAX_CODEWORDS],
    sibling: [u16; MAX_CODEWORDS],
    length: [u8; MAX_CODEWORDS],
    // C1: the codeword of the next empty entry (clause 6.5).
    next_empty: u16,
}

impl Dictionary {
    /// A dictionary in its initial condition (clause 6.2), for `size`
    /// codewords and strings of up to `longest` characters. The two are
    /// held to what this modem can do.
    pub fn new(size: u16, longest: u8) -> Dictionary {
        let mut dictionary = Dictionary {
            size: size.clamp(crate::MIN_CODEWORDS, MAX_CODEWORDS as u16),
            longest: longest.clamp(crate::MIN_STRING, crate::MAX_STRING),
            parent: [NONE; MAX_CODEWORDS],
            character: [0; MAX_CODEWORDS],
            child: [NONE; MAX_CODEWORDS],
            sibling: [NONE; MAX_CODEWORDS],
            length: [0; MAX_CODEWORDS],
            next_empty: N5,
        };
        dictionary.reset();
        dictionary
    }

    /// Makes the dictionary one of `size` codewords and strings of up to
    /// `longest` characters, in its initial condition: what was agreed
    /// when the connection was made (clause 5.2), set where the
    /// dictionary is, so that nothing of its size is made or moved.
    pub fn configure(&mut self, size: u16, longest: u8) {
        self.size = size.clamp(crate::MIN_CODEWORDS, MAX_CODEWORDS as u16);
        self.longest = longest.clamp(crate::MIN_STRING, crate::MAX_STRING);
        self.reset();
    }

    /// The initial condition again (clause 6.2): each tree its root and
    /// nothing else, and C1 at N5.
    pub fn reset(&mut self) {
        self.parent = [NONE; MAX_CODEWORDS];
        self.character = [0; MAX_CODEWORDS];
        self.child = [NONE; MAX_CODEWORDS];
        self.sibling = [NONE; MAX_CODEWORDS];
        self.length = [0; MAX_CODEWORDS];
        for character in 0..N4 {
            let entry = usize::from(N6 + character);
            self.character[entry] = character as u8;
            self.length[entry] = 1;
        }
        self.next_empty = N5;
    }

    /// N2: the number of codewords.
    pub fn size(&self) -> u16 {
        self.size
    }

    /// C1: the codeword of the next empty entry.
    pub fn next_empty(&self) -> u16 {
        self.next_empty
    }

    /// The codeword of the string of one character (clause 6.2).
    #[inline]
    pub(crate) const fn root(character: u8) -> u16 {
        N6 + character as u16
    }

    /// Whether a codeword is of an entry that holds a string.
    #[inline]
    pub(crate) fn in_use(&self, codeword: u16) -> bool {
        codeword >= N6
            && codeword < self.size
            && (codeword < N5 || self.parent[usize::from(codeword)] != NONE)
    }

    /// The length of an entry's string.
    #[inline]
    pub(crate) fn length(&self, codeword: u16) -> u8 {
        self.length[usize::from(codeword) % MAX_CODEWORDS]
    }

    /// The entry of a string lengthened by a character, if there is one
    /// (clause 6.3). It looks at each of the entries that lengthen the
    /// string, which are 256 at the most.
    #[inline]
    pub(crate) fn find(&self, string: u16, character: u8) -> Option<u16> {
        let mut entry = self.child[usize::from(string) % MAX_CODEWORDS];
        // The list cannot be longer than the alphabet; the count is so
        // that nothing received can make this go on.
        for _ in 0..N4 {
            if entry == NONE {
                return None;
            }
            if self.character[usize::from(entry)] == character {
                return Some(entry);
            }
            entry = self.sibling[usize::from(entry)];
        }
        None
    }

    /// Adds a string: an existing one lengthened by a character (clause
    /// 6.4), in the empty entry C1, and then finds the next empty entry
    /// (clause 6.5). Gives the codeword of the new entry, or nothing if
    /// the string would be too long or is there already, in which case
    /// nothing is changed.
    pub(crate) fn add(&mut self, string: u16, character: u8) -> Option<u16> {
        if !self.in_use(string)
            || self.length(string) >= self.longest
            || self.find(string, character).is_some()
        {
            return None;
        }
        let entry = self.next_empty;
        let at = usize::from(entry);
        self.parent[at] = string;
        self.character[at] = character;
        self.child[at] = NONE;
        self.length[at] = self.length(string) + 1;
        // At the head of the list of what lengthens the string.
        self.sibling[at] = self.child[usize::from(string)];
        self.child[usize::from(string)] = entry;
        self.recover();
        Some(entry)
    }

    /// The procedure for recovering a dictionary entry (clause 6.5),
    /// applied after an entry has been made: C1 moves on to the next
    /// entry that is empty, or that is a leaf, which is then taken from
    /// its tree and is empty.
    fn recover(&mut self) {
        // No more steps than there are entries: there is always a leaf
        // to find, the one just made if no other.
        for _ in 0..self.size {
            // a) and b).
            self.next_empty += 1;
            if self.next_empty >= self.size {
                self.next_empty = N5;
            }
            let at = usize::from(self.next_empty);
            let parent = self.parent[at];
            if parent == NONE {
                // Not in use: empty already.
                return;
            }
            if self.child[at] != NONE {
                // c) in use and not a leaf.
                continue;
            }
            // d) a leaf: detached from its parent.
            let mut link = self.child[usize::from(parent)];
            if link == self.next_empty {
                self.child[usize::from(parent)] = self.sibling[at];
            } else {
                for _ in 0..N4 {
                    let after = self.sibling[usize::from(link) % MAX_CODEWORDS];
                    if after == self.next_empty {
                        self.sibling[usize::from(link)] = self.sibling[at];
                        break;
                    }
                    if after == NONE {
                        break;
                    }
                    link = after;
                }
            }
            self.parent[at] = NONE;
            self.sibling[at] = NONE;
            self.length[at] = 0;
            return;
        }
    }

    /// Writes an entry's string into `out`, and gives its length: none
    /// if the entry holds no string. A string is 250 characters at the
    /// most.
    pub(crate) fn string(&self, codeword: u16, out: &mut [u8; 256]) -> usize {
        if !self.in_use(codeword) {
            return 0;
        }
        let len = usize::from(self.length(codeword));
        let mut entry = codeword;
        // From the last character back to the first.
        for at in (0..len).rev() {
            if entry == NONE {
                return 0;
            }
            out[at] = self.character[usize::from(entry) % MAX_CODEWORDS];
            entry = self.parent[usize::from(entry) % MAX_CODEWORDS];
        }
        len
    }
}

/// The string matching procedure (clause 6.3) with the dictionary update
/// that follows each match (clause 6.4), as both the encoder and, in
/// transparent mode, the decoder apply it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Matcher {
    // The entry of the string matched so far; NONE before the first
    // character.
    pub(crate) string: u16,
    // The entry made by the last invocation of the procedure, which this
    // one may not match (clause 6.3 b); NONE if it made none.
    pub(crate) last_made: u16,
    // Whether the procedure was ended before its longest match, by a
    // flush or a change of mode: the next character is then the
    // unmatched one, whatever the dictionary holds (clause 6.3).
    pub(crate) ended: bool,
}

/// What a character did to the match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Matched {
    /// The string was lengthened by it, or begun with it.
    Longer,
    /// It did not match: the string before it is complete, and this is
    /// its entry. The character begins the next string.
    Complete(u16),
    /// The match had been ended already, and the character begins the
    /// next string: nothing is complete that was not already.
    Restarted,
}

impl Matcher {
    pub(crate) const fn new() -> Matcher {
        Matcher {
            string: NONE,
            last_made: NONE,
            ended: false,
        }
    }

    /// Takes the next character of the data.
    #[inline]
    pub(crate) fn step(&mut self, dictionary: &mut Dictionary, character: u8) -> Matched {
        if self.string == NONE {
            self.string = Dictionary::root(character);
            return Matched::Longer;
        }
        if !self.ended
            && let Some(entry) = dictionary.find(self.string, character)
            && entry != self.last_made
        {
            // Clause 6.3 b).
            self.string = entry;
            return Matched::Longer;
        }
        // Clause 6.3 c): the string is the longest match and the
        // character is unmatched. Clause 6.4: the two make a new entry.
        let complete = self.string;
        self.last_made = dictionary.add(complete, character).unwrap_or(NONE);
        self.string = Dictionary::root(character);
        if std::mem::take(&mut self.ended) {
            Matched::Restarted
        } else {
            Matched::Complete(complete)
        }
    }
}
