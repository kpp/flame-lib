use std::sync::Arc;

use flamechain::BlockTip;

use crate::block_processing::{
    BlockProcessor, ConsensusManager, ConsensusSendError, FlameProcessor,
    journal::consensus::ConsensusJournal,
};

pub struct ConsensusHandle<B: BlockProcessor, F: FlameProcessor, J: ConsensusJournal> {
    manager: Arc<ConsensusManager<B, F, J>>,
}

impl<B: BlockProcessor, F: FlameProcessor, J: ConsensusJournal> ConsensusHandle<B, F, J> {
    pub(crate) fn new(manager: Arc<ConsensusManager<B, F, J>>) -> Self {
        Self { manager }
    }

    pub async fn on_flame_block_arrival(&self, tip: BlockTip) -> Result<(), ConsensusSendError> {
        self.manager.new_flame_block(tip)
    }
}

impl<B: BlockProcessor, F: FlameProcessor, J: ConsensusJournal> Clone for ConsensusHandle<B, F, J> {
    fn clone(&self) -> Self {
        Self {
            manager: self.manager.clone(),
        }
    }
}
