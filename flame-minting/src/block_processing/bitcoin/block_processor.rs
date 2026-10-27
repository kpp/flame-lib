use btc_integration::{BtcBlockTip, IndexedBlock};
use std::future::Future;

pub trait BlockProcessor: Send + Sync + 'static {
    type Error: Send + 'static;

    fn apply(
        &self,
        block: IndexedBlock,
    ) -> impl Future<Output = Result<BtcBlockTip, Self::Error>> + Send;
    fn resume(&self) -> impl Future<Output = Result<BtcBlockTip, Self::Error>> + Send;
    fn rollback(
        &self,
        tip: BtcBlockTip,
    ) -> impl Future<Output = Result<BtcBlockTip, Self::Error>> + Send;
}
