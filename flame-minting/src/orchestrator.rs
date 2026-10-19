use std::{fmt::Debug, sync::Arc};

use btc_integration::btc::rpc::Core31RpcApi;
use btc_integration::{
    BitcoinConfig, BitcoinFacade, BtcBlockTip, IdentityConfig, IdentityError, IdentityManager,
    ProtocolIndexerV31,
    identity::storage::{InMemorySecretStorage, MissingSecret},
};
use flame_chain_service::{ChainAccess, CoreBlockSource};
use flame_storage::{chain::InMemoryChainStorage, state::canonical::InMemoryCanonicalStorage};

use crate::ConsensusHandle;
use crate::block_processing::{
    BlockProcessor, ConsensusManagerError, FlameProcessor, journal::consensus::ConsensusJournal,
};
use crate::consensus::storage::InMemoryConsensusStorage;
use crate::core_block_notifier::CoreBlockNotifier;
use crate::minter::manager::{
    ShutdownError as MinterShutdownError, StartupError as MinterStartupError,
};
use crate::minter::{EndorsementPolicy, MinterManager, journal::in_memory::InMemoryMinterJournal};

pub use btc_integration::HistoryUpdate as BtcEvent;

pub mod consensus;
pub mod cursor_storage;
mod defaults;
pub mod ports;
pub mod sender;

use cursor_storage::in_memory::InMemoryCursorStorage;
pub use defaults::DefaultConsensusLoop;
use sender::{MintingSender, SenderStartupError};

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub enum OrchestratorError<C> {
    Identity(IdentityError<MissingSecret>),
    SenderStartup(SenderStartupError),
    Consensus(C),
    MinterStartup(MinterStartupError),
    MinterShutdown(MinterShutdownError),
}

type OrchestratorResult<E> = Result<(), OrchestratorError<consensus::ConsensusLoopError<E>>>;

pub struct MintingOrchestrator<S, I, C, M, H: ChainAccess, N: CoreBlockNotifier> {
    pub identity_manager: Arc<IdentityManager>,
    pub btc_sender: S,
    pub btc_indexer: I,
    pub consensus_manager: C,
    pub minter_manager: M,
    pub chain_manager: H,
    pub core_block_notifier: N,
}

impl<B, H, N>
    MintingOrchestrator<
        Arc<MintingSender>,
        Arc<ProtocolIndexerV31>,
        DefaultConsensusLoop<H>,
        MinterManager<B, MintingSender, InMemoryMinterJournal>,
        H,
        N,
    >
where
    B: CoreBlockSource + Send + Sync + 'static,
    B::Error: Debug + Send,
    H: ChainAccess + Clone + Send + Sync + 'static,
    H::Error: Send + 'static,
    N: CoreBlockNotifier,
{
    pub fn new(
        core_block_notifier: N,
        core_block_source: B,
        chain_manager: H,
        bitcoin_config: BitcoinConfig,
        initial_btc_cursor: BtcBlockTip,
        identity_config: IdentityConfig,
        endorsement_policy: EndorsementPolicy,
    ) -> Result<Self, corepc_client::client_sync::Error> {
        let rpc = Arc::new(Core31RpcApi::new(
            &bitcoin_config.node_rpc_url,
            bitcoin_config.auth.clone(),
        )?);
        let btc_indexer = Arc::new(ProtocolIndexerV31::new(Arc::new(BitcoinFacade::new(rpc))));
        let identity_manager = Arc::new(IdentityManager::new(
            Arc::new(InMemorySecretStorage::default()),
            identity_config,
        ));
        let btc_sender = Arc::new(MintingSender::new(bitcoin_config, identity_manager.clone()));
        let consensus_manager = defaults::consensus_loop(chain_manager.clone(), initial_btc_cursor);
        let minter_manager = MinterManager::new(
            Arc::new(core_block_source),
            btc_sender.clone(),
            Arc::new(InMemoryMinterJournal::new()),
            endorsement_policy,
        );

        Ok(Self {
            identity_manager,
            btc_sender,
            btc_indexer,
            consensus_manager,
            minter_manager,
            chain_manager,
            core_block_notifier,
        })
    }

    pub fn get_consensus_storage(&self) -> &InMemoryConsensusStorage {
        let processor = self.consensus_manager.manager().bitcoin_processor();
        &processor.engine.consensus_storage
    }

    pub fn get_chain_storage(&self) -> &InMemoryChainStorage {
        let processor = self.consensus_manager.manager().bitcoin_processor();
        &processor.engine.chain_storage
    }

    pub fn get_canonical_storage(&self) -> &InMemoryCanonicalStorage {
        let processor = self.consensus_manager.manager().bitcoin_processor();
        &processor.engine.canonical_storage
    }

    pub fn get_cursor_storage(&self) -> &InMemoryCursorStorage {
        let processor = self.consensus_manager.manager().bitcoin_processor();
        &processor.cursor_storage
    }
}

impl<S, I, B, F, J, M, H, N> MintingOrchestrator<S, I, consensus::ConsensusLoop<B, F, J>, M, H, N>
where
    B: BlockProcessor,
    F: FlameProcessor,
    J: ConsensusJournal,
    H: ChainAccess,
    N: CoreBlockNotifier,
{
    pub fn consensus_handle(&self) -> ConsensusHandle<B, F, J> {
        self.consensus_manager.consensus_handle()
    }
}

impl<S, I, B, F, J, M, H, N>
    MintingOrchestrator<S, std::sync::Arc<I>, consensus::ConsensusLoop<B, F, J>, M, H, N>
where
    S: ports::SenderLifecycle,
    I: ports::ConsensusIndexer,
    B: BlockProcessor,
    F: FlameProcessor,
    J: ConsensusJournal,
    M: ports::MinterLifecycle,
    H: ChainAccess,
    N: CoreBlockNotifier,
{
    pub async fn startup(
        &self,
    ) -> OrchestratorResult<ConsensusManagerError<B::Error, F::Error, J::Error>> {
        self.identity_manager
            .startup()
            .await
            .map_err(OrchestratorError::Identity)?;
        self.btc_sender
            .startup()
            .await
            .map_err(OrchestratorError::SenderStartup)?;
        self.consensus_manager
            .startup(self.btc_indexer.clone())
            .await
            .map_err(OrchestratorError::Consensus)?;
        if let Err(error) = self
            .minter_manager
            .startup(self.btc_indexer.subscribe())
            .await
        {
            self.consensus_manager
                .shutdown()
                .await
                .map_err(OrchestratorError::Consensus)?;
            return Err(OrchestratorError::MinterStartup(error));
        }
        Ok(())
    }

    pub async fn shutdown(
        &self,
    ) -> OrchestratorResult<ConsensusManagerError<B::Error, F::Error, J::Error>> {
        let (minter, consensus) = tokio::join!(
            self.minter_manager.shutdown(),
            self.consensus_manager.shutdown(),
        );
        minter
            .map_err(OrchestratorError::MinterShutdown)
            .and(consensus.map_err(OrchestratorError::Consensus))
    }
}
