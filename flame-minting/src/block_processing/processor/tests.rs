use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use btc_integration::{Acquisition, AcquisitionData, MinterP2wsh};
use corepc_client::bitcoin::{
    Amount, BlockHash as BtcBlockHash, Transaction, TxOut, absolute, hashes::Hash, transaction,
};
use ed25519_dalek::SigningKey;
use flamechain::{
    BlockHash, BlockTip, Blockchain, ChainParams, CoreBlockHeader, CoreBlockTip, CoreFlameHeight,
};
use flamevm::Predicate;

use super::*;
use crate::block_processing::journal::{
    FlameTransition, RewardsChanges,
    consensus::{
        BitcoinJournalEntry, ConsensusJournal, InMemoryBitcoinJournal, InMemoryConsensusJournal,
    },
};
use crate::consensus::{
    AcquisitionIndex, BitcoinRollbackInput, BitcoinRollbackPlan, ConsensusUndo, DoubleSign,
    EndorsementIndex, IncludedAcquisition, IncludedEndorsement, MinterEndorsementState,
    WeightedBlockHeader, WeightedEndorsement, engine::tests::endorsement_for_tip,
    storage::InMemoryConsensusStorage,
};
use crate::consensus::{ConsensusOutcome, MintingOutcome};
use crate::orchestrator::cursor_storage::InMemoryCursorStorage;
use crate::rewards::{
    engine::tests::{TestChainStorage, protocol_params},
    storage::InMemoryRewardsStorage,
};
use flame_chain_service::ChainPath;
use flamechain::Block;

fn btc_tip(height: u64) -> BtcBlockTip {
    BtcBlockTip {
        hash: BtcBlockHash::from_byte_array([height as u8; 32]),
        height,
    }
}

fn included(minter: u8, hash: u8) -> IncludedEndorsement {
    IncludedEndorsement {
        btc_block: btc_tip(101),
        endorsement: endorsement_for_tip(
            minter,
            CoreBlockTip {
                hash: BlockHash::from([hash; 32]),
                height: 1.into(),
            },
        ),
    }
}

fn acquisition(amount: u64) -> IncludedAcquisition {
    let data = AcquisitionData::new(
        [1; 32].into(),
        Predicate::opaque(Predicate::unspendable_key()),
        SigningKey::from_bytes(&[2; 32]).verifying_key(),
    );
    let transaction = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: Vec::new(),
        output: vec![TxOut {
            value: Amount::from_sat(amount),
            script_pubkey: data.to_script(),
        }],
    };
    IncludedAcquisition {
        btc_block: btc_tip(101),
        acquisition: Acquisition::from_tx(&transaction).pop().unwrap(),
    }
}

fn tip(state: &Blockchain) -> BlockTip {
    BlockTip {
        hash: state.tip(),
        height: state.height().into(),
    }
}

struct Chain {
    blocks: BTreeMap<BlockTip, Arc<Block>>,
    transition: FlameTransition,
    plans: AtomicUsize,
    reject_plans: AtomicBool,
}

impl ChainAccess for Chain {
    type Error = &'static str;

    async fn get_block(&self, tip: BlockTip) -> Result<Option<Arc<Block>>, Self::Error> {
        Ok(self.blocks.get(&tip).cloned())
    }

    async fn get_chain_path(&self, from: BlockTip, to: BlockTip) -> Result<ChainPath, Self::Error> {
        self.plans.fetch_add(1, Ordering::SeqCst);
        if self.reject_plans.load(Ordering::SeqCst) {
            return Err("path planning rejected");
        }
        if from == to {
            return Ok(ChainPath::default());
        }
        if from == self.transition.next_tip && to == self.transition.previous_tip {
            return Ok(ChainPath {
                detach: self.transition.path.attach.iter().rev().copied().collect(),
                attach: self.transition.path.detach.iter().rev().copied().collect(),
            });
        }
        assert_eq!(from, self.transition.previous_tip);
        assert_eq!(to, self.transition.next_tip);
        Ok(self.transition.path.clone())
    }
}

struct Storage {
    state: Mutex<StoredState>,
    journal: InMemoryBitcoinJournal,
    transition: FlameTransition,
}

struct StoredState {
    chain: Blockchain,
    commits: Vec<BlockTip>,
    failure: Option<(usize, bool)>,
}

impl CanonicalStorage for Storage {
    type Error = &'static str;

    async fn get_tip(&self) -> Result<Option<BlockTip>, Self::Error> {
        Ok(Some(tip(&self.state.lock().unwrap().chain)))
    }

    async fn get_state(&self) -> Result<Option<(BlockHash, Blockchain)>, Self::Error> {
        let state = self.state.lock().unwrap();
        Ok(Some((state.chain.tip(), state.chain.clone())))
    }

