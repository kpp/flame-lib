use std::sync::Arc;

use btc_integration::{
    Acquisition, AcquisitionData, AuthenticatedMintingEndorsement, BtcBlockTip, IndexedBlock,
    MintingEndorsementValidator, UncheckedMintingEndorsement, protocol::minter_witness_script,
};
use corepc_client::bitcoin::{
    Amount, BlockHash as BitcoinBlockHash, Script, ScriptBuf, Transaction, TxIn, TxOut, Witness,
    absolute, hashes::Hash, transaction,
};
use ed25519_dalek::SigningKey;
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

type Processor = BitcoinBlockProcessor<
    MintingEngineFactory<InMemoryConsensusStorage, InMemoryChainStorage, InMemoryCanonicalStorage>,
    InMemoryConsensusStorage,
    InMemoryChain,
    InMemoryBitcoinJournal,
    InMemoryCanonicalStorage,
    InMemoryRewardsStorage,
    InMemoryCursorStorage,
    InMemoryChainStorage,
>;

pub struct Fixture {
    initial: Blockchain,
    root: Arc<Block>,
    pub old_flame: Vec<Arc<Block>>,
    pub new_flame: Vec<Arc<Block>>,
}

impl Fixture {
    pub fn new(branch_length: u32) -> Self {
        let mut initial = Blockchain::new(ChainParams::default()).unwrap();
        let root = Arc::new(initial.build_block([0; 32], vec![]).unwrap());
        initial.connect(&root).unwrap();
        Self {
            old_flame: flame_branch(initial.clone(), 1, branch_length),
            new_flame: flame_branch(initial.clone(), 2, branch_length),
            initial,
            root,
        }
    }

    pub async fn node(&self) -> TestNode {
        let consensus = InMemoryConsensusStorage::new();
        let canonical = InMemoryCanonicalStorage::new();
        let rewards = InMemoryRewardsStorage::new();
        let cursor = InMemoryCursorStorage::new(btc_tip(0, 0));
        let mut chain = InMemoryChain::default();
        let blocks = InMemoryChainStorage::new();
        let params = Arc::new(MintingProtocolParams {
            acquisition_maturity: 1,
            default_acquisition_duration: 100.try_into().unwrap(),
            min_acquisition_duration: 1.try_into().unwrap(),
            max_endorsement_delay: 10,
        });
        let node = TestNode {
            processor: BitcoinBlockProcessor::new(
                MintingEngineFactory {
                    protocol_params: params.clone(),
                    consensus_storage: Arc::new(consensus.clone()),
                    chain_storage: Arc::new(blocks.clone()),
                    canonical_storage: Arc::new(canonical.clone()),
                },
                consensus.clone(),
                chain.clone(),
                InMemoryConsensusJournal::new(btc_tip(0, 0)).bitcoin_journal(),
                canonical.clone(),
                rewards.clone(),
                cursor.clone(),
                RewardsEngine {
                    protocol_params: params,
                    consensus_storage: Arc::new(consensus.clone()),
                    chain_storage: Arc::new(blocks.clone()),
                },
            ),
            canonical,
            rewards,
            cursor,
        };
        node.canonical.commit_state(&self.initial).await.unwrap();
        register_block(&self.root, &blocks, &mut chain, &consensus).await;
        node.apply(&IndexedBlock {
            btc_block_tip: btc_tip(1, 0),
            acquisitions: vec![acquisition(1, 1_000_000), acquisition(2, 100_000_000)],
            endorsements: vec![],
        })
        .await;
        for block in self.old_flame.iter().chain(&self.new_flame) {
            register_block(block, &blocks, &mut chain, &consensus).await;
        }
        node
    }
}

pub struct TestNode {
    processor: Processor,
    canonical: InMemoryCanonicalStorage,
    rewards: InMemoryRewardsStorage,
    cursor: InMemoryCursorStorage,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub btc_tip: BtcBlockTip,
    pub flame_tip: BlockTip,
    state: StateCommitment,
}

impl TestNode {
    pub async fn apply(&self, block: &IndexedBlock) {
        self.processor.apply(block.clone()).await.unwrap();
    }

    pub async fn rollback(&self, block: &IndexedBlock) {
        self.processor.rollback(block.btc_block_tip).await.unwrap();
    }

