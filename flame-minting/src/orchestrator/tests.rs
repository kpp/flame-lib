use std::{
    convert::Infallible,
    sync::atomic::{AtomicBool, Ordering},
};

use btc_integration::{ShutdownError, StartupError};
use corepc_client::{bitcoin::hashes::Hash, client_sync::Auth};
use flamevm::Predicate;

use super::*;

#[derive(Clone)]
struct Chain;

impl ChainAccess for Chain {
    type Error = Infallible;
}

struct Source;

impl CoreBlockSource for Source {
    type Error = Infallible;
}

struct Notifier;

impl CoreBlockNotifier for Notifier {
    type Error = Infallible;
}

struct UnexpectedFlameBlockArrival;

impl FlameBlockArrivalHandler for UnexpectedFlameBlockArrival {
    type Error = Infallible;

    async fn on_flame_block_arrival(&self, _: flamechain::BlockTip) -> Result<(), Self::Error> {
        panic!("Flame block arrival is not expected in startup tests")
    }
}

struct StartupProbe(Arc<IdentityManager>, Arc<AtomicBool>);

impl ports::ConsensusIndexer for StartupProbe {
    async fn startup(&self) -> Result<(), StartupError> {
        assert!(self.0.identity().is_some());
        assert!(self.1.load(Ordering::SeqCst));
        Err(StartupError::WorkerStopped)
    }

    async fn shutdown(&self) -> Result<(), ShutdownError> {
        Ok(())
    }
}

struct SenderProbe {
    identity: Arc<IdentityManager>,
    started: Arc<AtomicBool>,
    fail: bool,
}

impl ports::SenderLifecycle for SenderProbe {
    async fn startup(&self) -> Result<(), SenderStartupError> {
        assert!(self.identity.identity().is_some());
        if self.fail {
            return Err(SenderStartupError::Connection(
                btc_integration::BitcoinConnectionError::Rpc(
                    corepc_client::client_sync::Error::Returned("sender setup failed".into()),
                ),
            ));
        }
        self.started.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn initializes_identity_and_sender_before_starting_indexer() {
    check_startup(false).await;
}

#[tokio::test]
async fn sender_failure_prevents_indexer_and_minter_startup() {
    check_startup(true).await;
}

async fn check_startup(fail_sender: bool) {
    let config = IdentityConfig {
        flame_predicate: Predicate::opaque(Predicate::unspendable_key()),
        access_predicate: Predicate::opaque(Predicate::unspendable_key()),
        validator_pubkey: ed25519_dalek::SigningKey::from_bytes(&[0x22; 32]).verifying_key(),
    };
    let orchestrator = MintingOrchestrator::new(
        Notifier,
        Source,
        Chain,
        UnexpectedFlameBlockArrival,
        BitcoinConfig {
            node_rpc_url: "http://127.0.0.1:1".into(),
            auth: Auth::UserPass("test".into(), "test".into()),
        },
        BtcBlockTip {
            hash: corepc_client::bitcoin::BlockHash::all_zeros(),
            height: 0,
        },
        config.clone(),
        EndorsementPolicy::Auto,
    )
    .unwrap();
    assert!(orchestrator.identity_manager.identity().is_none());
    assert!(std::ptr::eq(
        orchestrator.identity_manager.as_ref(),
        orchestrator.btc_sender.identity_manager(),
    ));
    assert_eq!(
        orchestrator
            .btc_sender
            .identity_manager()
            .config()
            .validator_pubkey,
        config.validator_pubkey
    );
    let sender_started = Arc::new(AtomicBool::new(false));
    let orchestrator = MintingOrchestrator {
        btc_indexer: Arc::new(StartupProbe(
            orchestrator.identity_manager.clone(),
            sender_started.clone(),
        )),
        btc_sender: SenderProbe {
            identity: orchestrator.identity_manager.clone(),
            started: sender_started.clone(),
            fail: fail_sender,
        },
        identity_manager: orchestrator.identity_manager,
        consensus_manager: orchestrator.consensus_manager,
        minter_manager: orchestrator.minter_manager,
        chain_manager: orchestrator.chain_manager,
        core_block_notifier: orchestrator.core_block_notifier,
    };

    let result = orchestrator.startup().await;
    if fail_sender {
        assert!(matches!(result, Err(OrchestratorError::SenderStartup(_))));
    } else {
        assert!(matches!(
            result,
            Err(OrchestratorError::Consensus(
                consensus::ConsensusLoopError::IndexerStartup(StartupError::WorkerStopped)
            ))
        ));
    }
    assert_eq!(sender_started.load(Ordering::SeqCst), !fail_sender);
    assert!(orchestrator.identity_manager.identity().is_some());
    orchestrator.shutdown().await.unwrap();
}
