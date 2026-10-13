use std::future::Future;

use btc_integration::BtcBlockTip;

pub mod in_memory;

pub use in_memory::{InMemoryCursorStorage, InMemoryCursorStorageError};

pub trait CursorStorage: Send + Sync + 'static {
    type Error: Send + 'static;

    fn get_cursor(&self) -> impl Future<Output = Result<BtcBlockTip, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn store_cursor(
        &self,
        _cursor: BtcBlockTip,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }
}