    pub async fn snapshot(&self) -> Snapshot {
        let (_, state) = self.canonical.get_state().await.unwrap().unwrap();
        Snapshot {
            btc_tip: self.cursor.get_cursor().await.unwrap(),
            flame_tip: BlockTip {
                hash: state.tip(),
                height: state.height().into(),
            },
            state: state.state_commitment(),
        }
    }

    pub async fn rewards(&self, height: u32) -> Vec<(RewardIndex, u64)> {
        self.rewards
            .get_pending_rewards(height.into())
            .await
            .unwrap()
            .iter()
            .map(|reward| (RewardIndex::from(reward), reward.amount))
            .collect()
    }
}

pub fn bitcoin_block(height: u64, branch: u8, minter: u8, endorsed: &[Arc<Block>]) -> IndexedBlock {
    IndexedBlock {
        btc_block_tip: btc_tip(height, branch),
        acquisitions: vec![],
        endorsements: endorsed
            .iter()
            .map(|block| endorsement(minter, core_tip(block)))
            .collect(),
    }
}

async fn register_block(
    block: &Arc<Block>,
    blocks: &InMemoryChainStorage,
    chain: &mut InMemoryChain,
    consensus: &InMemoryConsensusStorage,
) {
    blocks.add_block(block.clone()).await.unwrap();
    chain.import_block(block.clone()).await.unwrap();
    consensus
        .store_cumulative_weight(
            block.header.block_tip(),
            &WeightedBlockHeader {
                header: block.header.clone(),
                parent_weight: 0,
                effective_power: 0,
            },
        )
        .await
        .unwrap();
}

fn flame_branch(mut state: Blockchain, branch: u8, length: u32) -> Vec<Arc<Block>> {
    (1..=length)
        .map(|height| {
            let mut block = state.build_block([branch; 32], vec![]).unwrap();
            block.header.core_block = Some(CoreBlockHeader {
                height: height.into(),
                target_btc_height: 2,
            });
            state.connect(&block).unwrap();
            Arc::new(block)
        })
        .collect()
}

fn core_tip(block: &Block) -> CoreBlockTip {
    CoreBlockTip {
        hash: block.header.id(),
        height: block.header.core_block.as_ref().unwrap().height,
    }
}

fn btc_tip(height: u64, branch: u8) -> BtcBlockTip {
    let mut bytes = [branch; 32];
    bytes[..8].copy_from_slice(&height.to_le_bytes());
    BtcBlockTip {
        hash: BitcoinBlockHash::from_byte_array(bytes),
        height,
    }
}

fn witness_script(minter: u8) -> ScriptBuf {
    minter_witness_script::build_with_authorization(
        &Predicate::opaque(Predicate::unspendable_key()),
        Script::from_bytes(&[minter]),
    )
}

fn acquisition(minter: u8, amount: u64) -> Acquisition {
    let point = Predicate::unspendable_key().decompress().unwrap();
    let access =
        Predicate::opaque((point * flamevm::Scalar::from(u64::from(minter)).to_dalek()).compress());
    let data = AcquisitionData::new(
        witness_script(minter).wscript_hash().to_byte_array().into(),
        access,
        SigningKey::from_bytes(&[minter; 32]).verifying_key(),
    );
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![],
        output: vec![TxOut {
            value: Amount::from_sat(amount),
            script_pubkey: data.to_script(),
        }],
    };
    Acquisition::from_tx(&tx).pop().unwrap()
}

fn endorsement(minter: u8, tip: CoreBlockTip) -> AuthenticatedMintingEndorsement {
    let script = witness_script(minter);
    let mut payload = b"FLMB".to_vec();
    payload.push(1);
    payload.extend_from_slice(&tip.height.as_u32().to_le_bytes());
    payload.extend_from_slice(tip.hash.as_bytes());
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            witness: Witness::from_slice(&[script.as_bytes()]),
            ..TxIn::default()
        }],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::new_op_return(
                <&corepc_client::bitcoin::script::PushBytes>::try_from(payload.as_slice()).unwrap(),
            ),
        }],
    };
    MintingEndorsementValidator::validate(
        UncheckedMintingEndorsement::from_tx(&tx).pop().unwrap(),
        &TxOut {
            value: Amount::from_sat(1),
            script_pubkey: script.to_p2wsh(),
        },
    )
    .unwrap()
}
