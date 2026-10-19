use std::future::Future;

use btc_integration::{BtcBlockTip, IndexedBlock};
use flame_chain_service::ChainAccess;
use flame_storage::{CanonicalStorage, ChainStorage};

use crate::{
    consensus::{ConsensusEngine, ConsensusStorage, ConsensusUndoManager},
    orchestrator::cursor_storage::CursorStorage,
    rewards::{RewardsEngine, RewardsStorage},
};

use super::{
    consensus::ConsensusOperations,
    flame::{self, FlameOperations},
    journal::{
        ApplyStage, BitcoinBlockJournalEntry, JournalStatus, RollbackStage,
        consensus::{BitcoinJournal, BitcoinJournalEntry, BitcoinJournalOperation, OperationId},
    },
    rewards,
};

mod rollback;

pub trait BlockProcessor: Send + Sync + 'static {
    type Error: Send + 'static;

    fn apply(
        &self,
        block: IndexedBlock,
    ) -> impl Future<Output = Result<BtcBlockTip, Self::Error>> + Send;
    fn resume(&self) -> impl Future<Output = Result<BtcBlockTip, Self::Error>> + Send;
    fn rollback(
        &self,
        tip: BtcBlockTip,
    ) -> impl Future<Output = Result<BtcBlockTip, Self::Error>> + Send;
}

pub struct BitcoinBlockProcessor<E, C: ConsensusStorage, H, J, S, R, K, T: ChainStorage> {
    pub(crate) engine: E,
    consensus_storage: C,
    chain: H,
    journal: J,
    canonical_storage: S,
    rewards_storage: R,
    pub(crate) cursor_storage: K,
    rewards_engine: RewardsEngine<T, C>,
}

type ProcessingResult<P, V> = Result<V, <P as BlockProcessor>::Error>;

