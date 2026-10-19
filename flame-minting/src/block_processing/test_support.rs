use std::{future::Future, time::Duration};

pub(crate) async fn wait_until<F: Future<Output = bool>>(mut condition: impl FnMut() -> F) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("consensus operation did not complete");
}

use super::{FlameBlockArrivalHandler, FlameProcessor};
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
