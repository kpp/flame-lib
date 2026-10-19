mod consensus;
mod flame;
mod flame_block_arrival;
mod flame_block_processor;
mod flame_processor;
pub mod journal;
mod manager;
mod processor;
mod rewards;

#[cfg(test)]
pub(crate) mod test_support;

pub use manager::{
    CONSENSUS_QUEUE_CAPACITY, ConsensusManager, ConsensusManagerError, ConsensusSendError,
    ConsensusStatus,
};
pub use processor::{BitcoinBlockProcessor, BlockProcessingError, BlockProcessor};

pub use flame::{ChainPathApplierError, PrepareFlameError};
pub use flame_block_arrival::FlameBlockArrivalHandler;
pub use flame_block_processor::{FlameBlockProcessingError, FlameBlockProcessor};
pub use flame_processor::FlameProcessor;
pub use rewards::PrepareRewardsError;