    async fn commit_state(&self, chain: &Blockchain) -> Result<(), Self::Error> {
        let entry = self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .unwrap()
            .operation
            .entry;
        assert!(matches!(
            entry.status,
            JournalStatus::Applying(ApplyStage::ApplyFlameTransition)
                | JournalStatus::RollingBack(RollbackStage::ApplyFlameTransition)
        ));
        let expected = if matches!(entry.status, JournalStatus::RollingBack(_)) {
            FlameTransition {
                previous_tip: self.transition.next_tip,
                next_tip: self.transition.previous_tip,
                path: ChainPath {
                    detach: self.transition.path.attach.iter().rev().copied().collect(),
                    attach: self.transition.path.detach.iter().rev().copied().collect(),
                },
                attached_core_headers: Vec::new(),
            }
        } else {
            self.transition.clone()
        };
        assert_eq!(entry.flame_transition.as_ref(), Some(&expected));
        let mut state = self.state.lock().unwrap();
        let failure = if state
            .failure
            .is_some_and(|(step, _)| step == state.commits.len() + 1)
        {
            state.failure.take().map(|(_, after)| after)
        } else {
            None
        };
        if failure == Some(false) {
            return Err("commit interrupted");
        }
        state.chain = chain.clone();
        state.commits.push(tip(chain));
        if failure == Some(true) {
            return Err("commit interrupted");
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Consensus(Arc<ConsensusData>);
impl std::ops::Deref for Consensus {
    type Target = ConsensusData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
struct ConsensusData {
    winner: BlockTip,
    previous_winner: BlockTip,
    selections: AtomicUsize,
    reject_selection: AtomicBool,
    inner: InMemoryConsensusStorage,
    journal: InMemoryBitcoinJournal,
    outcome: MintingOutcome,
    undo: ConsensusUndo,
    reject_reads: AtomicBool,
    writes: Mutex<Vec<ApplyStage>>,
    fail_after_write: Mutex<Option<usize>>,
}

impl Consensus {
    fn before_read(&self) -> Result<(), &'static str> {
        if self.reject_reads.load(Ordering::SeqCst) {
            Err("undo read rejected")
        } else {
            Ok(())
        }
    }

    async fn before_write(&self, stage: ApplyStage) {
        let entry = self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .unwrap()
            .operation
            .entry;
        assert_eq!(entry.minting_outcome, self.outcome);
        assert_eq!(entry.consensus_undo, self.undo);
        assert_eq!(entry.status, JournalStatus::Applying(stage));
        self.writes.lock().unwrap().push(stage);
    }

    fn after_write(&self) -> Result<(), &'static str> {
        let mut remaining = self.fail_after_write.lock().unwrap();
        if let Some(count) = remaining.as_mut() {
            *count -= 1;
            if *count == 0 {
                *remaining = None;
                return Err("interrupted after storage write");
            }
        }
        Ok(())
    }
}

impl ConsensusStorage for Consensus {
    type Error = &'static str;

    async fn get_minter_endorsement_state(
        &self,
        minter: &MinterP2wsh,
    ) -> Result<MinterEndorsementState, Self::Error> {
        self.before_read()?;
        self.inner
            .get_minter_endorsement_state(minter)
            .await
            .map_err(|_| "storage read failed")
    }

    async fn remove_endorsement_inclusion(
        &self,
        included: &IncludedEndorsement,
    ) -> Result<(), Self::Error> {
        self.inner
            .remove_endorsement_inclusion(included)
            .await
            .map_err(|_| "storage write failed")?;
        self.after_write()
    }

    async fn get_acquisition(
        &self,
        index: AcquisitionIndex,
    ) -> Result<Option<IncludedAcquisition>, Self::Error> {
        self.before_read()?;
        self.inner
            .get_acquisition(index)
            .await
            .map_err(|_| "storage read failed")
    }

    async fn get_minter_endorsement_for_height(
        &self,
        minter: &MinterP2wsh,
        height: CoreFlameHeight,
    ) -> Result<Option<WeightedEndorsement>, Self::Error> {
        self.before_read()?;
        self.inner
            .get_minter_endorsement_for_height(minter, height)
            .await
            .map_err(|_| "storage read failed")
    }

    async fn get_pending_endorsement(
        &self,
        tip: CoreBlockTip,
        index: EndorsementIndex,
    ) -> Result<Option<IncludedEndorsement>, Self::Error> {
        self.before_read()?;
        self.inner
            .get_pending_endorsement(tip, index)
            .await
            .map_err(|_| "storage read failed")
    }

    async fn get_cumulative_weight(
        &self,
        tip: BlockTip,
    ) -> Result<Option<WeightedBlockHeader>, Self::Error> {
        self.before_read()?;
        self.inner
            .get_cumulative_weight(tip)
            .await
            .map_err(|_| "storage read failed")
    }

    async fn get_double_sign(
        &self,
        minter: &MinterP2wsh,
        height: CoreFlameHeight,
    ) -> Result<Option<DoubleSign>, Self::Error> {
        self.before_read()?;
        self.inner
            .get_double_sign(minter, height)
            .await
            .map_err(|_| "storage read failed")
    }

    async fn remove_endorsement(
        &self,
        endorsement: &WeightedEndorsement,
    ) -> Result<(), Self::Error> {
        self.before_write(ApplyStage::RemoveEndorsements).await;
        self.inner
            .remove_endorsement(endorsement)
            .await
            .map_err(|_| "storage write failed")?;
        self.after_write()
    }

    async fn store_endorsement(
        &self,
        tip: BtcBlockTip,
        endorsement: &WeightedEndorsement,
    ) -> Result<(), Self::Error> {
        self.before_write(ApplyStage::StoreEndorsements).await;
        self.inner
            .store_endorsement(tip, endorsement)
            .await
            .map_err(|_| "storage write failed")?;
        self.after_write()
    }

    async fn store_acquisition(
        &self,
        tip: BtcBlockTip,
        acquisition: &IncludedAcquisition,
    ) -> Result<(), Self::Error> {
        self.before_write(ApplyStage::StoreAcquisitions).await;
        self.inner
            .store_acquisition(tip, acquisition)
            .await
            .map_err(|_| "storage write failed")?;
        self.after_write()
    }

    async fn store_cumulative_weight(
        &self,
        tip: BlockTip,
        weighted: &WeightedBlockHeader,
    ) -> Result<(), Self::Error> {
        if self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .is_some_and(|entry| matches!(entry.operation.entry.status, JournalStatus::Applying(_)))
        {
            self.before_write(ApplyStage::StoreBlockWeights).await;
        }
        self.inner
            .store_cumulative_weight(tip, weighted)
            .await
            .map_err(|_| "storage write failed")?;
        self.after_write()
    }

    async fn add_double_sign(&self, sign: &DoubleSign) -> Result<(), Self::Error> {
        self.before_write(ApplyStage::StoreDoubleSigns).await;
        self.inner
            .add_double_sign(sign)
            .await
            .map_err(|_| "storage write failed")?;
        self.after_write()
    }

    async fn store_pending_endorsement(
        &self,
        tip: CoreBlockTip,
        endorsement: &IncludedEndorsement,
    ) -> Result<(), Self::Error> {
        self.before_write(ApplyStage::StorePendingEndorsements)
            .await;
        self.inner
            .store_pending_endorsement(tip, endorsement)
            .await
            .map_err(|_| "storage write failed")?;
        self.after_write()
    }
    async fn get_block_tip_with_most_weight(&self) -> Result<Option<BlockTip>, Self::Error> {
        self.selections.fetch_add(1, Ordering::SeqCst);
        if self.reject_selection.load(Ordering::SeqCst) {
            return Err("selection rejected");
        }
        let rollback = self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .is_some_and(|entry| {
                matches!(entry.operation.entry.status, JournalStatus::RollingBack(_))
            });
        Ok(Some(if rollback {
            self.previous_winner
        } else {
            self.winner
        }))
    }
    async fn restore_acquisition(
        &self,
        index: AcquisitionIndex,
        previous: Option<&IncludedAcquisition>,
    ) -> Result<(), Self::Error> {
        self.inner
            .restore_acquisition(index, previous)
            .await
            .map_err(|_| "restore failed")?;
        self.after_write()
    }
    async fn restore_endorsement(
        &self,
        minter: &MinterP2wsh,
        height: CoreFlameHeight,
        previous: Option<&WeightedEndorsement>,
    ) -> Result<(), Self::Error> {
        self.inner
            .restore_endorsement(minter, height, previous)
            .await
            .map_err(|_| "restore failed")?;
        self.after_write()
    }
    async fn restore_pending_endorsement(
        &self,
        tip: CoreBlockTip,
        index: EndorsementIndex,
        previous: Option<&IncludedEndorsement>,
    ) -> Result<(), Self::Error> {
        if matches!(
            self.journal
                .pending_entry()
                .await
                .unwrap()
                .map(|entry| entry.operation.entry.status),
            Some(JournalStatus::Applying(
                ApplyStage::StorePendingEndorsements
            ))
        ) {
            self.before_write(ApplyStage::StorePendingEndorsements)
                .await;
        }
        self.inner
            .restore_pending_endorsement(tip, index, previous)
            .await
            .map_err(|_| "restore failed")?;
        self.after_write()
    }
    async fn restore_cumulative_weight(
        &self,
        tip: BlockTip,
        previous: Option<&WeightedBlockHeader>,
    ) -> Result<(), Self::Error> {
        self.inner
            .restore_cumulative_weight(tip, previous)
            .await
            .map_err(|_| "restore failed")?;
        self.after_write()
    }
    async fn restore_double_sign(
        &self,
        minter: &MinterP2wsh,
        height: CoreFlameHeight,
        previous: Option<&DoubleSign>,
    ) -> Result<(), Self::Error> {
        self.inner
            .restore_double_sign(minter, height, previous)
            .await
            .map_err(|_| "restore failed")?;
        self.after_write()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    Begin,
    BeginRollback,
    FlamePlan,
    RewardsPlan,
    Apply(ApplyStage),
    Restore(RollbackStage),
}
struct Journal {
    inner: InMemoryBitcoinJournal,
    fault: Mutex<Option<(Point, bool)>>,
}
impl Journal {
    fn check(&self, point: Point, after: bool) -> Result<(), &'static str> {
        let mut fault = self.fault.lock().unwrap();
        if *fault == Some((point, after)) {
            *fault = None;
            return Err("journal interrupted");
        }
        Ok(())
    }
}
impl BitcoinJournal for Journal {
    type Error = &'static str;

    async fn begin_apply(
        &self,
        previous: BtcBlockTip,
        block: &IndexedBlock,
        outcome: &MintingOutcome,
        undo: &ConsensusUndo,
    ) -> Result<OperationId, Self::Error> {
        self.check(Point::Begin, false)?;
        let id = self
            .inner
            .begin_apply(previous, block, outcome, undo)
            .await
            .map_err(|_| "journal operation failed")?;
        self.check(Point::Begin, true)?;
        Ok(id)
    }

    async fn store_flame_transition(
        &self,
        id: OperationId,
        transition: &FlameTransition,
    ) -> Result<(), Self::Error> {
        self.check(Point::FlamePlan, false)?;
        self.inner
            .store_flame_transition(id, transition)
            .await
            .map_err(|_| "journal operation failed")?;
        self.check(Point::FlamePlan, true)
    }

    async fn store_rewards_changes(
        &self,
        id: OperationId,
        changes: &RewardsChanges,
    ) -> Result<(), Self::Error> {
        self.check(Point::RewardsPlan, false)?;
        self.inner
            .store_rewards_changes(id, changes)
            .await
            .map_err(|_| "journal operation failed")?;
        self.check(Point::RewardsPlan, true)
    }

    async fn complete_apply_stage(
        &self,
        id: OperationId,
        stage: ApplyStage,
    ) -> Result<(), Self::Error> {
        self.check(Point::Apply(stage), false)?;
        self.inner
            .complete_apply_stage(id, stage)
            .await
            .map_err(|_| "journal operation failed")?;
        self.check(Point::Apply(stage), true)
    }

    async fn begin_rollback(
        &self,
        tip: BtcBlockTip,
        plan: &BitcoinRollbackPlan,
    ) -> Result<OperationId, Self::Error> {
        self.check(Point::BeginRollback, false)?;
        let id = self
            .inner
            .begin_rollback(tip, plan)
            .await
            .map_err(|_| "journal operation failed")?;
        self.check(Point::BeginRollback, true)?;
        Ok(id)
    }

    async fn complete_rollback_stage(
        &self,
        id: OperationId,
        stage: RollbackStage,
    ) -> Result<(), Self::Error> {
        self.check(Point::Restore(stage), false)?;
        self.inner
            .complete_rollback_stage(id, stage)
            .await
            .map_err(|_| "journal operation failed")?;
        self.check(Point::Restore(stage), true)
    }

    async fn get_applied_endorsements(&self) -> Result<Vec<IncludedEndorsement>, Self::Error> {
        self.inner
            .get_applied_endorsements()
            .await
            .map_err(|_| "history failed")
    }

    async fn get_entry(
        &self,
        tip: BtcBlockTip,
    ) -> Result<Option<BitcoinJournalEntry>, Self::Error> {
        self.inner.get_entry(tip).await.map_err(|_| "entry failed")
    }

    async fn pending_entry(&self) -> Result<Option<BitcoinJournalEntry>, Self::Error> {
        self.inner
            .pending_entry()
            .await
            .map_err(|_| "pending failed")
    }
}

struct Engine {
    rollback: BitcoinRollbackPlan,
    rollback_calls: AtomicUsize,
    outcome: MintingOutcome,
    calls: AtomicUsize,
}
impl ConsensusEngine for Engine {
    type Error = &'static str;

    async fn get_rollback_plan(
        &self,
        input: BitcoinRollbackInput,
    ) -> Result<BitcoinRollbackPlan, Self::Error> {
        self.rollback_calls.fetch_add(1, Ordering::SeqCst);
        let mut plan = self.rollback.clone();
        plan.included_endorsements = input.included_endorsements;
        plan.acquisition_undo = input.acquisition_undo;
        Ok(plan)
    }

    async fn get_endorsements_outcome(
        &self,
        _: Vec<IncludedEndorsement>,
    ) -> Result<ConsensusOutcome, Self::Error> {
        panic!("unexpected endorsement processing")
    }

    async fn get_minting_outcome(
        &self,
        block: IndexedBlock,
    ) -> Result<MintingOutcome, Self::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::task::yield_now().await;
        assert_eq!(block.btc_block_tip, self.outcome.next_btc_cursor);
        Ok(self.outcome.clone())
    }
}
type Processor = BitcoinBlockProcessor<
    Engine,
    Consensus,
    Chain,
    Journal,
    Storage,
    Rewards,
    Cursor,
    TestChainStorage,
>;
struct Fixture {
    processor: Processor,
    journal: InMemoryConsensusJournal,
    states: Vec<Blockchain>,
}
impl Fixture {
    async fn new() -> Self {
        let ancestor = Blockchain::new(ChainParams::default()).unwrap();
        let mut old = ancestor.clone();
        let old_first = old.build_block([1; 32], Vec::new()).unwrap();
        old.connect(&old_first).unwrap();
        let old_intermediate = old.clone();
        let old_second = old.build_block([2; 32], Vec::new()).unwrap();
        old.connect(&old_second).unwrap();
        let mut new = ancestor.clone();
        let mut new_first = new.build_block([3; 32], Vec::new()).unwrap();
        new_first.header.core_block = Some(CoreBlockHeader {
            height: 1.into(),
            target_btc_height: 101,
        });
        new.connect(&new_first).unwrap();
        let new_intermediate = new.clone();
        let new_second = new.build_block([4; 32], Vec::new()).unwrap();
        new.connect(&new_second).unwrap();
        let transition = FlameTransition {
            previous_tip: tip(&old),
            next_tip: tip(&new),
            path: ChainPath {
                detach: vec![old_second.header.block_tip(), old_first.header.block_tip()],
                attach: vec![new_first.header.block_tip(), new_second.header.block_tip()],
            },
            attached_core_headers: vec![new_first.header.clone()],
        };
        let inner = InMemoryConsensusStorage::new();
        let mut chain = Blockchain::new(ChainParams::default()).unwrap();
        let mut header = chain.build_block([1; 32], Vec::new()).unwrap().header;
        header.core_block = Some(CoreBlockHeader {
            height: 1.into(),
            target_btc_height: 100,
        });
        let mut weighted = WeightedBlockHeader {
            header,
            parent_weight: 10,
            effective_power: 16,
        };
        inner
            .store_cumulative_weight(weighted.block_tip(), &weighted)
            .await
            .unwrap();
        weighted.effective_power = 0;
        let old_endorsement = WeightedEndorsement {
            original: included(0x51, 1),
            effective_minting_power: 16,
        };
        inner
            .store_endorsement(old_endorsement.original.btc_block, &old_endorsement)
            .await
            .unwrap();
        let pending = included(0x53, 3);
        let resolved_pending = included(0x54, 3);
        inner
            .store_pending_endorsement(resolved_pending.endorsement.block_tip(), &resolved_pending)
            .await
            .unwrap();
        let outcome = MintingOutcome {
            consensus: ConsensusOutcome {
                removed_endorsements: vec![old_endorsement.clone()],
                accepted_endorsements: vec![WeightedEndorsement {
                    original: included(0x52, 2),
                    effective_minting_power: 8,
                }],
                accepted_acquisitions: vec![acquisition(10), acquisition(20)],
                weighted_blocks: HashMap::from([(weighted.block_tip(), weighted)]),
                double_signs: vec![DoubleSign {
                    minter: old_endorsement.original.endorsement.auth().minter().p2wsh(),
                    target_flame_height: 1.into(),
                    endorsements: vec![old_endorsement.original, included(0x51, 9)],
                }],
                pending_endorsements: BTreeMap::from([(
                    pending.endorsement.block_tip(),
                    vec![pending],
                )]),
                removed_pending_endorsements: BTreeSet::from([(
                    resolved_pending.endorsement.block_tip(),
                    EndorsementIndex::from(&resolved_pending.endorsement),
                )]),
            },
            next_btc_cursor: btc_tip(101),
        };
        let undo = ConsensusUndoManager {
            consensus_storage: &inner,
        }
        .prepare(&outcome.consensus)
        .await
        .unwrap();
        let owner = InMemoryConsensusJournal::new(btc_tip(100));
        let journal = owner.bitcoin_journal();
        let consensus = Consensus(Arc::new(ConsensusData {
            inner,
            journal: journal.clone(),
            outcome: outcome.clone(),
            undo: undo.clone(),
            previous_winner: tip(&old),
            winner: tip(&new),
            selections: AtomicUsize::new(0),
            reject_selection: AtomicBool::new(false),
            reject_reads: AtomicBool::new(false),
            writes: Mutex::new(Vec::new()),
            fail_after_write: Mutex::new(None),
        }));
        let processor = BitcoinBlockProcessor::new(
            Engine {
                rollback: BitcoinRollbackPlan {
                    endorsements: undo.endorsements.clone(),
                    pending_endorsements: undo.pending_endorsements.clone(),
                    double_signs: undo.double_signs.clone(),
                    weighted_blocks: undo
                        .weighted_blocks
                        .into_iter()
                        .filter_map(|(tip, value)| value.map(|value| (tip, value)))
                        .collect(),
                    ..BitcoinRollbackPlan::default()
                },
                rollback_calls: AtomicUsize::new(0),
                outcome,
                calls: AtomicUsize::new(0),
            },
            consensus.clone(),
            Chain {
                blocks: [old_first, old_second, new_first, new_second]
                    .into_iter()
                    .map(|block| (block.header.block_tip(), Arc::new(block)))
                    .collect(),
                transition: transition.clone(),
                plans: AtomicUsize::new(0),
                reject_plans: AtomicBool::new(false),
            },
            Journal {
                inner: journal.clone(),
                fault: Mutex::new(None),
            },
            Storage {
                state: Mutex::new(StoredState {
                    chain: old.clone(),
                    commits: Vec::new(),
                    failure: None,
                }),
                journal: journal.clone(),
                transition,
            },
            Rewards {
                inner: InMemoryRewardsStorage::new(),
                journal: journal.clone(),
                fail_after_write: AtomicBool::new(false),
            },
            Cursor {
                inner: InMemoryCursorStorage::new(btc_tip(100)),
                journal,
                fault: Mutex::new(None),
            },
            RewardsEngine {
                chain_storage: Arc::new(TestChainStorage::default()),
                consensus_storage: Arc::new(consensus),
                protocol_params: protocol_params(),
            },
        );
        Self {
            processor,
            journal: owner,
            states: vec![old, old_intermediate, ancestor, new_intermediate, new],
        }
    }
    fn block(&self) -> IndexedBlock {
        IndexedBlock {
            btc_block_tip: btc_tip(101),
            acquisitions: vec![],
            endorsements: vec![],
        }
    }
    async fn entry(&self) -> BitcoinBlockJournalEntry {
        self.processor
            .journal
            .get_entry(btc_tip(101))
            .await
            .unwrap()
            .unwrap()
            .operation
            .entry
    }
    fn allow_planning(&self) {
        self.processor
            .consensus_storage
            .reject_reads
            .store(false, Ordering::SeqCst);
        self.processor
            .consensus_storage
            .reject_selection
            .store(false, Ordering::SeqCst);
        self.processor
            .chain
            .reject_plans
            .store(false, Ordering::SeqCst);
    }
    async fn no_replanning(&self) {
        self.processor
            .consensus_storage
            .reject_reads
            .store(true, Ordering::SeqCst);
        let planned = self.entry().await.flame_transition.is_some();
        self.processor
            .consensus_storage
            .reject_selection
            .store(planned, Ordering::SeqCst);
        self.processor
            .chain
            .reject_plans
            .store(planned, Ordering::SeqCst);
    }
    async fn snapshot(&self) -> ConsensusUndo {
        ConsensusUndoManager {
            consensus_storage: &self.processor.consensus_storage.inner,
        }
        .prepare(&self.processor.engine.outcome.consensus)
        .await
        .unwrap()
    }
    async fn assert_applied(&self) {
        let entry = self.entry().await;
        assert_eq!(entry.status, JournalStatus::Applied);
        assert_eq!(entry.consensus_undo, self.processor.consensus_storage.undo);
        assert_eq!(
            entry.flame_transition,
            Some(self.processor.chain.transition.clone())
        );
        assert!(entry.rewards_changes.is_some());
        for &(tip, index) in &entry.minting_outcome.consensus.removed_pending_endorsements {
            assert_eq!(
                self.processor
                    .consensus_storage
                    .inner
                    .get_pending_endorsement(tip, index)
                    .await
                    .unwrap(),
                None
            );
        }
        assert!(
            self.processor
                .journal
                .pending_entry()
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            self.processor.cursor_storage.get_cursor().await.unwrap(),
            btc_tip(101)
        );
        let state = self.processor.canonical_storage.state.lock().unwrap();
        assert_eq!(tip(&state.chain), tip(&self.states[4]));
        assert_eq!(
            state.chain.state_commitment(),
            self.states[4].state_commitment()
        );
    }
    async fn assert_restored(&self) {
        assert_eq!(self.entry().await.status, JournalStatus::RolledBack);
        assert_eq!(self.snapshot().await, self.processor.consensus_storage.undo);
        assert_eq!(
            self.processor.cursor_storage.get_cursor().await.unwrap(),
            btc_tip(100)
        );
        let state = self.processor.canonical_storage.state.lock().unwrap();
        assert_eq!(tip(&state.chain), tip(&self.states[0]));
        assert_eq!(
            state.chain.state_commitment(),
            self.states[0].state_commitment()
        );
    }
}

fn apply_stages() -> Vec<ApplyStage> {
    let mut status = JournalStatus::Applying(ApplyStage::RemoveEndorsements);
    let mut stages = Vec::new();
    while let JournalStatus::Applying(stage) = status {
        stages.push(stage);
        status = status.next().unwrap();
    }
    assert_eq!(stages.len(), 11);
    stages
}

#[tokio::test]
async fn apply_completes_the_whole_block_and_rollback_restores_all_consensus_records() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    assert_eq!(p.apply(fixture.block()).await.unwrap(), btc_tip(101));
    fixture.assert_applied().await;
    assert_ne!(fixture.snapshot().await, p.consensus_storage.undo);
    assert_eq!(
        p.canonical_storage.state.lock().unwrap().commits,
        fixture.states[1..].iter().map(tip).collect::<Vec<_>>()
    );
    fixture.no_replanning().await;
    assert_eq!(p.apply(fixture.block()).await.unwrap(), btc_tip(101));
    assert_eq!(p.engine.calls.load(Ordering::SeqCst), 1);
    fixture.allow_planning();
    assert_eq!(p.rollback(btc_tip(101)).await.unwrap(), btc_tip(100));
    fixture.assert_restored().await;
    assert_eq!(p.rollback(btc_tip(101)).await.unwrap(), btc_tip(100));
    assert_eq!(p.canonical_storage.state.lock().unwrap().commits.len(), 8);
}

