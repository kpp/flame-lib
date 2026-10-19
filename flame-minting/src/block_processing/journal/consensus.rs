use std::future::Future;

use btc_integration::{BtcBlockTip, IndexedBlock};
use flamechain::BlockTip;

use crate::consensus::{
    BitcoinRollbackPlan, ConsensusOutcome, ConsensusUndo, IncludedEndorsement, MintingOutcome,
};

use super::{
    ApplyStage, BitcoinBlockJournalEntry, FlameTransition, JournalStatus, RewardsChanges,
    RollbackStage,
    flame::{FlameApplyStage, FlameJournalStatus},
};

mod in_memory;

pub use in_memory::{
    InMemoryBitcoinJournal, InMemoryConsensusJournal, InMemoryConsensusJournalError,
    InMemoryFlameJournal,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OperationId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitcoinOperationKind {
    Apply,
    Rollback,
}

#[derive(Clone, Debug)]
pub struct BitcoinJournalOperation {
    pub kind: BitcoinOperationKind,
    pub block: IndexedBlock,
    pub entry: BitcoinBlockJournalEntry,
    pub rollback_plan: Option<BitcoinRollbackPlan>,
}

#[derive(Clone, Debug)]
pub struct FlameJournalOperation {
    pub tip: BlockTip,
    pub btc_tip: BtcBlockTip,
    pub outcome: ConsensusOutcome,
    pub undo: ConsensusUndo,
    pub transition: Option<FlameTransition>,
    pub rewards: Option<RewardsChanges>,
    pub status: FlameJournalStatus,
}

#[derive(Clone, Debug)]
pub enum ConsensusOperation {
    Bitcoin(BitcoinJournalOperation),
    Flame(FlameJournalOperation),
}

#[derive(Clone, Debug)]
pub struct JournalEntry<T> {
    pub id: OperationId,
    pub operation: T,
}

pub type ConsensusJournalEntry = JournalEntry<ConsensusOperation>;
pub type BitcoinJournalEntry = JournalEntry<BitcoinJournalOperation>;
pub type FlameJournalEntry = JournalEntry<FlameJournalOperation>;

impl ConsensusJournalEntry {
    pub fn is_complete(&self) -> bool {
        match &self.operation {
            ConsensusOperation::Bitcoin(operation) => matches!(
                operation.entry.status,
                JournalStatus::Applied | JournalStatus::RolledBack
            ),
            ConsensusOperation::Flame(entry) => matches!(entry.status, FlameJournalStatus::Applied),
        }
    }

    pub fn btc_tip(&self) -> BtcBlockTip {
        match &self.operation {
            ConsensusOperation::Bitcoin(operation) => operation.entry.btc_tip,
            ConsensusOperation::Flame(entry) => entry.btc_tip,
        }
    }
}

pub trait ConsensusJournal: Send + Sync + 'static {
    type Error: Send + 'static;
    type Bitcoin: BitcoinJournal<Error = Self::Error>;
    type Flame: FlameJournal<Error = Self::Error>;

    fn bitcoin_journal(&self) -> Self::Bitcoin;

    fn flame_journal(&self) -> Self::Flame;

    fn latest_entry(
        &self,
    ) -> impl Future<Output = Result<Option<ConsensusJournalEntry>, Self::Error>> + Send;
}

pub trait BitcoinJournal: Send + Sync + 'static {
    type Error: Send + 'static;

    fn begin_apply(
        &self,
        previous_btc_tip: BtcBlockTip,
        block: &IndexedBlock,
        outcome: &MintingOutcome,
        undo: &ConsensusUndo,
    ) -> impl Future<Output = Result<OperationId, Self::Error>> + Send;

    fn get_entry(
        &self,
        btc_tip: BtcBlockTip,
    ) -> impl Future<Output = Result<Option<BitcoinJournalEntry>, Self::Error>> + Send;

    fn pending_entry(
        &self,
    ) -> impl Future<Output = Result<Option<BitcoinJournalEntry>, Self::Error>> + Send;

    fn store_flame_transition(
        &self,
        id: OperationId,
        transition: &FlameTransition,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn store_rewards_changes(
        &self,
        id: OperationId,
        changes: &RewardsChanges,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn complete_apply_stage(
        &self,
        id: OperationId,
        stage: ApplyStage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn begin_rollback(
        &self,
        btc_tip: BtcBlockTip,
        plan: &BitcoinRollbackPlan,
    ) -> impl Future<Output = Result<OperationId, Self::Error>> + Send;

    fn get_applied_endorsements(
        &self,
    ) -> impl Future<Output = Result<Vec<IncludedEndorsement>, Self::Error>> + Send;

    fn complete_rollback_stage(
        &self,
        id: OperationId,
        stage: RollbackStage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

pub trait FlameJournal: Send + Sync + 'static {
    type Error: Send + 'static;

    fn begin_apply(
        &self,
        tip: BlockTip,
        btc_tip: BtcBlockTip,
        outcome: &ConsensusOutcome,
        undo: &ConsensusUndo,
    ) -> impl Future<Output = Result<OperationId, Self::Error>> + Send;

    fn pending_entry(
        &self,
    ) -> impl Future<Output = Result<Option<FlameJournalEntry>, Self::Error>> + Send;

    fn store_transition(
        &self,
        id: OperationId,
        transition: &FlameTransition,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn store_rewards(
        &self,
        id: OperationId,
        rewards: &RewardsChanges,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn complete_apply_stage(
        &self,
        id: OperationId,
        stage: FlameApplyStage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
