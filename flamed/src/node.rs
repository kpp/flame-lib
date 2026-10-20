//! One node: a chain, a mempool, and the three things the chain forgets.
//!
//! A `Blockchain` keeps roots and an accumulator. It does not keep a proof
//! for anyone, it does not remember which transaction created which
//! contract, and it never knew which predicate locked it. A wallet needs all
//! three, so this node keeps them: `UtxoSet` for the proofs, `TxIndex` and
//! `OutputIndex` for the history. All three are rebuilt by replaying
//! `blocks.bin`, which is the only durable state there is.
//!
//! Times come from outside. A block's time is its core block's, which the
//! core indexer reports and remembers; a transaction's admission time is
//! this node's clock, kept in memory only.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::{Arc, Mutex};

use flamechain::utreexo::{Proof, UtreexoError};
use flamechain::{
    utreexo_hasher, Block, BlockHash, BlockTx, Blockchain, ChainError, ChainParams, ContractLeaf,
    ExecutionKind, Mempool, MempoolError, MempoolPolicy, RebaseReport,
};
use flamed_rpc::{
    ActorCode, ActorId, ActorQueryResult, ActorResult, ActorStateEnvelope, BlockHeader, BlockId,
    BlockResult, BlockSummary, BlockTxEnvelope, BlocksResult, ContractEnvelope, ContractId,
    ExecutionData, NoteEnvelope, PendingTransaction, PendingTransactionsResult, PredicatePoint,
    ScanEntry, SpentAt, StateCommitment, TransactionResult, TransactionSummary, TransactionsResult,
    TxId, ValueEnvelope, MAX_PAGE_SIZE, MAX_QUERY_BYTES, MAX_QUERY_GAS,
};
use flamevm::{
    ActorID, Cell, CellDecode, CellEncode, CellEnvelope, CellError, CellID, CellReader, ContractID,
    TxEntry, TxID, VMError, Value,
};

use crate::cells::{contract_bytes, log_bytes};
use crate::config::{GenesisFile, NodeConfig};
use crate::genesis::{self, GenesisError};
use crate::index::{OutputIndex, OutputRecord, SpendRecord, TxIndex, TxRecord};
use crate::indexer::{unix_now, CoreError, CoreIndexer, MockIndexer};
use crate::inspect;
use crate::store::{BlockStore, StoreError};
use crate::utxos::UtxoSet;

/// The txid the indexes record for a genesis allocation, which no
/// transaction created.
pub const GENESIS_TXID: TxID = TxID([0; 32]);

/// One node, shared by the RPC handlers and the minter.
pub type SharedNode = Arc<Mutex<Node>>;

// A compact shared Cell graph can expand into many Dict values during decoding.
struct QueryDecode<'a> {
    envelope: &'a mut CellEnvelope,
    gas: u64,
}

impl CellReader for QueryDecode<'_> {
    fn read(&mut self, id: CellID, resident: Option<&Arc<Cell>>) -> Result<Arc<Cell>, CellError> {
        let cell = self.envelope.read(id, resident)?;
        self.gas = self
            .gas
            .checked_sub(1 + cell.record_size() as u64 + cell.refs().len() as u64)
            .ok_or(CellError::ResourceExhausted)?;
        Ok(cell)
    }
}

pub use crate::indexer::core_block_hash;

/// An archival node.
pub struct Node {
    params: ChainParams,
    chain: Blockchain,
    mempool: Mempool,
    utxos: UtxoSet,
    txindex: TxIndex,
    outputs: OutputIndex,
    store: BlockStore,
    // Read again only by the minter; a build without one asks it only
    // while opening.
    #[cfg_attr(not(any(test, feature = "devnet")), allow(dead_code))]
    indexer: Box<dyn CoreIndexer>,
    /// When each pending transaction was first admitted, by txid. A
    /// transaction that leaves the pool by confirming takes its time into
    /// `TxIndex`; one that leaves any other way takes it nowhere.
    received: BTreeMap<[u8; 32], u64>,
}

// Every call into the node runs on the blocking pool behind a
// `std::sync::Mutex`, which needs this. It holds today — nothing upstream
// uses `Rc` — and this is the tripwire if that changes.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<Node>();
};

/// The chain's current tip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TipInfo {
    /// The tip block's hash.
    pub hash: BlockHash,
    /// Its height.
    pub height: u64,
    /// The contract accumulator root at that tip.
    pub contract_root: [u8; 32],
}

/// What the node knows about one contract.
#[derive(Clone, Debug)]
pub enum ProofStatus {
    /// Unspent, with a proof valid at the tip.
    Unspent(Proof),
    /// Spent, here.
    Spent {
        /// The height that spent it.
        height: u64,
        /// The transaction that spent it.
        txid: TxID,
    },
    /// Never seen.
    Unknown,
}

