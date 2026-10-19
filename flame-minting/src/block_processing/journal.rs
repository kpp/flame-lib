use btc_integration::BtcBlockTip;
use flame_chain_service::ChainPath;
use flamechain::{BlockHeader, BlockTip};

use crate::rewards::{RewardsOutcome, RewardsUndo};

use crate::consensus::{ConsensusUndo, MintingOutcome};

pub mod consensus;
pub mod flame;
pub mod in_memory;
mod status;
mod storage;

pub use in_memory::{InMemoryBitcoinBlockJournal, InMemoryBitcoinBlockJournalError};
pub use status::{ApplyStage, JournalStatus, JournalTransitionError, RollbackStage};
pub use storage::BitcoinBlockJournal;

#[derive(Clone, Debug)]
pub struct BitcoinBlockJournalEntry {
    pub btc_tip: BtcBlockTip,
    pub previous_btc_tip: BtcBlockTip,
    pub minting_outcome: MintingOutcome,
    pub consensus_undo: ConsensusUndo,
    pub flame_transition: Option<FlameTransition>,
    pub rewards_changes: Option<RewardsChanges>,
    pub status: JournalStatus,
}

impl BitcoinBlockJournalEntry {
    pub fn new(
        previous_btc_tip: BtcBlockTip,
        minting_outcome: MintingOutcome,
        consensus_undo: ConsensusUndo,
    ) -> Self {
        Self {
            btc_tip: minting_outcome.next_btc_cursor,
            previous_btc_tip,
            minting_outcome,
            consensus_undo,
            flame_transition: None,
            rewards_changes: None,
            status: JournalStatus::Applying(ApplyStage::RemoveEndorsements),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlameTransition {
    pub previous_tip: BlockTip,
    pub next_tip: BlockTip,
    pub path: ChainPath,
    pub attached_core_headers: Vec<BlockHeader>,
}

#[derive(Clone, Debug)]
pub struct RewardsChanges {
    pub outcomes: Vec<RewardsOutcome>,
    pub undo: RewardsUndo,
}
