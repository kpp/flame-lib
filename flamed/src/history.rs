//! Paging a set of addresses' histories.
//!
//! Each predicate's history is a list in chain order (`OutputIndex::history`),
//! so a page is a binary search into each list and a merge of what follows.
//! A cursor names a position, never an offset: the chain only grows, so the
//! entries before a position are the same however many blocks come after.
//! It also names the block at its height, so a page asked after a reorg is
//! refused rather than skipping or repeating entries.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use sha2::{Digest, Sha256};

use crate::index::{Position, Touch};

/// Which way a page reads from its cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Before the cursor, newest first.
    Older,
    /// After the cursor, oldest first.
    Newer,
}

/// The place `Cursor::position` gives for "after everything at this
/// height": a newer cursor at the tip, before the next block exists.
pub const END_OF_BLOCK: u32 = u32::MAX;

/// A place in one set of addresses' history.
///
/// Written `older:12:3:<block>:<addresses>`, readable to debug with and
/// promised to no one: only this node reads it back, and its format may
/// change with any release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    /// Which way the next page reads.
    pub direction: Direction,
    /// The last position already read, excluded from the next page.
    pub position: Position,
    /// The id of the block at `position`'s height when the cursor was made.
    pub block: [u8; 32],
    /// Which addresses it was made for; see [`address_set`].
    pub set: [u8; 8],
}

impl Cursor {
    /// Reads a cursor this node wrote, or `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(':');
        let direction = match parts.next()? {
            "older" => Direction::Older,
            "newer" => Direction::Newer,
            _ => return None,
        };
        let height = parts.next()?.parse().ok()?;
        let index = match parts.next()? {
            "end" => END_OF_BLOCK,
            index => index.parse().ok()?,
        };
        let block = hex::decode(parts.next()?).ok()?.try_into().ok()?;
        let set = hex::decode(parts.next()?).ok()?.try_into().ok()?;
        parts.next().is_none().then_some(Cursor {
            direction,
            position: (height, index),
            block,
            set,
        })
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let direction = match self.direction {
            Direction::Older => "older",
            Direction::Newer => "newer",
        };
        let (height, index) = self.position;
        write!(f, "{direction}:{height}:")?;
        if index == END_OF_BLOCK {
            f.write_str("end")?;
        } else {
            write!(f, "{index}")?;
        }
        write!(f, ":{}:{}", hex::encode(self.block), hex::encode(self.set))
    }
}

/// A short fingerprint of a set of predicates, so a cursor asked with
/// other addresses than it was made for is refused instead of answered
/// with a page of some other history. Order and repeats do not matter.
pub fn address_set(predicates: &BTreeSet<[u8; 32]>) -> [u8; 8] {
    let mut digest = Sha256::new();
    digest.update(b"flamed.history.addresses");
    for predicate in predicates {
        digest.update(predicate);
    }
    digest.finalize()[..8].try_into().expect("8 of 32 bytes")
}

/// Up to `limit` transactions from `histories` strictly past `after` in
/// `direction`, in that direction's order, and whether more follow.
///
/// A transaction in several histories is one entry, with what it did to
/// each. Taking `limit + 1` from each list is enough: an entry among the
/// first `limit` overall is among the first `limit` of every list it is
/// in, so it comes back whole, and one more says whether there is more.
pub fn page<'a>(
    histories: impl IntoIterator<Item = &'a [Touch]>,
    direction: Direction,
    after: Option<Position>,
    limit: usize,
) -> (Vec<Touch>, bool) {
    let mut merged: BTreeMap<Position, Touch> = BTreeMap::new();
    for history in histories {
        let past: Box<dyn Iterator<Item = &Touch>> = match direction {
            Direction::Older => {
                let end = after.map_or(history.len(), |at| {
                    history.partition_point(|touch| touch.position < at)
                });
                Box::new(history[..end].iter().rev())
            }
            Direction::Newer => {
                let start = after.map_or(0, |at| {
                    history.partition_point(|touch| touch.position <= at)
                });
                Box::new(history[start..].iter())
            }
        };
        for touch in past.take(limit + 1) {
            merged
                .entry(touch.position)
                .and_modify(|entry| {
                    entry.created.extend(&touch.created);
                    entry.spent.extend(&touch.spent);
                })
                .or_insert_with(|| touch.clone());
        }
    }

    let mut touches: Vec<Touch> = match direction {
        Direction::Older => merged.into_values().rev().collect(),
        Direction::Newer => merged.into_values().collect(),
    };
    let more = touches.len() > limit;
    touches.truncate(limit);
    (touches, more)
}

