pub mod bitcoin;
pub mod consensus_journal;
pub mod flame;
pub mod in_memory;
pub mod operation;

pub use crate::block_processing::canonical_chain::FlameTransition;
pub use crate::rewards::application::rewards_changes::RewardsChanges;
pub use bitcoin::{
    ApplyStage, BitcoinBlockJournal, BitcoinBlockJournalEntry, BitcoinJournal, BitcoinJournalEntry,
    BitcoinJournalOperation, BitcoinOperationKind, JournalStatus, JournalTransitionError,
    RollbackStage,
};
pub use consensus_journal::ConsensusJournal;
pub use flame::{
    FlameApplyStage, FlameJournal, FlameJournalEntry, FlameJournalOperation, FlameJournalStatus,
};
pub use in_memory::{
    InMemoryBitcoinBlockJournal, InMemoryBitcoinBlockJournalError, InMemoryBitcoinJournal,
    InMemoryConsensusJournal, InMemoryConsensusJournalError, InMemoryFlameJournal,
};
pub use operation::{ConsensusJournalEntry, ConsensusOperation, JournalEntry, OperationId};

pub mod consensus {
    pub use super::{
        BitcoinJournal, BitcoinJournalEntry, BitcoinJournalOperation, BitcoinOperationKind,
        ConsensusJournal, ConsensusJournalEntry, ConsensusOperation, FlameJournal,
        FlameJournalEntry, FlameJournalOperation, InMemoryBitcoinJournal, InMemoryConsensusJournal,
        InMemoryConsensusJournalError, InMemoryFlameJournal, JournalEntry, OperationId,
    };
}