/// Where a transaction is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxStatus {
    /// Neither in the mempool nor in the chain.
    Unknown,
    /// Waiting in this node's mempool.
    Mempool,
    /// In a block.
    Confirmed {
        /// The height of that block.
        height: u64,
        /// Its hash.
        block: BlockHash,
    },
}

impl Node {
    /// Opens a node over the mock core indexer in `cfg.indexer_dir()`.
    pub fn open(genesis: &GenesisFile, cfg: &NodeConfig) -> Result<Node, NodeError> {
        let indexer = MockIndexer::open(&cfg.indexer_dir())?;
        Self::open_with(genesis, cfg, Box::new(indexer))
    }

    /// Opens a node: rebuilds the genesis, replays the archive with each
    /// block's time from `indexer`, and starts with an empty mempool.
    pub fn open_with(
        genesis: &GenesisFile,
        cfg: &NodeConfig,
        indexer: Box<dyn CoreIndexer>,
    ) -> Result<Node, NodeError> {
        fs::create_dir_all(&cfg.data_dir)?;
        let params = genesis.chain.params();

        // Every allocation is checked against its own bytes before it is
        // allowed to be money.
        let allocations = genesis::contracts(genesis)?;
        let ids: Vec<ContractID> = allocations.iter().map(|(id, _)| *id).collect();

        let (mut chain, catchup) = build_chain(params, &ids)?;
        let derived = chain.tip().into_bytes();
        if derived != genesis.genesis_hash.0 {
            return Err(NodeError::GenesisHashMismatch {
                recorded: hex::encode(genesis.genesis_hash.0),
                derived: hex::encode(derived),
            });
        }

        let mut utxos = UtxoSet::new();
        let mut outputs = OutputIndex::new();
        let mut txindex = TxIndex::new();

        txindex.insert_block(block_summary(chain.header(), None, 0, 0, 0, 0));

        for ((id, contract), record) in allocations.iter().zip(&genesis.contracts) {
            utxos.insert_transient(*id);
            outputs.insert(
                *id,
                OutputRecord {
                    height: 0,
                    txid: GENESIS_TXID,
                    predicate: contract.predicate.to_point().to_bytes(),
                    contract: record.bytes.0.clone(),
                    note: None,
                },
            );
        }
        if let Some(catchup) = catchup {
            utxos.apply_catchup(&catchup)?;
        }

        let mut store = BlockStore::open(&cfg.blocks_path())?;
        let mut untimed = 0usize;
        // An archived block was validated when it was appended, so one that
        // will not decode or connect now means the archive is wrong, not
        // that this block is skippable. A block the indexer has no time for
        // is still the same block; it is shown without one.
        for block in store.replay(params)? {
            let height = block.header.height;
            let time = indexer.time_of(&block.header.core_block_hash)?;
            untimed += usize::from(time.is_none());
            connect_block(
                &mut chain,
                &mut utxos,
                &mut txindex,
                &mut outputs,
                None,
                &block,
                time,
            )
            .map_err(|source| NodeError::Replay {
                height,
                source: Box::new(source),
            })?;
        }

        if untimed != 0 {
            eprintln!("flamed: the core indexer has no time for {untimed} archived block(s)");
        }

        let mempool = Mempool::new(&chain, mempool_policy(params, cfg.minimum_fee));

        Ok(Node {
            params,
            chain,
            mempool,
            utxos,
            txindex,
            outputs,
            store,
            indexer,
            received: BTreeMap::new(),
        })
    }

    /// Offers a transaction to the mempool.
    ///
    /// The admission time is kept from the first admission: a transaction
    /// the minter takes out and puts back is the same pending transaction.
    pub fn submit(&mut self, block_tx_bytes: &[u8]) -> Result<TxID, NodeError> {
        let block_tx =
            BlockTx::from_bytes_bounded(block_tx_bytes, self.params.version, self.params.limits)
                .map_err(NodeError::Decode)?;
        let txid = self.mempool.admit(block_tx)?.txid();
        self.received.entry(txid.0).or_insert_with(unix_now);
        Ok(txid)
    }

    /// The current tip.
    pub fn tip(&self) -> TipInfo {
        TipInfo {
            hash: self.chain.tip(),
            height: self.chain.height(),
            contract_root: self.chain.state_commitment().contracts.0,
        }
    }