impl<E, C, H, J, S, R, K, T> BitcoinBlockProcessor<E, C, H, J, S, R, K, T>
where
    E: ConsensusEngine,
    C: ConsensusStorage + Send + Sync + 'static,
    C::Error: Send + 'static,
    H: ChainAccess + Send + Sync + 'static,
    H::Error: Send + 'static,
    J: BitcoinJournal,
    S: CanonicalStorage + Send + Sync + 'static,
    S::Error: Send + 'static,
    R: RewardsStorage + Send + Sync + 'static,
    R::Error: Send + 'static,
    K: CursorStorage,
    T: ChainStorage + Send + Sync + 'static,
    T::Error: Send + 'static,
{
    pub fn new(
        engine: E,
        consensus_storage: C,
        chain: H,
        journal: J,
        canonical_storage: S,
        rewards_storage: R,
        cursor_storage: K,
        rewards_engine: RewardsEngine<T, C>,
    ) -> Self {
        Self {
            engine,
            consensus_storage,
            chain,
            journal,
            canonical_storage,
            rewards_storage,
            cursor_storage,
            rewards_engine,
        }
    }

    pub async fn apply(&self, block: IndexedBlock) -> ProcessingResult<Self, BtcBlockTip> {
        let previous = self.drive().await?;
        let requested = block.btc_block_tip;
        if previous == requested {
            return Ok(previous);
        }
        if previous.height.checked_add(1) != Some(requested.height)
            || previous.hash == requested.hash
        {
            return Err(BlockProcessingError::InvalidSuccessor {
                previous,
                requested,
            });
        }
        let outcome = self
            .engine
            .get_minting_outcome(block.clone())
            .await
            .map_err(BlockProcessingError::Engine)?;
        if outcome.next_btc_cursor != requested {
            return Err(BlockProcessingError::OutcomeBlockMismatch {
                requested,
                actual: outcome.next_btc_cursor,
            });
        }
        let mut undo = ConsensusUndoManager {
            consensus_storage: &self.consensus_storage,
        }
        .prepare(&outcome.consensus)
        .await
        .map_err(BlockProcessingError::Consensus)?;
        undo.included_endorsements = block
            .endorsements
            .iter()
            .cloned()
            .map(|endorsement| crate::consensus::IncludedEndorsement {
                btc_block: requested,
                endorsement,
            })
            .collect();
        self.journal
            .begin_apply(previous, &block, &outcome, &undo)
            .await
            .map_err(BlockProcessingError::Journal)?;
        self.drive().await
    }

    pub async fn resume(&self) -> ProcessingResult<Self, BtcBlockTip> {
        self.drive().await
    }

    async fn rollback_required(
        &self,
        tip: BtcBlockTip,
        current: BtcBlockTip,
    ) -> ProcessingResult<Self, bool> {
        let entry = self
            .journal
            .get_entry(tip)
            .await
            .map_err(BlockProcessingError::Journal)?
            .ok_or(BlockProcessingError::MissingEntry(tip))?
            .operation
            .entry;
        if entry.status == JournalStatus::RolledBack && current == entry.previous_btc_tip {
            return Ok(false);
        }
        if current != tip {
            return Err(BlockProcessingError::RollbackTipMismatch {
                requested: tip,
                current,
            });
        }
        if entry.status != JournalStatus::Applied {
            return Err(BlockProcessingError::UnexpectedPendingStatus(entry.status));
        }
        Ok(true)
    }

    pub async fn rollback(&self, tip: BtcBlockTip) -> ProcessingResult<Self, BtcBlockTip> {
        let current = self.drive().await?;
        if !self.rollback_required(tip, current).await? {
            return Ok(current);
        }
        let entry = self
            .journal
            .get_entry(tip)
            .await
            .map_err(BlockProcessingError::Journal)?
            .ok_or(BlockProcessingError::MissingEntry(tip))?;
        let plan = self.prepare_rollback(&entry).await?;
        self.journal
            .begin_rollback(tip, &plan)
            .await
            .map_err(BlockProcessingError::Journal)?;
        self.drive().await
    }

    async fn drive(&self) -> ProcessingResult<Self, BtcBlockTip> {
        while let Some(snapshot) = self
            .journal
            .pending_entry()
            .await
            .map_err(BlockProcessingError::Journal)?
        {
            let id = snapshot.id;
            let entry = &snapshot.operation.entry;
            match entry.status {
                JournalStatus::Applying(stage) => {
                    self.apply_stage(id, entry, stage).await?;
                    self.journal
                        .complete_apply_stage(id, stage)
                        .await
                        .map_err(BlockProcessingError::Journal)?;
                }
                JournalStatus::RollingBack(stage) => {
                    self.rollback_stage(id, &snapshot.operation, stage).await?;
                    self.journal
                        .complete_rollback_stage(id, stage)
                        .await
                        .map_err(BlockProcessingError::Journal)?;
                }
                status => return Err(BlockProcessingError::UnexpectedPendingStatus(status)),
            }
        }
        self.cursor_storage
            .get_cursor()
            .await
            .map_err(BlockProcessingError::Cursor)
    }

    async fn apply_stage(
        &self,
        id: OperationId,
        entry: &BitcoinBlockJournalEntry,
        stage: ApplyStage,
    ) -> ProcessingResult<Self, ()> {
        let outcome = &entry.minting_outcome.consensus;
        let consensus = ConsensusOperations {
            storage: &self.consensus_storage,
        };
        let flame = FlameOperations {
            consensus_storage: &self.consensus_storage,
            chain: &self.chain,
            canonical_storage: &self.canonical_storage,
        };
        match stage {
            ApplyStage::RemoveEndorsements => consensus
                .remove_endorsements(&outcome.removed_endorsements)
                .await
                .map_err(BlockProcessingError::Consensus),
            ApplyStage::StoreEndorsements => consensus
                .store_endorsements(&outcome.accepted_endorsements)
                .await
                .map_err(BlockProcessingError::Consensus),
            ApplyStage::StoreAcquisitions => consensus
                .store_acquisitions(&outcome.accepted_acquisitions)
                .await
                .map_err(BlockProcessingError::Consensus),
            ApplyStage::StoreBlockWeights => consensus
                .store_block_weights(&outcome.weighted_blocks)
                .await
                .map_err(BlockProcessingError::Consensus),
            ApplyStage::StoreDoubleSigns => consensus
                .store_double_signs(&outcome.double_signs)
                .await
                .map_err(BlockProcessingError::Consensus),
            ApplyStage::StorePendingEndorsements => {
                consensus
                    .remove_pending_endorsements(&outcome.removed_pending_endorsements)
                    .await
                    .map_err(BlockProcessingError::Consensus)?;
                consensus
                    .store_pending_endorsements(&outcome.pending_endorsements)
                    .await
                    .map_err(BlockProcessingError::Consensus)
            }
            ApplyStage::PrepareFlameTransition => {
                if entry.flame_transition.is_none() {
                    let transition = flame
                        .prepare()
                        .await
                        .map_err(BlockProcessingError::PrepareFlame)?;
                    self.journal
                        .store_flame_transition(id, &transition)
                        .await
                        .map_err(BlockProcessingError::Journal)?;
                }
                Ok(())
            }
            ApplyStage::ApplyFlameTransition => {
                let transition = entry
                    .flame_transition
                    .as_ref()
                    .ok_or(BlockProcessingError::MissingFlameTransition)?;
                flame
                    .apply(transition)
                    .await
                    .map_err(BlockProcessingError::Flame)
            }
            ApplyStage::PrepareRewards => {
                if entry.rewards_changes.is_none() {
                    let transition = entry
                        .flame_transition
                        .as_ref()
                        .ok_or(BlockProcessingError::MissingFlameTransition)?;
                    let changes = rewards::prepare(
                        &self.rewards_engine,
                        &self.rewards_storage,
                        &transition.attached_core_headers,
                        entry.btc_tip,
                    )
                    .await
                    .map_err(BlockProcessingError::PrepareRewards)?;
                    self.journal
                        .store_rewards_changes(id, &changes)
                        .await
                        .map_err(BlockProcessingError::Journal)?;
                }
                Ok(())
            }
            ApplyStage::ApplyRewards => {
                let changes = entry
                    .rewards_changes
                    .as_ref()
                    .ok_or(BlockProcessingError::MissingRewardsChanges)?;
                rewards::apply(&self.rewards_storage, changes)
                    .await
                    .map_err(BlockProcessingError::Rewards)
            }
            ApplyStage::StoreCursor => self
                .cursor_storage
                .store_cursor(entry.btc_tip)
                .await
                .map_err(BlockProcessingError::Cursor),
        }
    }
}

