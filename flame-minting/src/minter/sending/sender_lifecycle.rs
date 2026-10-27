use super::minting_sender::{MintingSender, SenderStartupError};
use std::{future::Future, sync::Arc};

pub trait SenderLifecycle {
    fn startup(&self) -> impl Future<Output = Result<(), SenderStartupError>> + Send {
        async { unimplemented!() }
    }
}

impl SenderLifecycle for MintingSender {
    async fn startup(&self) -> Result<(), SenderStartupError> {
        self.startup().await
    }
}

impl<S: SenderLifecycle + Send + Sync> SenderLifecycle for Arc<S> {
    async fn startup(&self) -> Result<(), SenderStartupError> {
        self.as_ref().startup().await
    }
}
