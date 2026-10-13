use btc_integration::BtcBlockTip;

use super::JournalStatus;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum InMemoryBitcoinBlockJournalError {
    #[error("Bitcoin block journal lock poisoned")]
    LockPoisoned,
    #[error("a journal operation is already in progress for {0:?}")]
    OperationInProgress(BtcBlockTip),
    #[error("no journal operation is in progress")]
    NoPendingOperation,
    #[error("journal entry not found for {0:?}")]
    EntryNotFound(BtcBlockTip),
    #[error("journal entry already exists for {0:?}")]
    EntryAlreadyExists(BtcBlockTip),
    #[error("Bitcoin block tip mismatch: expected {expected:?}, got {actual:?}")]
    BlockTipMismatch {
        expected: BtcBlockTip,
        actual: BtcBlockTip,
    },
    #[error("{next:?} is not a successor of {previous:?}")]
    InvalidSuccessor {
        previous: BtcBlockTip,
        next: BtcBlockTip,
    },
    #[error("journal status mismatch: expected {expected:?}, got {actual:?}")]
    UnexpectedStatus {
        expected: JournalStatus,
        actual: JournalStatus,
    },
    #[error("Flame transition has already been prepared")]
    FlameTransitionAlreadyPrepared,
    #[error("reward changes have already been prepared")]
    RewardsChangesAlreadyPrepared,
    #[error("Flame transition has not been prepared")]
    MissingFlameTransition,
    #[error("reward changes have not been prepared")]
    MissingRewardsChanges,
}
