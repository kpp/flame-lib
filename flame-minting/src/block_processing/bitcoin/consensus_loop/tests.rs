use super::*;
use crate::block_processing::{
    BlockProcessor, ConsensusManager, ConsensusManagerError,
    journal::consensus::InMemoryConsensusJournal,
    tests::support::{ArrivalHandlerProcessor, wait_until},
};
use btc_integration::HistoryChange;
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
    fail: bool,
}
impl BlockProcessor for Processor {
    type Error = &'static str;
    async fn apply(&self, block: IndexedBlock) -> Result<BtcBlockTip, Self::Error> {
        if self.fail {
            return Err("apply failed");
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
    let manager = ConsensusManager::new(
        processor(tip(3, 1)),
        ArrivalHandlerProcessor::new(ArrivalHandler),
        InMemoryConsensusJournal::new(tip(3, 1)),
    );
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
    manager.startup().await.unwrap();
    let mut cursor = tip(3, 1);
    ConsensusLoop::catch_up(&manager, &indexer, &mut cursor, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(cursor, tip(3, 2));
    wait_until(|| async { manager.bitcoin_processor().events.lock().unwrap().len() == 4 }).await;
    manager.shutdown().await.unwrap();
    assert_eq!(
        *manager.bitcoin_processor().events.lock().unwrap(),
        vec![tip(2, 1), tip(1, 1), tip(2, 2), tip(3, 2)]
    );
}

#[tokio::test]
async fn cancelled_indexer_loop_does_not_submit_more_events() {
    let manager = ConsensusManager::new(
        processor(tip(1, 1)),
        ArrivalHandlerProcessor::new(ArrivalHandler),
        InMemoryConsensusJournal::new(tip(1, 1)),
    );
    let indexer = Indexer(StdMutex::new(VecDeque::new()));
    manager.startup().await.unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    let mut cursor = tip(1, 1);
    ConsensusLoop::catch_up(&manager, &indexer, &mut cursor, &cancellation)
        .await
        .unwrap();
    manager.shutdown().await.unwrap();
    assert_eq!(cursor, tip(1, 1));
    assert!(
        manager
            .bitcoin_processor()
            .events
            .lock()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn queued_delivery_does_not_confirm_that_the_bitcoin_cursor_was_applied() {
    let mut bitcoin = processor(tip(1, 1));
    bitcoin.fail = true;
    let manager = ConsensusManager::new(
        bitcoin,
        ArrivalHandlerProcessor::new(ArrivalHandler),
        InMemoryConsensusJournal::new(tip(1, 1)),
    );
    let indexer = Indexer(StdMutex::new(VecDeque::from([(
        tip(1, 1),
        history(
            HistoryChange::Extension {
                new_blocks: vec![block(tip(2, 1))],
            },
            tip(2, 1),
        ),
    )])));
    manager.startup().await.unwrap();
    let mut cursor = tip(1, 1);
    ConsensusLoop::catch_up(&manager, &indexer, &mut cursor, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(cursor, tip(2, 1));
    assert_eq!(manager.stopped().await, crate::ConsensusStatus::Failed);
    assert!(matches!(
        manager.shutdown().await,
        Err(ConsensusManagerError::Bitcoin("apply failed"))
    ));
    assert_eq!(
        *manager.bitcoin_processor().cursor.lock().unwrap(),
        tip(1, 1)
    );
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
    let consensus = ConsensusLoop::new(ConsensusManager::new(
        RecoveryProbe(events.clone()),
        ArrivalHandlerProcessor::new(ArrivalHandler),
        InMemoryConsensusJournal::new(tip(2, 1)),
    ));
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

struct ArrivalHandler;

impl crate::FlameBlockArrivalHandler for ArrivalHandler {
    type Error = std::convert::Infallible;
    async fn on_flame_block_arrival(&self, _: flamechain::BlockTip) -> Result<(), Self::Error> {
        Ok(())
    }
}

struct ManagedIndexer {
    tip: watch::Sender<Option<BtcBlockTip>>,
    shutdown_done: tokio::sync::Notify,
}

impl ConsensusIndexer for ManagedIndexer {
    async fn startup(&self) -> Result<(), StartupError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), ShutdownError> {
        self.shutdown_done.notify_one();
        Ok(())
    }
    fn subscribe(&self) -> watch::Receiver<Option<BtcBlockTip>> {
        self.tip.subscribe()
    }
    async fn get_history(
        &self,
        cursor: BtcBlockTip,
        _: NonZeroUsize,
    ) -> Result<HistoryUpdate, HistoryError> {
        assert_eq!(cursor, tip(1, 1));
        Ok(history(
            HistoryChange::Extension {
                new_blocks: vec![block(tip(2, 1))],
            },
            tip(2, 1),
        ))
    }
}

#[tokio::test]
async fn queued_worker_failure_stops_the_indexer_loop_without_another_notification() {
    let mut bitcoin = processor(tip(1, 1));
    bitcoin.fail = true;
    let manager = ConsensusManager::new(
        bitcoin,
        ArrivalHandlerProcessor::new(ArrivalHandler),
        InMemoryConsensusJournal::new(tip(1, 1)),
    );
    let consensus = ConsensusLoop::new(manager);
    let indexer = Arc::new(ManagedIndexer {
        tip: watch::channel(Some(tip(2, 1))).0,
        shutdown_done: tokio::sync::Notify::new(),
    });
    consensus.startup(indexer.clone()).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        indexer.shutdown_done.notified(),
    )
    .await
    .unwrap();
    assert!(matches!(
        consensus.shutdown().await,
        Err(ConsensusLoopError::Process(ConsensusManagerError::Bitcoin(
            "apply failed"
        )))
    ));
    assert_eq!(
        consensus.manager().status(),
        crate::ConsensusStatus::Stopped
    );
}

#[tokio::test]
async fn indexer_startup_failure_shuts_down_the_ready_manager() {
    let events = Arc::new(StdMutex::new(Vec::new()));
    let manager = ConsensusManager::new(
        RecoveryProbe(events.clone()),
        ArrivalHandlerProcessor::new(ArrivalHandler),
        InMemoryConsensusJournal::new(tip(2, 1)),
    );
    let consensus = ConsensusLoop::new(manager);
    assert!(matches!(
        consensus.startup(Arc::new(RecoveryProbe(events))).await,
        Err(ConsensusLoopError::IndexerStartup(
            StartupError::WorkerStopped
        ))
    ));
    assert_eq!(
        consensus.manager().status(),
        crate::ConsensusStatus::Stopped
    );
}