#[cfg(test)]
mod tests {
    use flamevm::{ContractID, TxID};

    use super::*;

    fn id(n: u8) -> ContractID {
        [n; 32]
    }

    fn touch(height: u64, index: u32, created: &[u8], spent: &[u8]) -> Touch {
        Touch {
            position: (height, index),
            txid: TxID([height as u8 * 16 + index as u8; 32]),
            created: created.iter().copied().map(id).collect(),
            spent: spent.iter().copied().map(id).collect(),
        }
    }

    fn positions(touches: &[Touch]) -> Vec<Position> {
        touches.iter().map(|touch| touch.position).collect()
    }

    #[test]
    fn a_cursor_reads_back_what_it_wrote() {
        for (direction, index) in [(Direction::Older, 3), (Direction::Newer, END_OF_BLOCK)] {
            let cursor = Cursor {
                direction,
                position: (12, index),
                block: [0xab; 32],
                set: [0x01; 8],
            };
            let text = cursor.to_string();
            assert_eq!(Cursor::parse(&text), Some(cursor), "{text}");
        }
        assert!(Cursor {
            direction: Direction::Newer,
            position: (7, END_OF_BLOCK),
            block: [0; 32],
            set: [0; 8],
        }
        .to_string()
        .starts_with("newer:7:end:"));
    }

    #[test]
    fn a_cursor_that_is_not_one_is_refused() {
        let good = Cursor {
            direction: Direction::Older,
            position: (1, 0),
            block: [0xab; 32],
            set: [0x01; 8],
        }
        .to_string();
        let mut extra = good.clone();
        extra.push_str(":0");
        for text in [
            "",
            "older",
            "sideways:1:0:ab:01",
            &good[..good.len() - 2],
            &extra,
            &good.replace("older:1:0", "older:x:0"),
        ] {
            assert_eq!(Cursor::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_address_set_ignores_order_and_repeats() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let ab: BTreeSet<_> = [a, b].into();
        let ba: BTreeSet<_> = [b, a, b].into();
        assert_eq!(address_set(&ab), address_set(&ba));
        assert_ne!(address_set(&ab), address_set(&[a].into()));
    }

    #[test]
    fn pages_walk_both_ways_and_merge_a_shared_transaction() {
        // A pays from `receiving` at (2, 0) and takes change to `change`
        // in the same transaction.
        let receiving = vec![touch(0, 0, &[1], &[]), touch(2, 0, &[], &[1])];
        let change = vec![
            touch(1, 0, &[2], &[]),
            touch(2, 0, &[3], &[]),
            touch(3, 1, &[], &[3]),
        ];
        let both = || [receiving.as_slice(), change.as_slice()];

        let (first, more) = page(both(), Direction::Older, None, 2);
        assert_eq!(positions(&first), [(3, 1), (2, 0)]);
        assert!(more);
        assert_eq!(first[1].created, [id(3)]);
        assert_eq!(first[1].spent, [id(1)]);

        let (second, more) = page(both(), Direction::Older, Some((2, 0)), 2);
        assert_eq!(positions(&second), [(1, 0), (0, 0)]);
        assert!(!more);

        let (newer, more) = page(both(), Direction::Newer, Some((1, 0)), 1);
        assert_eq!(positions(&newer), [(2, 0)]);
        assert!(more);
        assert_eq!(newer[0].spent, [id(1)]);

        let (caught_up, more) = page(both(), Direction::Newer, Some((3, END_OF_BLOCK)), 5);
        assert!(caught_up.is_empty());
        assert!(!more);
    }
}
