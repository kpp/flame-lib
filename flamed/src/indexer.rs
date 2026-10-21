//! The core chain, as this node sees it: through an indexer.
//!
//! Every Flame header commits a `core_block_hash`, and the Bitcoin block it
//! names carries the time a Flame block is shown with. A Flame header has
//! no time of its own, so the node asks an indexer for both: which core
//! block to anchor the next block to, and later, what time a core block it
//! anchored to had.
//!
//! The node keeps those times only in memory, in its block index. The
//! indexer is what remembers them, so a node that restarts and replays its
//! archive asks the indexer again for every block and ends up with the same
//! times it had.
//!
//! `MockIndexer` is the only implementation. There is no Bitcoin behind it:
//! the hash it anchors to is `core_block_hash(height)`, as before, and the
//! time is this machine's clock when it first saw that height. It keeps its
//! own table in its own directory, apart from `blocks.bin`, so an
//! implementation backed by a real Bitcoin indexer can replace it without
//! the node changing.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// One core block, as an indexer reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreBlock {
    /// The hash a Flame header commits as `core_block_hash`.
    pub hash: [u8; 32],
    /// Its time, in Unix seconds.
    pub time: u64,
}

/// What the node needs from a core-chain indexer.
pub trait CoreIndexer: Send {
    /// The core block to anchor the Flame block at `flame_height` to.
    fn anchor(&mut self, flame_height: u64) -> Result<CoreBlock, CoreError>;

    /// The time of a core block, or `None` if this indexer does not know
    /// it.
    fn time_of(&self, hash: &[u8; 32]) -> Result<Option<u64>, CoreError>;
}

/// This devnet's stand-in for a core-chain identity.
///
/// `check_header` does not constrain `core_block_hash`, so a devnet needs
/// some rule; height is the only thing a second node could independently
/// agree on.
pub fn core_block_hash(height: u64) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"flame.devnet.core");
    digest.update(height.to_le_bytes());
    digest.finalize().into()
}

/// This machine's clock, in Unix seconds.
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// An indexer with no Bitcoin behind it.
///
/// Its table lives in `core-times.txt`, one line per core block: the hash
/// in hex, a space, the Unix time, a newline. Each line is written and
/// synced before `anchor` returns it, so a time the node has used is one
/// this indexer can give back after a restart.
#[derive(Debug)]
pub struct MockIndexer {
    file: File,
    times: BTreeMap<[u8; 32], u64>,
}

impl MockIndexer {
    /// The table's file name inside the indexer's directory.
    pub const FILE: &'static str = "core-times.txt";

    /// Opens the table in `dir`, creating both if this is a fresh start.
    ///
    /// A trailing line with no newline means the process died mid-append.
    /// The block it was for was never archived, because `anchor` had not
    /// returned yet, so the line is dropped.
    pub fn open(dir: &Path) -> Result<Self, CoreError> {
        fs::create_dir_all(dir)?;
        let path = dir.join(Self::FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let complete = text.rfind('\n').map_or(0, |end| end + 1);

        let mut times = BTreeMap::new();
        for (index, line) in text[..complete].lines().enumerate() {
            let (hash, time) = parse_line(line).ok_or_else(|| CoreError::Corrupt {
                path: path.clone(),
                line: index + 1,
            })?;
            times.insert(hash, time);
        }

        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        if complete < text.len() {
            file.set_len(complete as u64)?;
        }
        Ok(Self { file, times })
    }

    /// How many core blocks this indexer knows.
    pub fn len(&self) -> usize {
        self.times.len()
    }

    /// Whether it knows any.
    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }
}

impl CoreIndexer for MockIndexer {
    /// Anchors to `core_block_hash(flame_height)`. A height asked for twice
    /// — a mint that failed and was retried — gets the time it got the
    /// first time.
    fn anchor(&mut self, flame_height: u64) -> Result<CoreBlock, CoreError> {
        let hash = core_block_hash(flame_height);
        if let Some(&time) = self.times.get(&hash) {
            return Ok(CoreBlock { hash, time });
        }
        let time = unix_now();
        writeln!(self.file, "{} {time}", hex::encode(hash))?;
        self.file.sync_data()?;
        self.times.insert(hash, time);
        Ok(CoreBlock { hash, time })
    }

    fn time_of(&self, hash: &[u8; 32]) -> Result<Option<u64>, CoreError> {
        Ok(self.times.get(hash).copied())
    }
}

fn parse_line(line: &str) -> Option<([u8; 32], u64)> {
    let (hash, time) = line.split_once(' ')?;
    let hash = hex::decode(hash).ok()?.try_into().ok()?;
    Some((hash, time.parse().ok()?))
}

/// An indexer could not answer.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A line of the mock's table that does not parse.
    #[error("{path} line {line} is not `<hex hash> <unix time>`")]
    Corrupt {
        /// The table.
        path: PathBuf,
        /// Which line, from 1.
        line: usize,
    },
    /// The filesystem.
    #[error(transparent)]
    Io(#[from] io::Error),
}