#[tokio::test]
async fn resumes_before_and_after_every_apply_stage_marker_without_recomputing_undo() {
    for stage in apply_stages() {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            *p.journal.fault.lock().unwrap() = Some((Point::Apply(stage), after));
            assert!(matches!(
                p.apply(fixture.block()).await,
                Err(BlockProcessingError::Journal("journal interrupted"))
            ));
            let saved = fixture.entry().await;
            p.consensus_storage
                .reject_reads
                .store(true, Ordering::SeqCst);
            if saved.flame_transition.is_some() {
                fixture.no_replanning().await;
            }
            assert_eq!(p.apply(fixture.block()).await.unwrap(), btc_tip(101));
            fixture.assert_applied().await;
            assert_eq!(p.engine.calls.load(Ordering::SeqCst), 1);
            assert_eq!(p.canonical_storage.state.lock().unwrap().commits.len(), 4);
        }
    }
}

#[tokio::test]
async fn retries_plan_persistence_without_overwriting_saved_plans() {
    for point in [Point::Begin, Point::FlamePlan, Point::RewardsPlan] {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            *p.journal.fault.lock().unwrap() = Some((point, after));
            assert!(matches!(
                p.apply(fixture.block()).await,
                Err(BlockProcessingError::Journal("journal interrupted"))
            ));
            if let Some(entry) = p.journal.pending_entry().await.unwrap() {
                p.consensus_storage
                    .reject_reads
                    .store(true, Ordering::SeqCst);
                if entry.operation.entry.flame_transition.is_some() {
                    fixture.no_replanning().await;
                }
            }
            p.apply(fixture.block()).await.unwrap();
            fixture.assert_applied().await;
            assert_eq!(
                p.engine.calls.load(Ordering::SeqCst),
                if point == Point::Begin && !after {
                    2
                } else {
                    1
                }
            );
        }
    }
}

