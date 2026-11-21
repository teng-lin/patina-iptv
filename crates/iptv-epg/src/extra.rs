//! The fields most guide cells do not need, packed per program into one
//! shared `Vec<u32>`: a presence mask followed only by the fields that are
//! set. A program with a description and an episode number costs three
//! words (12 bytes) instead of a fixed 28-byte record; one with nothing
//! extra costs nothing (its slot says [`NO_EXTRA`]).

use crate::intern::{NONE, Sym};

pub(crate) const NO_EXTRA: u32 = u32::MAX;

/// Text fields, in mask bit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Field {
    SubTitle = 0,
    Desc = 1,
    Category = 2,
    Icon = 3,
    Rating = 4,
    CatchupId = 5,
}

const TEXT_FIELDS: u32 = 6;
/// Mask bit of the packed `season << 16 | episode` word (after the texts).
const EPISODE_BIT: u32 = 1 << TEXT_FIELDS;

/// One program's extras while building.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Extra {
    pub text: [Sym; TEXT_FIELDS as usize],
    /// One-based; 0 = unknown.
    pub season: u16,
    pub episode: u16,
}

impl Extra {
    pub fn set(&mut self, field: Field, sym: Sym) {
        self.text[field as usize] = sym;
    }
}

/// Appends `e` to `words`; returns its offset, or [`NO_EXTRA`] when empty.
pub(crate) fn push(words: &mut Vec<u32>, e: &Extra) -> u32 {
    let mut mask = 0;
    for (bit, &sym) in e.text.iter().enumerate() {
        if sym != NONE {
            mask |= 1 << bit;
        }
    }
    if e.season != 0 || e.episode != 0 {
        mask |= EPISODE_BIT;
    }
    if mask == 0 {
        return NO_EXTRA;
    }
    let at = words.len();
    // A guide would need 4G words of extras to reach this.
    assert!(at < NO_EXTRA as usize, "extras overflow");
    words.push(mask);
    words.extend(e.text.iter().copied().filter(|&s| s != NONE));
    if mask & EPISODE_BIT != 0 {
        words.push(u32::from(e.season) << 16 | u32::from(e.episode));
    }
    at as u32
}

fn word(words: &[u32], at: u32, bit: u32) -> Option<u32> {
    let mask = *words.get(at as usize)?;
    if mask & bit == 0 {
        return None;
    }
    let before = (mask & (bit - 1)).count_ones() as usize;
    words.get(at as usize + 1 + before).copied()
}

pub(crate) fn text(words: &[u32], at: u32, field: Field) -> Sym {
    word(words, at, 1 << field as u32).unwrap_or(NONE)
}

/// One-based season and episode (0 = unknown).
pub(crate) fn episode(words: &[u32], at: u32) -> (u16, u16) {
    word(words, at, EPISODE_BIT).map_or((0, 0), |w| ((w >> 16) as u16, w as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_only_what_is_set() {
        let mut words = Vec::new();
        assert_eq!(push(&mut words, &Extra::default()), NO_EXTRA);
        assert!(words.is_empty());

        let mut a = Extra::default();
        a.set(Field::Desc, 7);
        a.episode = 3;
        let mut b = Extra::default();
        b.set(Field::SubTitle, 1);
        b.set(Field::CatchupId, 9);
        b.season = 2;
        let (pa, pb) = (push(&mut words, &a), push(&mut words, &b));
        assert_eq!(words.len(), 3 + 4);
        assert_eq!(text(&words, pa, Field::Desc), 7);
        assert_eq!(text(&words, pa, Field::SubTitle), NONE);
        assert_eq!(episode(&words, pa), (0, 3));
        assert_eq!(text(&words, pb, Field::SubTitle), 1);
        assert_eq!(text(&words, pb, Field::CatchupId), 9);
        assert_eq!(text(&words, pb, Field::Desc), NONE);
        assert_eq!(episode(&words, pb), (2, 0));
    }
}
