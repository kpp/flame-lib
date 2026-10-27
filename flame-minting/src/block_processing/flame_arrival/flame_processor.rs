use flamechain::BlockTip;
use std::future::Future;

pub trait FlameProcessor: Send + Sync + 'static {
    type Error: Send + 'static;

    fn process(&self, tip: BlockTip) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn resume(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
