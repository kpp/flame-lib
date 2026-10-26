use std::{future::Future, time::Duration};

mod arrival_handler_processor;
pub(crate) use arrival_handler_processor::ArrivalHandlerProcessor;

pub(crate) async fn wait_until<F: Future<Output = bool>>(mut condition: impl FnMut() -> F) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("consensus operation did not complete");
}
