use std::{num::NonZeroUsize, sync::Arc};

use btc_integration::{BtcBlockTip, HistoryChange, HistoryError, ShutdownError, StartupError};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::ports::ConsensusIndexer;
use crate::block_processing::{
    BlockProcessor, ConsensusManager, ConsensusManagerError, FlameProcessor,
    journal::consensus::ConsensusJournal,
};
use crate::{ConsensusHandle, ConsensusSendError};

#[cfg(test)]
mod tests;

type ProcessingError<B, F, J> = ConsensusManagerError<
    <B as BlockProcessor>::Error,
    <F as FlameProcessor>::Error,
    <J as ConsensusJournal>::Error,
>;

type LoopResult<E> = Result<(), ConsensusLoopError<E>>;

#[derive(Debug)]
pub enum ConsensusLoopError<E> {
    AlreadyRunning,
    IndexerStartup(StartupError),
    IndexerShutdown(ShutdownError),
    History(HistoryError),
    SubscriptionClosed,
    Process(E),
    Delivery(ConsensusSendError),
    WorkerJoin(tokio::task::JoinError),
}

struct Worker<E> {
    cancellation: CancellationToken,
    task: JoinHandle<LoopResult<E>>,
}

impl<E> Drop for Worker<E> {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

pub struct ConsensusLoop<B: BlockProcessor, F: FlameProcessor, J: ConsensusJournal> {
    manager: Arc<ConsensusManager<B, F, J>>,
    worker: Mutex<Option<Worker<ProcessingError<B, F, J>>>>,
}

impl<B: BlockProcessor, F: FlameProcessor, J: ConsensusJournal> ConsensusLoop<B, F, J> {
    pub fn new(manager: ConsensusManager<B, F, J>) -> Self {
        Self {
            manager: Arc::new(manager),
            worker: Mutex::new(None),
        }
    }

    pub(super) fn manager(&self) -> &Arc<ConsensusManager<B, F, J>> {
        &self.manager
    }

    pub fn consensus_handle(&self) -> ConsensusHandle<B, F, J> {
        self.manager.consensus_handle()
    }

    pub async fn startup<I: ConsensusIndexer>(
        &self,
        indexer: Arc<I>,
    ) -> LoopResult<ProcessingError<B, F, J>> {
        let mut worker = self.worker.lock().await;
        if worker.is_some() {
            return Err(ConsensusLoopError::AlreadyRunning);
        }
        let cursor = self
            .manager
            .startup()
            .await
            .map_err(ConsensusLoopError::Process)?;
        if let Err(error) = indexer.startup().await {
            self.manager
                .shutdown()
                .await
                .map_err(ConsensusLoopError::Process)?;
            return Err(ConsensusLoopError::IndexerStartup(error));
        }
        let subscription = indexer.subscribe();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let manager = self.manager.clone();
        let task = tokio::spawn(async move {
            let result = Self::run(
                manager.as_ref(),
                indexer.as_ref(),
                subscription,
                cursor,
                task_cancellation,
            )
            .await;
            let shutdown = indexer
                .shutdown()
                .await
                .map_err(ConsensusLoopError::IndexerShutdown);
            let consensus_shutdown = manager
                .shutdown()
                .await
                .map_err(ConsensusLoopError::Process);
            consensus_shutdown.and(result).and(shutdown)
        });
        *worker = Some(Worker { cancellation, task });
        Ok(())
    }

    pub async fn shutdown(&self) -> LoopResult<ProcessingError<B, F, J>> {
        let mut slot = self.worker.lock().await;
        let Some(worker) = slot.as_mut() else {
            return Ok(());
        };
        worker.cancellation.cancel();
        self.manager.request_shutdown();
        let result = (&mut worker.task).await;
        *slot = None;
        match result {
            Ok(result) => result,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(error) => Err(ConsensusLoopError::WorkerJoin(error)),
        }
    }

    async fn run<I: ConsensusIndexer>(
        manager: &ConsensusManager<B, F, J>,
        indexer: &I,
        mut subscription: watch::Receiver<Option<BtcBlockTip>>,
        mut cursor: BtcBlockTip,
        cancellation: CancellationToken,
    ) -> LoopResult<ProcessingError<B, F, J>> {
        loop {
            let tip = *subscription.borrow_and_update();
            if tip.is_some_and(|tip| tip != cursor) {
                Self::catch_up(manager, indexer, &mut cursor, &cancellation).await?;
            }
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Ok(()),
                _ = manager.stopped() => return manager.shutdown().await.map_err(ConsensusLoopError::Process),
                result = subscription.changed() => { result.map_err(|_| ConsensusLoopError::SubscriptionClosed)?; }
            }
        }
    }

    async fn catch_up<I: ConsensusIndexer>(
        manager: &ConsensusManager<B, F, J>,
        indexer: &I,
        cursor: &mut BtcBlockTip,
        cancellation: &CancellationToken,
    ) -> LoopResult<ProcessingError<B, F, J>> {
        loop {
            let history = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Ok(()),
                _ = manager.stopped() => return manager.shutdown().await.map_err(ConsensusLoopError::Process),
                result = indexer.get_history(*cursor, NonZeroUsize::new(1000).unwrap()) => {
                    result.map_err(ConsensusLoopError::History)?
                }
            };
            let target_tip = history.target_tip;
            match history.change {
                HistoryChange::Extension { new_blocks } => {
                    for block in new_blocks {
                        if cancellation.is_cancelled() {
                            return Ok(());
                        }
                        let next = block.btc_block_tip;
                        match manager.new_bitcoin_block(block) {
                            Ok(()) => *cursor = next,
                            Err(ConsensusSendError::Stopped) if cancellation.is_cancelled() => {
                                return Ok(());
                            }
                            Err(error) => return Err(ConsensusLoopError::Delivery(error)),
                        }
                    }
                }
                HistoryChange::Reorg {
                    removed_block_tips,
                    new_blocks,
                } => {
                    if cancellation.is_cancelled() {
                        return Ok(());
                    }
                    match manager.bitcoin_reorg(removed_block_tips, new_blocks) {
                        Ok(()) => *cursor = history.next_cursor,
                        Err(ConsensusSendError::Stopped) if cancellation.is_cancelled() => {
                            return Ok(());
                        }
                        Err(error) => return Err(ConsensusLoopError::Delivery(error)),
                    }
                }
            }
            if cancellation.is_cancelled() {
                return Ok(());
            }
            if *cursor == target_tip {
                return Ok(());
            }
        }
    }
}