    /// What the node knows about one contract.
    pub fn proof(&self, id: &ContractID) -> ProofStatus {
        if let Some(proof) = self.utxos.get(id) {
            return ProofStatus::Unspent(proof.clone());
        }
        if let Some(spend) = self.outputs.spend_of(id) {
            return ProofStatus::Spent {
                height: spend.height,
                txid: spend.txid,
            };
        }
        ProofStatus::Unknown
    }

    /// The same question for several contracts.
    pub fn proofs(&self, ids: &[ContractID]) -> Vec<(ContractID, ProofStatus)> {
        ids.iter().map(|id| (*id, self.proof(id))).collect()
    }

    /// One contract as the node archived it.
    pub fn contract(&self, id: &ContractID) -> Option<&OutputRecord> {
        self.outputs.get(id)
    }

    /// A page of blocks, newest first.
    pub fn blocks(&self, before: Option<u64>, limit: u32) -> Result<BlocksResult, NodeError> {
        Ok(self.txindex.blocks(before, page_limit(limit)?))
    }

    /// A block header and its execution counts.
    pub fn block_summary(&self, height: u64) -> Result<&BlockSummary, NodeError> {
        self.txindex
            .block(height)
            .ok_or_else(|| NodeError::NotFound(format!("block {height}")))
    }

    /// A header and its executions, including internal deliveries.
    pub fn block(&self, height: u64) -> Result<BlockResult, NodeError> {
        let summary = self.block_summary(height)?.clone();
        let executions = self
            .txindex
            .at_height(height)
            .iter()
            .filter_map(|id| self.txindex.get(id))
            .map(|r| r.summary.clone())
            .collect();
        Ok(BlockResult {
            summary,
            executions,
        })
    }

    /// Current actor code, state, and storage availability.
    pub fn actor(&self, id: ActorId) -> Result<ActorResult, NodeError> {
        let info = self
            .chain
            .actor_info(&ActorID::from(id.0))
            .map_err(|error| match error {
                ChainError::Vm(VMError::ActorNotFound) => {
                    NodeError::NotFound(format!("actor {id}"))
                }
                other => NodeError::Chain(other),
            })?;

        let tip = self.tip();

        Ok(ActorResult {
            id,
            height: tip.height,
            block: BlockId(tip.hash.into_bytes()),
            root_hash: flamed_rpc::CellId(info.root),
            frozen: info.frozen,
            code_hash: info.code_root.map(flamed_rpc::CellId),
            state_hash: info.state_root.map(flamed_rpc::CellId),
            code_size: info.code_size,
            state_size: info.state_size,
            storage_used: info.storage_used,
            storage_capacity: info.storage_capacity,
            code: info.code.map(|code| ActorCode(code.encode())),
            state: info.state.map(|state| ActorStateEnvelope(state.encode())),
            decoded_state: None,
            state_error: None,
            instructions: Vec::new(),
            code_error: None,
        })
    }

    /// Executes a getter with no authenticated caller and no write permission.
    /// An expected tip pins multi-hop lookups: callers restart if the tip changes.
    pub fn query_actor(
        &mut self,
        id: ActorId,
        argument: ValueEnvelope,
        gas_limit: u64,
        expected_block: Option<BlockId>,
    ) -> Result<ActorQueryResult, NodeError> {
        if gas_limit == 0 || gas_limit > MAX_QUERY_GAS {
            return Err(NodeError::QueryGasLimit(gas_limit));
        }
        if argument.0.len() > MAX_QUERY_BYTES {
            return Err(NodeError::Limit {
                max: MAX_QUERY_BYTES,
                got: argument.0.len(),
            });
        }
        let tip = self.tip();
        let block = BlockId(tip.hash.into_bytes());
        if expected_block.is_some_and(|expected| expected != block) {
            return Err(NodeError::StaleBlock);
        }
        let mut decode_gas = MAX_QUERY_GAS;
        let mut envelope = CellEnvelope::decode(&argument.0, MAX_QUERY_BYTES, &mut decode_gas)
            .map_err(NodeError::QueryDecode)?;
        let root = envelope
            .cells()
            .get(&envelope.root())
            .ok_or_else(|| NodeError::QueryDecode(CellError::MissingCell(envelope.root())))?;
        // Dict leaves receive the same recursive data checks inside the VM.
        if !matches!(root.payload().first(), Some(0 | 2 | 3 | 13)) {
            return Err(NodeError::QueryDecode(CellError::InvalidFormat));
        }
        let argument = Value::from_cell(
            &root,
            &mut QueryDecode {
                envelope: &mut envelope,
                gas: decode_gas,
            },
        )
        .map_err(NodeError::QueryDecode)?;
        let result = self
            .chain
            .query_actor(
                ActorID::from(id.0),
                argument,
                gas_limit,
                Arc::new(envelope.into_parts().1),
            )
            .map_err(|error| match error {
                VMError::ActorNotFound => NodeError::NotFound(format!("actor {id}")),
                other => NodeError::Query(other),
            })?;
        let value = result.value.to_envelope()?.encode();
        if value.len() > MAX_QUERY_BYTES {
            return Err(NodeError::Limit {
                max: MAX_QUERY_BYTES,
                got: value.len(),
            });
        }
        Ok(ActorQueryResult {
            block,
            height: tip.height,
            value: ValueEnvelope(value),
            gas_used: result.gas_used,
        })
    }

