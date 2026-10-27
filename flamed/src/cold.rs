//! `cold.bin`: the bytes the indexes keep and seldom read.
//!
//! Effect logs, contracts and notes are most of what an index record
//! weighs, and the node reads one only when a wallet asks about it. They are
//! appended here, and the record in memory keeps a 16-byte [`Cold`] handle
//! instead. What has to stay in memory — every live Utreexo proof, which
//! each block rewrites — stays there.
//!
//! The file is a cache, not state. Every start replaces it with an empty
//! one, and replaying `blocks.bin` writes it again, so a lost or corrupt
//! `cold.bin` costs a restart and nothing else. Nothing in it is synced. Its
//! pages live in the kernel's page cache, which, unlike the heap, the kernel
//! can drop under memory pressure and read back when asked.
//!
//! A read is checked against a hash its handle carries, so a corrupt file is
//! an error that says to restart, never wrong bytes served as a contract.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Where one blob is in `cold.bin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cold {
    offset: u64,
    len: u32,
    check: u32,
}

/// The append-only blob file.
#[derive(Debug)]
pub struct ColdStore {
    file: File,
    end: u64,
}

impl ColdStore {
    /// Starts an empty file at `path`, replacing whatever was there.
    ///
    /// By unlinking first, not by truncating: a process that still has the
    /// old file open, such as a node being replaced by this one, keeps
    /// reading its own.
    pub fn create(path: &Path) -> io::Result<ColdStore> {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        Ok(ColdStore { file, end: 0 })
    }

    /// Appends one blob.
    pub fn put(&mut self, bytes: &[u8]) -> Result<Cold, ColdError> {
        let len = u32::try_from(bytes.len()).map_err(|_| ColdError::TooLarge(bytes.len()))?;
        self.file.write_all_at(bytes, self.end)?;
        let cold = Cold {
            offset: self.end,
            len,
            check: check(bytes),
        };
        self.end += u64::from(len);
        Ok(cold)
    }

    /// Reads one blob back.
    pub fn get(&self, cold: Cold) -> Result<Vec<u8>, ColdError> {
        let mut bytes = vec![0; cold.len as usize];
        self.file
            .read_exact_at(&mut bytes, cold.offset)
            .map_err(|error| match error.kind() {
                io::ErrorKind::UnexpectedEof => ColdError::Corrupt {
                    offset: cold.offset,
                },
                _ => ColdError::Io(error),
            })?;
        if check(&bytes) != cold.check {
            return Err(ColdError::Corrupt {
                offset: cold.offset,
            });
        }
        Ok(bytes)
    }

    /// How many bytes the file holds.
    pub fn len(&self) -> u64 {
        self.end
    }

    /// Whether it holds any.
    pub fn is_empty(&self) -> bool {
        self.end == 0
    }
}

fn check(bytes: &[u8]) -> u32 {
    let digest = Sha256::digest(bytes);
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

/// `cold.bin` could not be written, or did not give back what was written.
#[derive(Debug, thiserror::Error)]
pub enum ColdError {
    /// A blob too large for a handle.
    #[error("a {0}-byte blob is too large for cold.bin")]
    TooLarge(usize),
    /// The file no longer holds what the handle says.
    #[error("cold.bin is corrupt at byte {offset}; restart the node to rebuild it")]
    Corrupt {
        /// Where the blob was written.
        offset: u64,
    },
    /// The file itself.
    #[error("cold.bin: {0}")]
    Io(#[from] io::Error),
}
