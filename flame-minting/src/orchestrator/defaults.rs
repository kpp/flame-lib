use std::{num::NonZeroU16, sync::Arc};

use btc_integration::BtcBlockTip;
use flame_chain_service::ChainAccess;
use flame_storage::{chain::InMemoryChainStorage, state::canonical::InMemoryCanonicalStorage};

use super::{consensus::ConsensusLoop, cursor_storage::InMemoryCursorStorage};
use crate::{
    FlameBlockArrivalHandler,
    block_processing::{
        BitcoinBlockProcessor, ConsensusManager, journal::InMemoryBitcoinBlockJournal,
    },
    consensus::{MintingEngineFactory, MintingProtocolParams, storage::InMemoryConsensusStorage},
    rewards::{RewardsEngine, storage::InMemoryRewardsStorage},
};

pub type DefaultConsensusLoop<H, F> = ConsensusLoop<
    ConsensusManager<
        BitcoinBlockProcessor<
            MintingEngineFactory<
                InMemoryConsensusStorage,
                InMemoryChainStorage,
                InMemoryCanonicalStorage,
            >,
            InMemoryConsensusStorage,
            H,
            InMemoryBitcoinBlockJournal,
            InMemoryCanonicalStorage,
            InMemoryRewardsStorage,
            InMemoryCursorStorage,
            InMemoryChainStorage,
        >,
        F,
    >,
>;

pub(super) fn consensus_loop<H, F>(
    chain: H,
    initial_cursor: BtcBlockTip,
    flame_handler: F,
) -> DefaultConsensusLoop<H, F>
where
    H: ChainAccess + Send + Sync + 'static,
    H::Error: Send + 'static,
    F: FlameBlockArrivalHandler,
{
    let protocol_params = Arc::new(MintingProtocolParams {
        acquisition_maturity: 1,
        default_acquisition_duration: NonZeroU16::new(100).unwrap(),
        min_acquisition_duration: NonZeroU16::MIN,
        max_endorsement_delay: 10,
    });
    let consensus_storage = InMemoryConsensusStorage::new();
    let chain_storage = Arc::new(InMemoryChainStorage::new());
    let canonical_storage = InMemoryCanonicalStorage::new();

    let bitcoin_processor = BitcoinBlockProcessor::new(
        MintingEngineFactory {
            protocol_params: protocol_params.clone(),
            consensus_storage: Arc::new(consensus_storage.clone()),
            chain_storage: chain_storage.clone(),
            canonical_storage: Arc::new(canonical_storage.clone()),
        },
        consensus_storage.clone(),
        chain,
        InMemoryBitcoinBlockJournal::new(initial_cursor),
        canonical_storage,
        InMemoryRewardsStorage::new(),
        InMemoryCursorStorage::new(initial_cursor),
        RewardsEngine {
            chain_storage,
            consensus_storage: Arc::new(consensus_storage),
            protocol_params,
        },
    );
    ConsensusLoop::new(ConsensusManager::new(bitcoin_processor, flame_handler))
}