    /// Confirmed external or internal execution details.
    pub fn transaction(
        &self,
        id: TxId,
        decode_effects: bool,
    ) -> Result<TransactionResult, NodeError> {
        let record = self
            .txindex
            .get(&TxID(id.0))
            .ok_or_else(|| NodeError::NotFound(format!("transaction {id}")))?;

        Ok(TransactionResult {
            summary: record.summary.clone(),
            height: record.height,
            block: BlockId(record.block.into_bytes()),
            block_time: self.block_time(record.height),
            received_at: record.received_at,
            log: record.log.clone(),
            effects: if decode_effects {
                Some(inspect::effects(&record.log)?)
            } else {
                None
            },
        })
    }

    /// Confirmed executions, newest first.
    pub fn transactions(
        &self,
        before: Option<TxId>,
        limit: u32,
    ) -> Result<TransactionsResult, NodeError> {
        self.txindex
            .transactions(before.map(|id| TxID(id.0)), page_limit(limit)?)
            .ok_or_else(|| NodeError::NotFound("transaction cursor".into()))
    }

    /// The first pending transactions in mempool admission order.
    pub fn pending_transactions(&self, limit: u32) -> Result<PendingTransactionsResult, NodeError> {
        let limit = page_limit(limit)?;
        Ok(PendingTransactionsResult {
            transactions: self
                .mempool
                .entries()
                .take(limit)
                .map(|entry| {
                    Ok(PendingTransaction {
                        id: TxId(entry.txid().0),
                        block_tx: BlockTxEnvelope(entry.transaction().to_bytes()?),
                        fee_sparks: entry.fee().to_string(),
                        witness_bytes: entry.witness_bytes() as u64,
                        received_at: self
                            .received
                            .get(&entry.txid().0)
                            .copied()
                            .unwrap_or_default(),
                        locktime: entry.transaction().tx.header.locktime,
                    })
                })
                .collect::<Result<_, CellError>>()?,
            total: self.mempool.len() as u64,
        })
    }

    /// Where a transaction is.
    pub fn tx_status(&self, txid: &TxID) -> TxStatus {
        if self.mempool.entries().any(|entry| entry.txid() == *txid) {
            return TxStatus::Mempool;
        }
        match self.txindex.get(txid) {
            Some(record) => TxStatus::Confirmed {
                height: record.height,
                block: record.block,
            },
            None => TxStatus::Unknown,
        }
    }

    /// Every contract created at `since_height` or later under one of these
    /// predicates, in height order, each with the note that followed it.
    pub fn scan(&self, predicates: &[[u8; 32]], since_height: u64) -> Vec<ScanEntry> {
        let unique: BTreeSet<[u8; 32]> = predicates.iter().copied().collect();
        let mut hits = Vec::new();
        for predicate in unique {
            for id in self.outputs.under(&predicate) {
                let Some(record) = self.outputs.get(id) else {
                    continue;
                };
                if record.height < since_height {
                    continue;
                }
                hits.push(ScanEntry {
                    id: ContractId(*id),
                    height: record.height,
                    txid: TxId(record.txid.0),
                    predicate: PredicatePoint(record.predicate),
                    bytes: ContractEnvelope(record.contract.clone()),
                    time: self.block_time(record.height),
                    note: record.note.clone().map(NoteEnvelope),
                    spent: self.outputs.spend_of(id).map(|spend| SpentAt {
                        height: spend.height,
                        txid: TxId(spend.txid.0),
                    }),
                });
            }
        }
        // Stable, so contracts created in one block keep their order.
        hits.sort_by_key(|hit| hit.height);
        hits
    }

    /// The time of the block at `height`, if the indexer knew it.
    pub fn block_time(&self, height: u64) -> Option<u64> {
        self.txindex.block(height).and_then(|block| block.time)
    }

    /// Whether a proof really proves membership at this tip.
    ///
    /// A `Transient` proof never does: it is a promise that a block is about
    /// to insert the leaf, not evidence that one has.
    pub fn verify_proof(&self, id: &ContractID, proof: &Proof) -> bool {
        let Some(path) = proof.as_path() else {
            return false;
        };
        self.chain
            .contract_forest()
            .verify(&ContractLeaf(*id), path, &utreexo_hasher::<ContractLeaf>())
            .is_ok()
    }

