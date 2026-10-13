use std::{future::Future, sync::Arc};

use flamechain::{Block, BlockHeader, BlockTip, CoreBlockHeader, CoreBlockTip};

pub mod in_memory;

pub use in_memory::{InMemoryChainStorage, InMemoryChainStorageError};

pub trait ChainStorage {
    type Error;

    fn add_block(
        &self,
        _block: Arc<Block>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_block(
        &self,
        _tip: BlockTip,
    ) -> impl Future<Output = Result<Option<Arc<Block>>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_core_block_header(
        &self,
        _tip: CoreBlockTip,
    ) -> impl Future<Output = Result<Option<CoreBlockHeader>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_block_header_by_block_tip(
        &self,
        _tip: BlockTip,
    ) -> impl Future<Output = Result<Option<BlockHeader>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_block_header(
        &self,
        _tip: CoreBlockTip,
    ) -> impl Future<Output = Result<Option<BlockHeader>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_core_block_header_with_core_descendants(
        &self,
        _tip: CoreBlockTip,
    ) -> impl Future<Output = Result<Option<(BlockHeader, Vec<BlockHeader>)>, Self::Error>> + Send
    {
        async { unimplemented!() }
    }
}
