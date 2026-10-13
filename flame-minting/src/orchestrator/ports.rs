use std::{fmt::Debug, future::Future, num::NonZeroUsize, sync::Arc};

use btc_integration::{
    BtcBlockTip, HistoryError, HistoryUpdate, ProtocolIndexer, ShutdownError, StartupError,
    btc::rpc::RpcApi,
};
use flame_chain_service::CoreBlockSource;
use tokio::sync::watch;

use crate::minter::{
    MinterJournal, MinterManager,
    manager::{ShutdownError as MinterShutdownError, StartupError as MinterStartupError},
    ports::EndorsementSender,
};

use super::sender::{MintingSender, SenderStartupError};

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

pub trait MinterLifecycle {
    fn startup(
        &self,
        _subscription: watch::Receiver<Option<BtcBlockTip>>,
    ) -> impl Future<Output = Result<(), MinterStartupError>> + Send {
        async { unimplemented!() }
    }

    fn shutdown(&self) -> impl Future<Output = Result<(), MinterShutdownError>> + Send {
        async { unimplemented!() }
    }
}

impl<C, S, J> MinterLifecycle for MinterManager<C, S, J>
where
    C: CoreBlockSource + Send + Sync + 'static,
    C::Error: Debug + Send,
    S: EndorsementSender,
    S::Error: Debug,
    J: MinterJournal<TransactionId = S::TransactionId> + Send + Sync + 'static,
    J::Error: Debug + Send,
{
    async fn startup(
        &self,
        subscription: watch::Receiver<Option<BtcBlockTip>>,
    ) -> Result<(), MinterStartupError> {
        self.startup(subscription).await
    }

    async fn shutdown(&self) -> Result<(), MinterShutdownError> {
        self.shutdown().await
    }
}

pub trait ConsensusIndexer: Send + Sync + 'static {
    fn startup(&self) -> impl Future<Output = Result<(), StartupError>> + Send {
        async { unimplemented!() }
    }

    fn shutdown(&self) -> impl Future<Output = Result<(), ShutdownError>> + Send {
        async { unimplemented!() }
    }

    fn subscribe(&self) -> watch::Receiver<Option<BtcBlockTip>> {
        unimplemented!()
    }

    fn get_history(
        &self,
        _cursor: BtcBlockTip,
        _limit: NonZeroUsize,
    ) -> impl Future<Output = Result<HistoryUpdate, HistoryError>> + Send {
        async { unimplemented!() }
    }
}

impl<R: RpcApi + 'static> ConsensusIndexer for ProtocolIndexer<R> {
    async fn startup(&self) -> Result<(), StartupError> {
        self.startup().await
    }

    async fn shutdown(&self) -> Result<(), ShutdownError> {
        self.shutdown().await
    }

    fn subscribe(&self) -> watch::Receiver<Option<BtcBlockTip>> {
        self.subscribe()
    }

    async fn get_history(
        &self,
        cursor: BtcBlockTip,
        limit: NonZeroUsize,
    ) -> Result<HistoryUpdate, HistoryError> {
        self.get_history(cursor, limit).await
    }
}