#[tokio::test]
async fn retries_a_partially_written_consensus_stage_using_the_original_undo() {
    for write in 1..=8 {
        let fixture = Fixture::new().await;
        let p = &fixture.processor;
        *p.consensus_storage.fail_after_write.lock().unwrap() = Some(write);
        assert!(matches!(
            p.apply(fixture.block()).await,
            Err(BlockProcessingError::Consensus(
                "interrupted after storage write"
            ))
        ));
        p.consensus_storage
            .reject_reads
            .store(true, Ordering::SeqCst);
        p.resume().await.unwrap();
        fixture.assert_applied().await;
        fixture.allow_planning();
        p.rollback(btc_tip(101)).await.unwrap();
        fixture.assert_restored().await;
    }
}

#[tokio::test]
async fn resumes_every_flame_commit_during_application_and_rollback() {
    for rollback in [false, true] {
        for step in 1..=4 {
            for after in [false, true] {
                let fixture = Fixture::new().await;
                let p = &fixture.processor;
                if rollback {
                    p.apply(fixture.block()).await.unwrap();
                }
                p.canonical_storage.state.lock().unwrap().failure =
                    Some((if rollback { 4 + step } else { step }, after));
                let result = if rollback {
                    p.rollback(btc_tip(101)).await
                } else {
                    p.apply(fixture.block()).await
                };
                assert!(matches!(result, Err(BlockProcessingError::Flame(_))));
                fixture.no_replanning().await;
                p.resume().await.unwrap();
                if rollback {
                    fixture.assert_restored().await;
                } else {
                    fixture.assert_applied().await;
                }
                assert_eq!(
                    p.canonical_storage.state.lock().unwrap().commits.len(),
                    if rollback { 8 } else { 4 }
                );
                assert_eq!(
                    p.chain.plans.load(Ordering::SeqCst),
                    if rollback { 2 } else { 1 }
                );
            }
        }
    }
}

