mod consensus_journal_state;
pub mod in_memory_bitcoin_block_journal;
pub mod in_memory_bitcoin_journal;
pub mod in_memory_consensus_journal;
pub mod in_memory_consensus_journal_error;
pub mod in_memory_flame_journal;

pub use in_memory_bitcoin_block_journal::{
    InMemoryBitcoinBlockJournal, InMemoryBitcoinBlockJournalError,
};
pub use in_memory_bitcoin_journal::InMemoryBitcoinJournal;
pub use in_memory_consensus_journal::InMemoryConsensusJournal;
pub use in_memory_consensus_journal_error::InMemoryConsensusJournalError;
pub use in_memory_flame_journal::InMemoryFlameJournal;

#[cfg(test)]
mod tests;
