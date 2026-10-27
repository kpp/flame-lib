use std::sync::Arc;

use btc_integration::{
    Acquisition, AcquisitionData, AuthenticatedMintingEndorsement, BtcBlockTip, IndexedBlock,
    MintingEndorsementValidator, UncheckedMintingEndorsement, protocol::minter_witness_script,
};
use corepc_client::bitcoin::{
    Amount, BlockHash as BitcoinBlockHash, Script, ScriptBuf, Transaction, TxIn, TxOut, Witness,
    absolute, hashes::Hash, transaction,
};
use flame_chain_service::{ChainAccess, InMemoryChain};
use flame_minting::{
    block_processing::{
        BitcoinBlockProcessor,
        journal::consensus::{ConsensusJournal, InMemoryBitcoinJournal, InMemoryConsensusJournal},
    },
    consensus::{
        ConsensusStorage, MintingEngineFactory, MintingProtocolParams, WeightedBlockHeader,
        storage::InMemoryConsensusStorage,
    },
    orchestrator::cursor_storage::{CursorStorage, InMemoryCursorStorage},
    rewards::{
        RewardsEngine,
        storage::{InMemoryRewardsStorage, RewardIndex},
    },
};
use flame_storage::{
    CanonicalStorage, ChainStorage, chain::InMemoryChainStorage,
    state::canonical::InMemoryCanonicalStorage,
};
use flamechain::{
    Block, BlockTip, Blockchain, ChainParams, CoreBlockHeader, CoreBlockTip, StateCommitment,
};
use flamevm::Predicate;

#[path = "bitcoin_reorg/bitcoin_block.rs"]
mod bitcoin_block;
#[path = "bitcoin_reorg/fixture.rs"]
mod fixture;
#[path = "bitcoin_reorg/flame_branch.rs"]
mod flame_branch;
#[path = "bitcoin_reorg/test_node.rs"]
mod test_node;

pub use bitcoin_block::bitcoin_block;
use bitcoin_block::{acquisition, btc_tip};
pub use fixture::Fixture;
use flame_branch::{flame_branch, register_block};
use test_node::TestNode;
