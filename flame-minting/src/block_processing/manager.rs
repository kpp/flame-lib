use btc_integration::{BtcBlockTip, IndexedBlock};
use flamechain::BlockTip;
use tokio::sync::Mutex;

use super::{BlockProcessor, FlameBlockArrivalHandler};

pub struct ConsensusManager<B, F> {
    bitcoin_processor: B,
    flame_handler: F,
    operation: Mutex<()>,
}

impl<B, F> ConsensusManager<B, F> {
    pub fn new(bitcoin_processor: B, flame_handler: F) -> Self {
        Self {
            bitcoin_processor,
            flame_handler,
            operation: Mutex::new(()),
        }
    }

    pub(crate) fn bitcoin_processor(&self) -> &B {
        &self.bitcoin_processor
    }
}

impl<B: BlockProcessor, F: FlameBlockArrivalHandler> BlockProcessor for ConsensusManager<B, F> {
    type Error = B::Error;

    async fn apply(&self, block: IndexedBlock) -> Result<BtcBlockTip, Self::Error> {
        let _operation = self.operation.lock().await;
        self.bitcoin_processor.apply(block).await
    }

    async fn resume(&self) -> Result<BtcBlockTip, Self::Error> {
        let _operation = self.operation.lock().await;
        self.bitcoin_processor.resume().await
    }

    async fn rollback(&self, tip: BtcBlockTip) -> Result<BtcBlockTip, Self::Error> {
        let _operation = self.operation.lock().await;
        self.bitcoin_processor.rollback(tip).await
    }
}

impl<B: BlockProcessor, F: FlameBlockArrivalHandler> FlameBlockArrivalHandler
    for ConsensusManager<B, F>
{
    type Error = F::Error;

    async fn on_flame_block_arrival(&self, tip: BlockTip) -> Result<(), Self::Error> {
        let _operation = self.operation.lock().await;
        self.flame_handler.on_flame_block_arrival(tip).await
    }
}
