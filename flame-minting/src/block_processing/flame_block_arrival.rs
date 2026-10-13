use std::future::Future;

use flamechain::BlockTip;

pub trait FlameBlockArrivalHandler: Send + Sync + 'static {
    type Error: Send + 'static;

    fn on_flame_block_arrival(
        &self,
        tip: BlockTip,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
