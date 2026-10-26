use crate::block_processing::{FlameBlockArrivalHandler, FlameProcessor};
use flamechain::BlockTip;

pub(crate) struct ArrivalHandlerProcessor<F> {
    handler: F,
}

impl<F> ArrivalHandlerProcessor<F> {
    pub(crate) fn new(handler: F) -> Self {
        Self { handler }
    }
}

impl<F: FlameBlockArrivalHandler> FlameProcessor for ArrivalHandlerProcessor<F> {
    type Error = F::Error;

    async fn process(&self, tip: BlockTip) -> Result<(), Self::Error> {
        self.handler.on_flame_block_arrival(tip).await
    }

    async fn resume(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