    /// How many transactions are waiting.
    pub fn mempool_len(&self) -> usize {
        self.mempool.len()
    }

    /// How many contracts are unspent.
    pub fn utxo_count(&self) -> usize {
        self.utxos.len()
    }

    /// How many blocks are archived.
    pub fn block_count(&self) -> usize {
        self.store.len()
    }

    /// Mints the next block from whatever the mempool holds.
    ///
    /// The order below is the whole of this method's correctness, and the
    /// two places it is unusual are called out where they happen.
    #[cfg(any(test, feature = "devnet"))]
    pub fn mint_block(&mut self) -> Result<BlockHash, NodeError> {
        // 0. Ask the indexer first: if it cannot answer, nothing has been
        //    touched yet and the next tick tries again.
        let height = self.chain.height().saturating_add(1);
        let anchor = self.indexer.anchor(height)?;
        let core = anchor.hash;

        // 1. Write every candidate down before draining the pool. `BlockTx`
        //    is not `Clone` and `build_block` consumes what it is given, so
        //    bytes are the only form that survives a retry — and a candidate
        //    this node cannot write down is one it could not put back.
        let candidates: Vec<(TxID, Vec<u8>)> = self
            .mempool
            .entries()
            .map(|entry| Ok((entry.txid(), entry.transaction().to_bytes()?)))
            .collect::<Result<_, CellError>>()
            .map_err(NodeError::Cell)?;
        let _ = self.mempool.take_all();

        // The pool is already drained, so an error here would take every
        // candidate with it — and the minter treats a failed build as
        // recoverable and keeps ticking. Put them all back first.
        let (block, taken, mut dropped) = match self.build_next(core, height, &candidates) {
            Ok(built) => built,
            Err(error) => {
                let restored = self.readmit(&candidates);
                eprintln!(
                    "no block at height {height}: {error}; kept {restored} of {} transaction(s)",
                    candidates.len()
                );
                return Err(error);
            }
        };

        // 2. The leftovers go back now, while the tip is still the one they
        //    were built against. The cost is known and accepted: a
        //    transaction spending a contract this very block creates is
        //    refused here rather than deferred, and its sender resubmits.
        dropped += candidates.len() - taken - self.readmit(&candidates[taken..]);

        // 3. Disk before memory, always.
        self.store
            .append(&block)
            .map_err(|source| NodeError::Archive {
                height,
                source: Box::new(source.into()),
            })?;

        let (hash, rebase) = connect_block(
            &mut self.chain,
            &mut self.utxos,
            &mut self.txindex,
            &mut self.outputs,
            Some(&mut self.mempool),
            &block,
            Some(anchor.time),
        )
        .map_err(|source| NodeError::Archive {
            height,
            source: Box::new(source),
        })?;
        self.settle_received();

        dropped += rebase.dropped;
        println!(
            "block {height} {} txs={} dropped={dropped}",
            hex::encode(hash.as_bytes()),
            block.transactions.len()
        );
        Ok(hash)
    }

    /// Hands each confirmed transaction its admission time, and forgets the
    /// times of transactions that left the pool without confirming.
    #[cfg(any(test, feature = "devnet"))]
    fn settle_received(&mut self) {
        let pending: BTreeSet<[u8; 32]> =
            self.mempool.entries().map(|entry| entry.txid().0).collect();
        for (txid, time) in std::mem::take(&mut self.received) {
            if pending.contains(&txid) {
                self.received.insert(txid, time);
            } else {
                self.txindex.set_received(&TxID(txid), time);
            }
        }
    }

    /// Builds the largest prefix of `candidates` that fits in a block.
    ///
    /// Returns the block, how many candidates it accounted for, and how many
    /// of those were dropped outright.
    #[cfg(any(test, feature = "devnet"))]
    fn build_next(
        &mut self,
        core: [u8; 32],
        height: u64,
        candidates: &[(TxID, Vec<u8>)],
    ) -> Result<(Block, usize, usize), NodeError> {
        let mut take = candidates.len();
        loop {
            let transactions = self.decode_batch(&candidates[..take])?;
            match self.chain.build_block(core, transactions) {
                Ok(block) => return Ok((block, take, 0)),
                // Too much for one block: halve and try again. The rest are
                // put back by the caller. Both shapes have to be caught —
                // `check_header` counts transactions and witness bytes
                // itself and says `LimitExceeded`, but it finishes by
                // re-decoding the whole encoded block, and that bound comes
                // back as a `CellError`. A block that is merely too big
                // must never look like a bad transaction.
                Err(error) if take > 1 && is_too_big(&error) => take /= 2,
                // Anything else is one bad transaction somewhere in the
                // batch, and one bad transaction must never halt minting.
                Err(error) => {
                    let block = self.chain.build_block(core, Vec::new())?;
                    // Said only once the empty block is a fact.
                    for (txid, _) in &candidates[..take] {
                        eprintln!(
                            "dropping tx {} at height {height}: {error}",
                            hex::encode(txid.0)
                        );
                    }
                    return Ok((block, take, take));
                }
            }
        }
    }

