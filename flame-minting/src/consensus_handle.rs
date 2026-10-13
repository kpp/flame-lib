use std::sync::Arc;

use flamechain::BlockTip;

use crate::block_processing::FlameBlockArrivalHandler;

pub struct ConsensusHandle<C> {
    consensus: Arc<C>,
}

impl<C> ConsensusHandle<C> {
    pub fn new(consensus: Arc<C>) -> Self {
        Self { consensus }
    }
}

impl<C: FlameBlockArrivalHandler> ConsensusHandle<C> {
    pub async fn on_flame_block_arrival(&self, tip: BlockTip) -> Result<(), C::Error> {
        self.consensus.on_flame_block_arrival(tip).await
    }
}

impl<C> Clone for ConsensusHandle<C> {
    fn clone(&self) -> Self {
        Self {
            consensus: Arc::clone(&self.consensus),
        }
    }
}
