use std::sync::Arc;

use crate::{Block, Blockchain, ChainError};

pub trait FlameBlockValidator {
    fn validate_flame_block(
        &self,
        _parent: &Blockchain,
        _block: Arc<Block>,
    ) -> Result<(), ChainError> {
        unimplemented!()
    }
}