    /// Offers a slice of the byte snapshot back to the mempool, and says
    /// how many it took. Anything refused is gone, and says so.
    #[cfg(any(test, feature = "devnet"))]
    fn readmit(&mut self, candidates: &[(TxID, Vec<u8>)]) -> usize {
        let height = self.chain.height().saturating_add(1);
        candidates
            .iter()
            .filter(|(txid, bytes)| match self.submit(bytes) {
                Ok(_) => true,
                Err(error) => {
                    eprintln!(
                        "dropping tx {} at height {height}: {error}",
                        hex::encode(txid.0)
                    );
                    false
                }
            })
            .count()
    }

    /// Decodes a slice of the byte snapshot back into transactions.
    #[cfg(any(test, feature = "devnet"))]
    fn decode_batch(&self, candidates: &[(TxID, Vec<u8>)]) -> Result<Vec<BlockTx>, NodeError> {
        candidates
            .iter()
            .map(|(_, bytes)| {
                BlockTx::from_bytes_bounded(bytes, self.params.version, self.params.limits)
                    .map_err(NodeError::Decode)
            })
            .collect()
    }
}

fn page_limit(limit: u32) -> Result<usize, NodeError> {
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(NodeError::Limit {
            got: limit as usize,
            max: MAX_PAGE_SIZE as usize,
        });
    }
    Ok(limit as usize)
}

/// Whether a build failure means the block was too large rather than that
/// something in it was wrong.
#[cfg(any(test, feature = "devnet"))]
fn is_too_big(error: &ChainError) -> bool {
    matches!(
        error,
        ChainError::LimitExceeded | ChainError::Cells(CellError::LimitExceeded)
    )
}

/// This node's mempool policy for a chain with these parameters.
///
/// The per-transaction caps are taken from consensus, not from
/// `MempoolPolicy::default()`, which pins them to the *default* block
/// limits: a network that raises `max_transaction_gas` in
/// `chainparams.toml` would otherwise have every node silently refuse to
/// relay what the chain itself accepts. The two capacity fields are
/// genuinely local — they bound this pool, not one block — and keep their
/// defaults.
fn mempool_policy(params: ChainParams, minimum_fee: u64) -> MempoolPolicy {
    MempoolPolicy {
        max_transaction_gas: params.limits.max_transaction_gas,
        max_gas_credit: params.limits.max_gas_credit,
        max_script_bytes: params.limits.max_transaction_script_bytes,
        max_multiplications: params.limits.max_multiplications_per_transaction,
        max_proofs_per_transaction: params.limits.max_proofs_per_transaction,
        max_proof_depth: params.limits.max_proof_depth,
        minimum_fee,
        ..Default::default()
    }
}

/// The chain a genesis file describes.
///
/// Seeded with the allocations on a devnet build; empty otherwise, because
/// contracts enter a public chain by minting and nothing else.
#[cfg(any(test, feature = "devnet"))]
fn build_chain(
    params: ChainParams,
    ids: &[ContractID],
) -> Result<(Blockchain, Option<flamechain::utreexo::Catchup>), NodeError> {
    let (chain, catchup) = Blockchain::devnet_genesis(params, ids)?;
    Ok((chain, Some(catchup)))
}

#[cfg(not(any(test, feature = "devnet")))]
fn build_chain(
    params: ChainParams,
    ids: &[ContractID],
) -> Result<(Blockchain, Option<flamechain::utreexo::Catchup>), NodeError> {
    if !ids.is_empty() {
        return Err(NodeError::DevnetRequired);
    }
    Ok((Blockchain::new(params)?, None))
}