#[tokio::test]
async fn resumes_every_rollback_marker_and_the_begin_record() {
    for point in [Point::BeginRollback, Point::FlamePlan, Point::RewardsPlan]
        .into_iter()
        .chain(
            [
                RollbackStage::RemoveEndorsements,
                RollbackStage::RestoreAcquisitions,
                RollbackStage::StoreDoubleSigns,
                RollbackStage::StoreEndorsements,
                RollbackStage::StorePendingEndorsements,
                RollbackStage::StoreBlockWeights,
                RollbackStage::PrepareFlameTransition,
                RollbackStage::ApplyFlameTransition,
                RollbackStage::PrepareRewards,
                RollbackStage::ApplyRewards,
                RollbackStage::StoreCursor,
            ]
            .into_iter()
            .map(Point::Restore),
        )
    {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            p.apply(fixture.block()).await.unwrap();
            *p.journal.fault.lock().unwrap() = Some((point, after));
            assert!(matches!(
                p.rollback(btc_tip(101)).await,
                Err(BlockProcessingError::Journal("journal interrupted"))
            ));
            let saved = p.journal.pending_entry().await.unwrap().is_some();
            if saved {
                fixture.no_replanning().await;
            }
            p.rollback(btc_tip(101)).await.unwrap();
            assert_eq!(
                p.engine.rollback_calls.load(Ordering::SeqCst),
                if point == Point::BeginRollback && !after {
                    2
                } else {
                    1
                }
            );
            fixture.assert_restored().await;
        }
    }
}

