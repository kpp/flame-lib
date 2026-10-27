use btc_integration::BtcBlockTip;
use std::future::Future;

pub use super::in_memory_cursor_storage as in_memory;
pub use super::in_memory_cursor_storage::{InMemoryCursorStorage, InMemoryCursorStorageError};

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
