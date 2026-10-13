use std::{num::NonZeroUsize, sync::Arc};

use btc_integration::{BtcBlockTip, HistoryChange, HistoryError, ShutdownError, StartupError};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::ports::ConsensusIndexer;
use crate::block_processing::BlockProcessor;
use crate::{ConsensusHandle, FlameBlockArrivalHandler};

#[cfg(test)]
mod tests;

type LoopResult<E> = Result<(), ConsensusLoopError<E>>;

#[derive(Debug)]
pub enum ConsensusLoopError<E> {
    AlreadyRunning,
    IndexerStartup(StartupError),
    IndexerShutdown(ShutdownError),
    History(HistoryError),
    SubscriptionClosed,
    Process(E),
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

pub struct ConsensusLoop<P: BlockProcessor> {
    processor: Arc<P>,
    worker: Mutex<Option<Worker<P::Error>>>,
}

impl<P: BlockProcessor> ConsensusLoop<P> {
    pub fn new(processor: P) -> Self {
        Self {
            processor: Arc::new(processor),
            worker: Mutex::new(None),
        }
    }

    pub(super) fn processor(&self) -> &P {
        &self.processor
    }

    pub fn consensus_handle(&self) -> ConsensusHandle<P>
    where
        P: FlameBlockArrivalHandler,
    {
        ConsensusHandle::new(Arc::clone(&self.processor))
    }

    pub async fn startup<I: ConsensusIndexer>(&self, indexer: Arc<I>) -> LoopResult<P::Error> {
        let mut worker = self.worker.lock().await;
        if worker.is_some() {
            return Err(ConsensusLoopError::AlreadyRunning);
        }
        let cursor = self
            .processor
            .resume()
            .await
            .map_err(ConsensusLoopError::Process)?;
        indexer
            .startup()
            .await
            .map_err(ConsensusLoopError::IndexerStartup)?;
        let subscription = indexer.subscribe();
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();
        let processor = self.processor.clone();
        let task = tokio::spawn(async move {
            let result = Self::run(
                processor.as_ref(),
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
            result.and(shutdown)
        });
        *worker = Some(Worker { cancellation, task });
        Ok(())
    }

    pub async fn shutdown(&self) -> LoopResult<P::Error> {
        let mut slot = self.worker.lock().await;
        let Some(worker) = slot.as_mut() else {
            return Ok(());
        };
        worker.cancellation.cancel();
        let result = (&mut worker.task)
            .await
            .map_err(ConsensusLoopError::WorkerJoin);
        *slot = None;
        result?
    }

    async fn run<I: ConsensusIndexer>(
        processor: &P,
        indexer: &I,
        mut subscription: watch::Receiver<Option<BtcBlockTip>>,
        mut cursor: BtcBlockTip,
        cancellation: CancellationToken,
    ) -> LoopResult<P::Error> {
        loop {
            let tip = *subscription.borrow_and_update();
            if tip.is_some_and(|tip| tip != cursor) {
                Self::catch_up(processor, indexer, &mut cursor, &cancellation).await?;
            }
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Ok(()),
                result = subscription.changed() => { result.map_err(|_| ConsensusLoopError::SubscriptionClosed)?; }
            }
        }
    }

    async fn catch_up<I: ConsensusIndexer>(
        processor: &P,
        indexer: &I,
        cursor: &mut BtcBlockTip,
        cancellation: &CancellationToken,
    ) -> LoopResult<P::Error> {
        loop {
            let history = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Ok(()),
                result = indexer.get_history(*cursor, NonZeroUsize::new(1000).unwrap()) => {
                    result.map_err(ConsensusLoopError::History)?
                }
            };
            let new_blocks = match history.change {
                HistoryChange::Extension { new_blocks } => new_blocks,
                HistoryChange::Reorg {
                    removed_block_tips,
                    new_blocks,
                } => {
                    for tip in removed_block_tips {
                        if cancellation.is_cancelled() {
                            return Ok(());
                        }
                        *cursor = processor
                            .rollback(tip)
                            .await
                            .map_err(ConsensusLoopError::Process)?;
                    }
                    new_blocks
                }
            };
            for block in new_blocks {
                if cancellation.is_cancelled() {
                    return Ok(());
                }
                *cursor = processor
                    .apply(block)
                    .await
                    .map_err(ConsensusLoopError::Process)?;
            }
            if *cursor == history.target_tip {
                return Ok(());
            }
        }
    }
}