#[tokio::test]
async fn resumes_a_partially_restored_consensus_undo() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    p.apply(fixture.block()).await.unwrap();
    *p.consensus_storage.fail_after_write.lock().unwrap() = Some(3);
    assert!(matches!(
        p.rollback(btc_tip(101)).await,
        Err(BlockProcessingError::Consensus(_))
    ));
    fixture.no_replanning().await;
    assert_eq!(p.resume().await.unwrap(), btc_tip(100));
    fixture.assert_restored().await;
}

struct ArrivalProbe(Arc<tokio::sync::Notify>);

impl crate::FlameBlockArrivalHandler for ArrivalProbe {
    type Error = std::convert::Infallible;
    async fn on_flame_block_arrival(&self, _: BlockTip) -> Result<(), Self::Error> {
        self.0.notify_one();
        Ok(())
    }
}

#[tokio::test]
async fn manager_serializes_bitcoin_calculation_and_the_entire_application() {
    use crate::block_processing::{ConsensusManager, test_support::ArrivalHandlerProcessor};
    let fixture = Fixture::new().await;
    let block = fixture.block();
    let done = Arc::new(tokio::sync::Notify::new());
    let manager = Arc::new(ConsensusManager::new(
        fixture.processor,
        ArrivalHandlerProcessor::new(ArrivalProbe(done.clone())),
        fixture.journal,
    ));
    manager.startup().await.unwrap();
    manager.new_bitcoin_block(block.clone()).unwrap();
    manager.new_bitcoin_block(block).unwrap();
    manager.new_flame_block(tip(&fixture.states[0])).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), done.notified())
        .await
        .unwrap();
    manager.shutdown().await.unwrap();
    let processor = manager.bitcoin_processor();
    assert_eq!(processor.engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(processor.consensus_storage.writes.lock().unwrap().len(), 8);
    assert_eq!(
        processor.cursor_storage.get_cursor().await.unwrap(),
        btc_tip(101)
    );
    assert!(processor.journal.pending_entry().await.unwrap().is_none());
}

