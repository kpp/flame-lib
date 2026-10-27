use block_attacher::{BlockAttacher, BlockAttacherError};
use block_detacher::{BlockDetacher, BlockDetacherError};

pub mod block_attacher;
pub mod block_detacher;
pub mod chain_path_applier;
pub mod flame_operations;
pub mod flame_transition;

pub use chain_path_applier::ChainPathApplierError;
pub(in crate::block_processing) use flame_operations::FlameOperations;
pub use flame_operations::PrepareFlameError;
pub use flame_transition::FlameTransition;