impl<E, C, H, J, S, R, K, T> BlockProcessor for BitcoinBlockProcessor<E, C, H, J, S, R, K, T>
where
    E: ConsensusEngine,
    C: ConsensusStorage + Send + Sync + 'static,
    C::Error: Send + 'static,
    H: ChainAccess + Send + Sync + 'static,
    H::Error: Send + 'static,
    J: BitcoinJournal,
    S: CanonicalStorage + Send + Sync + 'static,
    S::Error: Send + 'static,
    R: RewardsStorage + Send + Sync + 'static,
    R::Error: Send + 'static,
    K: CursorStorage,
    T: ChainStorage + Send + Sync + 'static,
    T::Error: Send + 'static,
{
    type Error = BlockProcessingError<
        E::Error,
        C::Error,
        H::Error,
        J::Error,
        S::Error,
        R::Error,
        K::Error,
        T::Error,
    >;
    async fn apply(&self, block: IndexedBlock) -> Result<BtcBlockTip, Self::Error> {
        self.apply(block).await
    }
    async fn resume(&self) -> Result<BtcBlockTip, Self::Error> {
        self.resume().await
    }
    async fn rollback(&self, tip: BtcBlockTip) -> Result<BtcBlockTip, Self::Error> {
        self.rollback(tip).await
    }
}

#[derive(Debug)]
pub enum BlockProcessingError<E, C, H, J, S, R, K, T> {
    Engine(E),
    Consensus(C),
    Journal(J),
    Rewards(R),
    Cursor(K),
    PrepareFlame(flame::PrepareFlameError<C, S, H>),
    Flame(flame::ChainPathApplierError<S, H>),
    PrepareRewards(rewards::PrepareRewardsError<T, C, R>),
    MissingEntry(BtcBlockTip),
    MissingRollbackPlan,
    Chain(H),
    MissingBlock(flamechain::BlockTip),
    RewardHeightOverflow,
    MissingFlameTransition,
    MissingRewardsChanges,
    UnexpectedPendingStatus(JournalStatus),
    InvalidSuccessor {
        previous: BtcBlockTip,
        requested: BtcBlockTip,
    },
    OutcomeBlockMismatch {
        requested: BtcBlockTip,
        actual: BtcBlockTip,
    },
    RollbackTipMismatch {
        requested: BtcBlockTip,
        current: BtcBlockTip,
    },
}

#[cfg(test)]
mod tests;