#[tokio::test]
async fn manager_startup_resumes_the_saved_bitcoin_operation_without_recalculation() {
    use crate::block_processing::{ConsensusManager, test_support::ArrivalHandlerProcessor};
    let fixture = Fixture::new().await;
    *fixture.processor.journal.fault.lock().unwrap() =
        Some((Point::Apply(ApplyStage::ApplyFlameTransition), true));
    assert!(fixture.processor.apply(fixture.block()).await.is_err());
    fixture.no_replanning().await;
    let original_undo = fixture.entry().await.consensus_undo;
    let manager = Arc::new(ConsensusManager::new(
        fixture.processor,
        ArrivalHandlerProcessor::new(ArrivalProbe(Arc::new(tokio::sync::Notify::new()))),
        fixture.journal.clone(),
    ));
    assert_eq!(manager.startup().await.unwrap(), btc_tip(101));
    manager.shutdown().await.unwrap();
    assert_eq!(
        manager
            .bitcoin_processor()
            .engine
            .calls
            .load(Ordering::SeqCst),
        1
    );
    let latest = fixture.journal.latest_entry().await.unwrap().unwrap();
    assert!(latest.is_complete());
    let crate::block_processing::journal::consensus::ConsensusOperation::Bitcoin(operation) =
        latest.operation
    else {
        panic!()
    };
    assert_eq!(operation.entry.consensus_undo, original_undo);
}

#[tokio::test]
async fn a_rolled_back_block_can_be_applied_again_with_fresh_undo() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    p.apply(fixture.block()).await.unwrap();
    p.rollback(btc_tip(101)).await.unwrap();
    p.apply(fixture.block()).await.unwrap();
    fixture.assert_applied().await;
    assert_eq!(p.engine.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn undo_read_failure_does_not_begin_an_operation() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    p.consensus_storage
        .reject_reads
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        p.apply(fixture.block()).await,
        Err(BlockProcessingError::Consensus("undo read rejected"))
    ));
    assert!(p.journal.pending_entry().await.unwrap().is_none());
    assert!(p.consensus_storage.writes.lock().unwrap().is_empty());
    assert_eq!(p.resume().await.unwrap(), btc_tip(100));
}

#[tokio::test]
async fn rejects_invalid_block_order_before_calculation() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    let mut block = fixture.block();
    block.btc_block_tip = btc_tip(102);
    assert!(matches!(
        p.apply(block).await,
        Err(BlockProcessingError::InvalidSuccessor { .. })
    ));
    assert_eq!(p.engine.calls.load(Ordering::SeqCst), 0);
    p.apply(fixture.block()).await.unwrap();
    assert!(matches!(
        p.rollback(btc_tip(100)).await,
        Err(BlockProcessingError::MissingEntry(_))
    ));
    fixture.assert_applied().await;
}

