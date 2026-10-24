//! The methods `flamed` serves.
//!
//! The macro generates `FlamedApiServer`, which `flamed` implements, and
//! `FlamedApiClient`, blanket-implemented for every jsonrpsee client, so the
//! wire is written once and a wallet, an explorer or a monitor speaks it
//! without repeating a single method name.
//!
//! Nothing else belongs in this module: attributes on the trait are discarded
//! with it, so a lint allow would have to be module-level, and an item named
//! `core` or `jsonrpsee` here would shadow the paths the macro emits.

use jsonrpsee::core::RpcResult;
use jsonrpsee::proc_macros::rpc;

use crate::types::{
    ActorId, ActorQueryResult, ActorResult, BlockId, BlockResult, BlockTxEnvelope, BlocksResult,
    ContractId, ContractResult, HistoryResult, PendingTransactionsResult, PredicatePoint,
    ProofResult, ScanResult, TipResult, TransactionResult, TransactionsResult, TxId,
    TxStatusResult, UnspentResult, ValueEnvelope,
};

/// The `flamed` JSON-RPC 2.0 API. Parameters are positional, in the order
/// written; method names carry no prefix.
#[rpc(server, client)]
pub trait FlamedApi {
    /// Blocks newest first, strictly below `before` when provided. Limit 1..100.
    /// Includes genesis at height zero.
    #[method(name = "blocks")]
    async fn blocks(&self, before: Option<u64>, limit: u32) -> RpcResult<BlocksResult>;

    /// One block by height, including its internal executions.
    /// Genesis is height zero; `NOT_FOUND` if the height is absent.
    #[method(name = "block")]
    async fn block(&self, height: u64) -> RpcResult<BlockResult>;

    /// Confirmed executions, newest first.
    /// `before` excludes the named execution and all newer executions. Limit 1..100.
    /// Returns `NOT_FOUND` if the cursor does not name a confirmed execution.
    #[method(name = "transactions")]
    async fn transactions(&self, before: Option<TxId>, limit: u32)
        -> RpcResult<TransactionsResult>;

    /// The first pending transactions in mempool admission order. Limit 1..100.
    #[method(name = "pending_transactions")]
    async fn pending_transactions(&self, limit: u32) -> RpcResult<PendingTransactionsResult>;

    /// One confirmed external or internal execution.
    /// Set `decode_effects` to true to decode public effects. The default is false.
    /// Returns `NOT_FOUND` for unknown or pending transactions.
    #[method(name = "tx")]
    async fn tx(&self, id: TxId, decode_effects: Option<bool>) -> RpcResult<TransactionResult>;

    /// Current actor state and code, with decoded state and instructions.
    /// Returns `NOT_FOUND` if the actor does not exist.
    #[method(name = "actor")]
    async fn actor(&self, id: ActorId) -> RpcResult<ActorResult>;

    /// Runs an actor getter against the current committed tip, without effects.
    /// Arguments and results must be copyable and portable, as Value envelopes.
    /// Dicts must retain both sticky flags, including when empty.
    /// Gas must be 1..=MAX_QUERY_GAS; envelopes are limited to MAX_QUERY_BYTES.
    /// If supplied, `expected_block` must match the current tip; a mismatch
    /// returns STALE_BLOCK. Historical blocks are not queried.
    #[method(name = "query_actor")]
    async fn query_actor(
        &self,
        id: ActorId,
        argument: ValueEnvelope,
        gas_limit: u64,
        expected_block: Option<BlockId>,
    ) -> RpcResult<ActorQueryResult>;

    /// The current tip: its hash, its height, and the contract accumulator root.
    #[method(name = "tip")]
    async fn tip(&self) -> RpcResult<TipResult>;

    /// One contract's membership proof, or what became of it.
    #[method(name = "proof")]
    async fn proof(&self, id: ContractId) -> RpcResult<ProofResult>;

    /// The same question for up to `MAX_PROOF_IDS` contracts at once.
    #[method(name = "proofs")]
    async fn proofs(&self, ids: Vec<ContractId>) -> RpcResult<Vec<(ContractId, ProofResult)>>;

    /// An archived contract, its spend status, and its decoded public payload.
    /// Returns `NOT_FOUND` if the contract does not exist.
    #[method(name = "contract")]
    async fn contract(&self, id: ContractId) -> RpcResult<ContractResult>;

    /// Offers a transaction to the mempool. `INVALID_BYTES` if it does not
    /// decode, `MEMPOOL_REJECTED` if the pool refuses it.
    #[method(name = "submit_tx")]
    async fn submit_tx(&self, block_tx: BlockTxEnvelope) -> RpcResult<TxId>;

    /// Whether a transaction is unknown, waiting, or confirmed.
    #[method(name = "tx_status")]
    async fn tx_status(&self, txid: TxId) -> RpcResult<TxStatusResult>;

    /// Every contract created at `since_height` or later under one of these
    /// predicates, each with the note that followed it in its log.
    /// `LIMIT_EXCEEDED` past `MAX_SCAN_PREDICATES`.
    #[method(name = "scan")]
    async fn scan(
        &self,
        predicates: Vec<PredicatePoint>,
        since_height: u64,
    ) -> RpcResult<ScanResult>;

    /// Every unspent contract locked to one receiving or tracking address,
    /// with a proof for each against the current tip. Set `with_pending` to
    /// true to apply the mempool on top: contracts a pending transaction
    /// spends are left out, and contracts it creates are added without a
    /// height or a proof. The default is false.
    /// `INVALID_ADDRESS` if the address does not decode for this network.
    #[method(name = "unspent")]
    async fn unspent(
        &self,
        address: String,
        with_pending: Option<bool>,
    ) -> RpcResult<UnspentResult>;

    /// What happened to a wallet's addresses, a page at a time, with a
    /// cursor at each end of what has been read (`HistoryResult`).
    ///
    /// Without `cursor`, the newest entries; with an `older` cursor, the
    /// entries before it; with a `newer` one, the entries after it, oldest
    /// first. Every page needs the same `addresses`, up to
    /// `MAX_HISTORY_ADDRESSES`. `limit` is 1..=`MAX_PAGE_SIZE`.
    /// `with_pending` (default false) adds the mempool to a first or newer
    /// page. Each entry also names the other side: whose contracts it spent
    /// and whom it created contracts for, beyond the asked addresses.
    /// `STALE_BLOCK` if the block a cursor stands on is no longer in the
    /// chain: start again without one. `INVALID_ADDRESS` for an address,
    /// invalid params for a cursor that does not read or was made for other
    /// addresses.
    #[method(name = "history")]
    async fn history(
        &self,
        addresses: Vec<String>,
        cursor: Option<String>,
        limit: u32,
        with_pending: Option<bool>,
    ) -> RpcResult<HistoryResult>;
}