/// Applies one block to the chain and to everything the chain forgets.
///
/// One function for minting and for replay, so an archived chain and a live
/// one cannot drift apart. `mempool` is `None` during replay, where no pool
/// exists yet. `time` is the core block's, as the indexer reported it.
fn connect_block(
    chain: &mut Blockchain,
    utxos: &mut UtxoSet,
    txindex: &mut TxIndex,
    outputs: &mut OutputIndex,
    mempool: Option<&mut Mempool>,
    block: &Block,
    time: Option<u64>,
) -> Result<(BlockHash, RebaseReport), NodeError> {
    // 1. `connect` reports only `ExecutionRecord { kind, txid }`, so the
    //    effects are read through an observer during block validation.
    //    A log decoded from this node's own archive would be the archive
    //    checking itself.
    let height = block.header.height;
    let block_id = block.header.id();
    let size_bytes = block.to_bytes()?.len() as u64;
    let mut messages = BTreeMap::new();
    let mut staged: Vec<(TxID, TxRecord, Vec<UtxoEffect>)> = Vec::new();

    // 2. Nothing is recorded before the chain has accepted the block.
    let applied = chain.connect_with_observer(block, |record, log, error| {
        let txid = record.txid;

        let mut changes = Vec::new();
        let mut received = None;
        let mut inputs = 0;
        let mut output_count = 0;
        let mut sends = 0;
        let mut fees = 0u128;

        // 3. In log order, not inputs then outputs: a contract created and
        //    spent inside one block has to end up spent, and two passes
        //    would leave it unspent and spent at once. An output keeps the
        //    entry after it when that entry is `Data`: the output's note,
        //    kept byte for byte and never parsed here.
        let entries = log.entries();
        let locktime = match entries.first() {
            Some(TxEntry::Header(header)) => header.locktime,
            _ => 0,
        };
        for (index, entry) in entries.iter().enumerate() {
            match entry {
                TxEntry::Input(id) => {
                    inputs += 1;
                    changes.push(UtxoEffect::Input(*id));
                }
                TxEntry::Output(contract) => {
                    output_count += 1;
                    let id = contract.id();
                    changes.push(UtxoEffect::Output(
                        id,
                        OutputRecord {
                            height,
                            txid,
                            predicate: contract.predicate.to_point().to_bytes(),
                            contract: contract_bytes(contract)?,
                            note: match entries.get(index + 1) {
                                Some(TxEntry::Data(note)) => Some(note.clone()),
                                _ => None,
                            },
                        },
                    ));
                }
                TxEntry::Fee(q) => fees += u128::from(*q),
                TxEntry::Send(m) => {
                    sends += 1;
                    let target = ActorId(m.target.to_hash());
                    messages.insert(m.id().0, (TxId(txid.0), target));
                }
                TxEntry::Receive(id) => {
                    received = Some(messages.remove(id).ok_or(ChainError::InvalidEffectLog)?);
                }
                _ => {}
            }
        }

        let execution = match (record.kind, received, error) {
            (ExecutionKind::External, None, None) => ExecutionData::External,
            (ExecutionKind::Internal, Some((parent, actor)), None) => {
                ExecutionData::Internal { parent, actor }
            }
            (ExecutionKind::InternalFailed, Some((parent, actor)), Some(error)) => {
                ExecutionData::InternalFailed {
                    parent,
                    actor,
                    error: error.to_owned(),
                }
            }
            _ => return Err(ChainError::InvalidEffectLog),
        };
        let summary = TransactionSummary {
            id: TxId(txid.0),
            execution,
            fee_sparks: fees.to_string(),
            inputs,
            outputs: output_count,
            messages: sends,
            locktime,
        };

        // 4. The log itself, for anyone who asks what this transaction did.
        staged.push((
            txid,
            TxRecord {
                height,
                block: block_id,
                log: log_bytes(log)?,
                summary,
                received_at: None,
            },
            changes,
        ));
        Ok(())
    })?;

    // 5. Every execution the chain ran that was not one of these external
    //    transactions produced effects this node can now see through
    //    `connect_with_observer`. It is reachable
    //    today, without a single actor existing — a `send` in an external
    //    transaction finds no actor, bounces, and the bounce mints a
    //    refund contract under the sender's refund predicate. That
    //    contract is real and in the accumulator, and this node can serve
    //    both a proof and a record for it.
    for (txid, record, changes) in staged {
        for change in changes {
            match change {
                UtxoEffect::Input(id) => {
                    utxos.remove(&id);
                    outputs.spend(id, SpendRecord { height, txid });
                }
                UtxoEffect::Output(id, output) => {
                    utxos.insert_transient(id);
                    outputs.insert(id, output);
                }
            }
        }
        txindex.insert(txid, record);
    }

    let internal = applied
        .records
        .iter()
        .filter(|r| r.kind != ExecutionKind::External)
        .count() as u32;
    let failed = applied
        .records
        .iter()
        .filter(|r| r.kind == ExecutionKind::InternalFailed)
        .count() as u32;

    txindex.insert_block(block_summary(
        &block.header,
        time,
        block.transactions.len() as u32,
        internal,
        failed,
        size_bytes,
    ));

    // 6. Every surviving proof moves to the new tip, and the ids this block
    //    inserted turn from `Transient` into `Committed`. After the removals,
    //    because a deleted leaf has no proof to lift; after the inserts,
    //    because a new id has to be in the map to be promoted.
    utxos.apply_catchup(&applied.catchup)?;

    // 7. Whatever the mempool still holds is replayed on the new tip.
    let rebase = match mempool {
        Some(mempool) => {
            let report = mempool.rebase(chain, Some(&applied.catchup));
            if report.dropped != 0 {
                eprintln!(
                    "mempool dropped {} transaction(s) rebasing onto height {height}",
                    report.dropped
                );
            }
            report
        }
        None => RebaseReport {
            kept: 0,
            dropped: 0,
        },
    };

    Ok((applied.id, rebase))
}

