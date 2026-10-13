mod consensus;
mod flame;
mod flame_block_arrival;
pub mod journal;
mod manager;
mod processor;
mod rewards;

pub use manager::ConsensusManager;
pub use processor::{BitcoinBlockProcessor, BlockProcessingError, BlockProcessor};

pub use flame::{ChainPathApplierError, PrepareFlameError};
pub use flame_block_arrival::FlameBlockArrivalHandler;
pub use rewards::PrepareRewardsError;