use crate::rewards::{PendingReward, RewardIndex, RewardsOutcome, RewardsUndo};
struct Rewards {
    inner: InMemoryRewardsStorage,
    journal: InMemoryBitcoinJournal,
    fail_after_write: AtomicBool,
}
impl RewardsStorage for Rewards {
    type Error = &'static str;
    async fn get_pending_rewards(
        &self,
        height: CoreFlameHeight,
    ) -> Result<Vec<PendingReward>, Self::Error> {
        self.inner
            .get_pending_rewards(height)
            .await
            .map_err(|_| "reward read failed")
    }
    async fn get_pending_reward(
        &self,
        height: CoreFlameHeight,
        index: RewardIndex,
    ) -> Result<Option<PendingReward>, Self::Error> {
        self.inner
            .get_pending_reward(height, index)
            .await
            .map_err(|_| "reward read failed")
    }
    async fn store_pending_reward(
        &self,
        height: CoreFlameHeight,
        reward: &PendingReward,
    ) -> Result<(), Self::Error> {
        let entry = self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .unwrap()
            .operation
            .entry;
        assert_eq!(
            entry.status,
            JournalStatus::Applying(ApplyStage::ApplyRewards)
        );
        assert!(entry.rewards_changes.is_some());
        self.inner
            .store_pending_reward(height, reward)
            .await
            .map_err(|_| "reward write failed")?;
        if self.fail_after_write.swap(false, Ordering::SeqCst) {
            return Err("reward interrupted");
        }
        Ok(())
    }
    async fn restore_pending_reward(
        &self,
        height: CoreFlameHeight,
        index: RewardIndex,
        previous: Option<&PendingReward>,
    ) -> Result<(), Self::Error> {
        self.inner
            .restore_pending_reward(height, index, previous)
            .await
            .map_err(|_| "reward restore failed")?;
        if self.fail_after_write.swap(false, Ordering::SeqCst) {
            return Err("reward interrupted");
        }
        Ok(())
    }
}
struct Cursor {
    inner: InMemoryCursorStorage,
    journal: InMemoryBitcoinJournal,
    fault: Mutex<Option<bool>>,
}
impl CursorStorage for Cursor {
    type Error = &'static str;
    async fn get_cursor(&self) -> Result<BtcBlockTip, Self::Error> {
        self.inner
            .get_cursor()
            .await
            .map_err(|_| "cursor read failed")
    }
    async fn store_cursor(&self, tip: BtcBlockTip) -> Result<(), Self::Error> {
        let entry = self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .unwrap()
            .operation
            .entry;
        assert!(matches!(
            entry.status,
            JournalStatus::Applying(ApplyStage::StoreCursor)
                | JournalStatus::RollingBack(RollbackStage::StoreCursor)
        ));
        let failure = self.fault.lock().unwrap().take();
        if failure == Some(false) {
            return Err("cursor interrupted");
        }
        self.inner
            .store_cursor(tip)
            .await
            .map_err(|_| "cursor write failed")?;
        if failure == Some(true) {
            return Err("cursor interrupted");
        }
        Ok(())
    }
}

#[tokio::test]
async fn resumes_cursor_writes_before_and_after_persistence_in_both_directions() {
    for rollback in [false, true] {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            if rollback {
                p.apply(fixture.block()).await.unwrap();
            }
            *p.cursor_storage.fault.lock().unwrap() = Some(after);
            let result = if rollback {
                p.rollback(btc_tip(101)).await
            } else {
                p.apply(fixture.block()).await
            };
            assert!(matches!(
                result,
                Err(BlockProcessingError::Cursor("cursor interrupted"))
            ));
            fixture.no_replanning().await;
            p.resume().await.unwrap();
            if rollback {
                fixture.assert_restored().await;
            } else {
                fixture.assert_applied().await;
            }
        }
    }
}

#[tokio::test]
async fn saved_rewards_are_replayed_and_restored_after_interrupted_writes() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    *p.journal.fault.lock().unwrap() = Some((Point::Apply(ApplyStage::ApplyFlameTransition), true));
    assert!(p.apply(fixture.block()).await.is_err());
    let reward = PendingReward {
        flame_predicate: Predicate::opaque(Predicate::unspendable_key()),
        access_predicate: Predicate::opaque(Predicate::unspendable_key()),
        amount: 42,
    };
    let index = RewardIndex::from(&reward);
    let changes = RewardsChanges {
        outcomes: vec![RewardsOutcome {
            target_block_height: 200.into(),
            rewards: vec![reward],
        }],
        undo: RewardsUndo {
            rewards: BTreeMap::from([((200.into(), index), None)]),
        },
    };
    p.journal
        .store_rewards_changes(
            p.journal.pending_entry().await.unwrap().unwrap().id,
            &changes,
        )
        .await
        .unwrap();
    p.rewards_storage
        .fail_after_write
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        p.resume().await,
        Err(BlockProcessingError::Rewards("reward interrupted"))
    ));
    assert_eq!(p.cursor_storage.get_cursor().await.unwrap(), btc_tip(100));
    fixture.no_replanning().await;
    p.resume().await.unwrap();
    assert_eq!(
        p.rewards_storage
            .get_pending_reward(200.into(), index)
            .await
            .unwrap()
            .unwrap()
            .amount,
        42
    );
    fixture.allow_planning();
    p.rewards_storage
        .fail_after_write
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        p.rollback(btc_tip(101)).await,
        Err(BlockProcessingError::Rewards("reward interrupted"))
    ));
    p.resume().await.unwrap();
    assert!(
        p.rewards_storage
            .get_pending_reward(200.into(), index)
            .await
            .unwrap()
            .is_none()
    );
    fixture.assert_restored().await;
}
