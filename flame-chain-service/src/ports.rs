use std::{future::Future, sync::Arc};

use flamechain::{Block, BlockTip};

use crate::chain::{ChainPath, ImportOutcome};

pub trait ChainAccess {
    type Error;

    fn get_block(
        &self,
        _tip: BlockTip,
    ) -> impl Future<Output = Result<Option<Arc<Block>>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_chain_path(
        &self,
        _from: BlockTip,
        _to: BlockTip,
    ) -> impl Future<Output = Result<ChainPath, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn import_block(
        &mut self,
        _block: Arc<Block>,
    ) -> impl Future<Output = Result<ImportOutcome, Self::Error>> + Send {
        async { unimplemented!() }
    }
}

pub trait CoreBlockSource {
    type Error;

    fn get_core_block(
        &self,
        _target_height: u64,
    ) -> impl Future<Output = Result<Option<Arc<Block>>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn wait_core_block(
        &self,
        _height: u64,
    ) -> impl Future<Output = Result<Option<Arc<Block>>, Self::Error>> + Send {
        async { unimplemented!() }
    }
}