enum UtxoEffect {
    Input(ContractID),
    Output(ContractID, OutputRecord),
}

fn block_summary(
    header: &flamechain::BlockHeader,
    time: Option<u64>,
    transactions: u32,
    internal: u32,
    failed: u32,
    size_bytes: u64,
) -> BlockSummary {
    BlockSummary {
        id: BlockId(header.id().into_bytes()),
        header: BlockHeader {
            version: header.version,
            height: header.height,
            core_block_hash: header.core_block_hash,
            parent: BlockId(header.parent.into_bytes()),
            witness_root: header.witness_root,
            effects_root: header.effects_root,
            state: StateCommitment {
                contracts: header.state.contracts.0,
                actors: header.state.actors,
                available_storage_units: header.state.available_storage_units,
            },
        },
        time,
        transactions,
        internal,
        failed,
        size_bytes,
    }
}

/// Something the node could not do.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// The node has no such contract.
    #[error("no contract {0}")]
    UnknownContract(String),
    /// A requested actor, block, or confirmed execution is absent.
    #[error("not found: {0}")]
    NotFound(String),
    /// A request named more items than the method allows.
    #[error("{got} items, at most {max}")]
    Limit {
        /// The bound.
        max: usize,
        /// What was asked for.
        got: usize,
    },
    /// Inbound bytes that this chain will not read as a transaction —
    /// malformed, or well formed and past one of this chain's limits. The
    /// `CellError` says which.
    #[error("the submitted bytes do not decode as a transaction under this chain's limits: {0}")]
    Decode(CellError),
    /// A query argument is not a bounded, data-only Value envelope.
    #[error("invalid actor query argument: {0}")]
    QueryDecode(CellError),
    /// Query execution gas must be positive and fit the node's bound.
    #[error("actor query gas must be 1..={MAX_QUERY_GAS}, got {0}")]
    QueryGasLimit(u64),
    /// A lookup's pinned block is no longer the current tip.
    #[error("the expected query block is not the current tip")]
    StaleBlock,
    /// A getter failed without committing effects.
    #[error("actor query failed: {0}")]
    Query(VMError),
    /// The mempool refused a transaction.
    #[error(transparent)]
    Mempool(#[from] MempoolError),
    /// The recorded genesis hash is not the one this node derives.
    #[error("genesis.json records {recorded} but this node derives {derived}")]
    GenesisHashMismatch {
        /// What the file says.
        recorded: String,
        /// What the parameters and allocations give.
        derived: String,
    },
    /// A genesis file that cannot be believed.
    #[error(transparent)]
    Genesis(#[from] GenesisError),
    /// An archived block that will not replay.
    #[error("the block archived at height {height} does not replay: {source}")]
    Replay {
        /// Which height.
        height: u64,
        /// Why.
        source: Box<NodeError>,
    },
    /// A genesis file with allocations, on a build that cannot have them.
    #[error("this build has no genesis allocations; rebuild with --features devnet")]
    DevnetRequired,
    /// Disk and memory may no longer agree.
    #[error("the archive and memory may disagree at height {height}: {source}")]
    Archive {
        /// Which height.
        height: u64,
        /// Why.
        source: Box<NodeError>,
    },
    /// The chain refused something.
    #[error(transparent)]
    Chain(#[from] ChainError),
    /// A transaction did not verify.
    #[error(transparent)]
    Vm(#[from] VMError),
    /// A proof could not be lifted.
    #[error(transparent)]
    Utreexo(#[from] UtreexoError),
    /// This node's own encoding failed.
    #[error(transparent)]
    Cell(#[from] CellError),
    /// The archive.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The core indexer.
    #[error("core indexer: {0}")]
    Indexer(#[from] CoreError),
    /// The filesystem.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
