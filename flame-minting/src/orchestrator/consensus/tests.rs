use super::*;
use btc_integration::{HistoryUpdate, IndexedBlock};
use corepc_client::bitcoin::{BlockHash, hashes::Hash};
use std::{collections::VecDeque, sync::Mutex as StdMutex};

fn tip(height: u64, branch: u8) -> BtcBlockTip {
    BtcBlockTip {
        hash: BlockHash::from_byte_array([branch; 32]),
        height,
    }
}
fn block(tip: BtcBlockTip) -> IndexedBlock {
    IndexedBlock {
        btc_block_tip: tip,
        acquisitions: vec![],
        endorsements: vec![],
    }
}

struct Processor {
    cursor: StdMutex<BtcBlockTip>,
    events: StdMutex<Vec<BtcBlockTip>>,
    cancellation: CancellationToken,
    cancel_on_apply: bool,
    fail: bool,
}
impl BlockProcessor for Processor {
    type Error = &'static str;
    async fn apply(&self, block: IndexedBlock) -> Result<BtcBlockTip, Self::Error> {
        if self.fail {
            return Err("apply failed");
        }
        if self.cancel_on_apply {
            self.cancellation.cancel();
        }
        *self.cursor.lock().unwrap() = block.btc_block_tip;
        self.events.lock().unwrap().push(block.btc_block_tip);
        Ok(block.btc_block_tip)
    }
    async fn resume(&self) -> Result<BtcBlockTip, Self::Error> {
        Ok(*self.cursor.lock().unwrap())
    }
    async fn rollback(&self, removed: BtcBlockTip) -> Result<BtcBlockTip, Self::Error> {
        assert_eq!(*self.cursor.lock().unwrap(), removed);
        let previous = tip(removed.height - 1, 1);
        *self.cursor.lock().unwrap() = previous;
        self.events.lock().unwrap().push(previous);
        Ok(previous)
    }
}
struct Indexer(StdMutex<VecDeque<(BtcBlockTip, HistoryUpdate)>>);
impl ConsensusIndexer for Indexer {
    async fn get_history(
        &self,
        cursor: BtcBlockTip,
        _: NonZeroUsize,
    ) -> Result<HistoryUpdate, HistoryError> {
        let (expected, update) = self.0.lock().unwrap().pop_front().unwrap();
        assert_eq!(cursor, expected);
        Ok(update)
    }
}
fn processor(cursor: BtcBlockTip) -> Processor {
    Processor {
        cursor: StdMutex::new(cursor),
        events: StdMutex::new(vec![]),
        cancellation: CancellationToken::new(),
        cancel_on_apply: false,
        fail: false,
    }
}
fn history(change: HistoryChange, target: BtcBlockTip) -> HistoryUpdate {
    HistoryUpdate {
        target_tip: target,
        next_cursor: target,
        change,
    }
}

#[tokio::test]
async fn reorg_rolls_back_old_blocks_before_applying_the_new_branch() {
    let processor = processor(tip(3, 1));
    let indexer = Indexer(StdMutex::new(VecDeque::from([(
        tip(3, 1),
        history(
            HistoryChange::Reorg {
                removed_block_tips: vec![tip(3, 1), tip(2, 1)],
                new_blocks: vec![block(tip(2, 2)), block(tip(3, 2))],
            },
            tip(3, 2),
        ),
    )])));
    let mut cursor = tip(3, 1);
    ConsensusLoop::catch_up(&processor, &indexer, &mut cursor, &processor.cancellation)
        .await
        .unwrap();
    assert_eq!(cursor, tip(3, 2));
    assert_eq!(
        *processor.events.lock().unwrap(),
        vec![tip(2, 1), tip(1, 1), tip(2, 2), tip(3, 2)]
    );
}

#[tokio::test]
async fn cancellation_finishes_the_current_block_and_stops_before_the_next() {
    let mut processor = processor(tip(1, 1));
    processor.cancel_on_apply = true;
    let indexer = Indexer(StdMutex::new(VecDeque::from([(
        tip(1, 1),
        history(
            HistoryChange::Extension {
                new_blocks: vec![block(tip(2, 1)), block(tip(3, 1))],
            },
            tip(3, 1),
        ),
    )])));
    let mut cursor = tip(1, 1);
    ConsensusLoop::catch_up(&processor, &indexer, &mut cursor, &processor.cancellation)
        .await
        .unwrap();
    assert_eq!(cursor, tip(2, 1));
    assert_eq!(*processor.events.lock().unwrap(), vec![tip(2, 1)]);
}

#[tokio::test]
async fn a_failed_operation_does_not_advance_the_loop_cursor() {
    let mut processor = processor(tip(1, 1));
    processor.fail = true;
    let indexer = Indexer(StdMutex::new(VecDeque::from([(
        tip(1, 1),
        history(
            HistoryChange::Extension {
                new_blocks: vec![block(tip(2, 1))],
            },
            tip(2, 1),
        ),
    )])));
    let mut cursor = tip(1, 1);
    assert!(matches!(
        ConsensusLoop::catch_up(&processor, &indexer, &mut cursor, &processor.cancellation).await,
        Err(ConsensusLoopError::Process("apply failed"))
    ));
    assert_eq!(cursor, tip(1, 1));
}

struct RecoveryProbe(Arc<StdMutex<Vec<&'static str>>>);
impl BlockProcessor for RecoveryProbe {
    type Error = &'static str;
    async fn resume(&self) -> Result<BtcBlockTip, Self::Error> {
        self.0.lock().unwrap().push("recovered");
        Ok(tip(2, 1))
    }
    async fn apply(&self, _: IndexedBlock) -> Result<BtcBlockTip, Self::Error> {
        unreachable!()
    }
    async fn rollback(&self, _: BtcBlockTip) -> Result<BtcBlockTip, Self::Error> {
        unreachable!()
    }
}
impl ConsensusIndexer for RecoveryProbe {
    async fn startup(&self) -> Result<(), StartupError> {
        assert_eq!(*self.0.lock().unwrap(), vec!["recovered"]);
        self.0.lock().unwrap().push("indexer started");
        Err(StartupError::WorkerStopped)
    }
}

#[tokio::test]
async fn startup_finishes_recovery_before_starting_the_indexer() {
    let events = Arc::new(StdMutex::new(Vec::new()));
    let consensus = ConsensusLoop::new(RecoveryProbe(events.clone()));
    assert!(matches!(
        consensus
            .startup(Arc::new(RecoveryProbe(events.clone())))
            .await,
        Err(ConsensusLoopError::IndexerStartup(
            StartupError::WorkerStopped
        ))
    ));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["recovered", "indexer started"]
    );
}
