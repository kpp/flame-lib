//! FlameVM execution engine: Tx → CallFrame + dispatch loop.

use bulletproofs::r1cs;
use bulletproofs::r1cs::R1CSProof;
#[cfg(test)]
use cells::{read_cell, CellRef};
use cells::{
    Cell, CellBuilder, CellDecode, CellEncode, CellError, CellID, CellIndex, CellReader, CellSlice,
};
use core::convert::{TryFrom, TryInto};
use core::mem;
use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::scalar::Scalar as DalekScalar;
use merlin::Transcript;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::actor::{empty_state, ActorAddress, ActorID, ActorRegistry};
use crate::cell_value::array32;
use crate::constraints::Commitment;
use crate::contract::{Contract, ContractID, Predicate};
use crate::errors::VMError;
use crate::fees::CheckedFee;
use crate::message::Message;
use crate::ops::Instruction;
use crate::script::{Program, Script, ScriptBuilder};
use crate::token::{flavor_from_actor, flavor_from_predicate, FLAME_FLAVOR};
use crate::tx::TxHeader;
use crate::tx::{TxEntry, TxID};
use crate::{
    ClearToken, Constraint, Dict, Expression, Merlin, Point, Scalar, Token, Value, Variable,
    WideToken,
};

#[cfg(feature = "debugger")]
#[path = "debugger.rs"]
pub mod debugger;

/// Bitcoin BIP-65 convention threshold for distinguishing
/// `TxHeader::locktime` as a block height vs. a Unix timestamp:
/// values `< 500_000_000` are block heights, `≥` are timestamps
/// (~1985-11-05 epoch). Surfaced to scripts via the `timelock`
/// opcode's `{0|1}` flag.
pub const LOCKTIME_TIMESTAMP_THRESHOLD: u32 = 500_000_000;

/// 32-byte anchor. Chained via `ratchet` to make outputs unique
/// within a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor(pub [u8; 32]);

impl Anchor {
    /// Deterministically splits this anchor into two distinct
    /// children. Used by every consume site that needs a unique
    /// entity-anchor: the `left` half is embedded in the new entity
    /// (contract / message / callee frame), the `right` half replaces
    /// the consumer's current anchor.
    ///
    /// Uniqueness inherits from the parent: if `self` came from a
    /// spend-once source (an input contract's id) and from a chain of
    /// prior splits over that source, both children are unique
    /// within the network.
    pub fn split(&self) -> (Anchor, Anchor) {
        let mut t = Transcript::new(b"flamevm.anchor.split");
        t.append_message(b"parent", &self.0);
        let mut left = [0u8; 32];
        let mut right = [0u8; 32];
        t.challenge_bytes(b"left", &mut left);
        t.challenge_bytes(b"right", &mut right);
        (Anchor(left), Anchor(right))
    }
}

/// 32 raw bytes — fixed size, no tag, no length prefix.
impl CellEncode for Anchor {
    fn encode(&self, w: &mut CellBuilder) -> Result<(), CellError> {
        w.store_bytes(&self.0)?;
        Ok(())
    }
}

impl CellDecode for Anchor {
    fn decode<R: CellReader + ?Sized>(
        r: &mut CellSlice<'_>,
        cells: &mut R,
    ) -> Result<Self, CellError> {
        Ok(Anchor(<[u8; 32]>::decode(r, cells)?))
    }
}

// `Predicate` lives in `contract::predicate`; re-exported via `Predicate`.

/// External signature check deferred to transaction finalization. `TxBound`
/// comes from external-only `signtx`; `Explicit` comes from external
/// `signcall`. Internal `signcall` signatures are checked immediately and are
/// never recorded here.
#[derive(Clone, Debug)]
pub enum DeferredSig {
    TxBound {
        verification_key: CompressedRistretto,
        contract_id: ContractID,
    },
    Explicit {
        verification_key: CompressedRistretto,
        message: Vec<u8>,
        signature: [u8; 64],
    },
}

/// Consensus transcript for a `signcall` signature after the factual program Cell ID
/// have been reduced to `message`. Shared by immediate internal verification
/// and deferred external batch verification.
pub(crate) fn signcall_verification_transcript(message: &[u8]) -> Transcript {
    let mut transcript = Transcript::new(b"flamevm.signcall");
    transcript.append_message(b"msg", message);
    transcript
}

/// Block-level immutable context (height, chain stats).
pub struct BlockContext {
    pub height: u64,
}

/// External-context user-task abstraction: prover or verifier. Owns the
/// R1CS constraint system and finalizes proofs and signatures.
pub trait Delegate {
    /// The R1CS constraint system. The
    /// [`r1cs::CheckpointableConstraintSystem`] bound lets the VM
    /// snapshot the CS on call entry and roll back the witness +
    /// constraint vectors + transcript on call failure — so a
    /// failed nested call's CS effects can't pollute the caller's
    /// proof.
    type CS: r1cs::RandomizableConstraintSystem + r1cs::CheckpointableConstraintSystem;
    /// Per-side batched scalar-point check accumulator. The
    /// [`musig::BatchCheckpoint`] bound lets the VM snapshot the
    /// accumulator on call entry and restore it on call failure —
    /// so a failed nested call's MSM contributions can't pollute
    /// the caller's proof.
    type BatchVerifier: musig::BatchVerification + musig::BatchCheckpoint;

    fn cs(&mut self) -> &mut Self::CS;

    fn batch_verifier(&mut self) -> &mut Self::BatchVerifier;

    /// Adds a Commitment to the CS, producing a high-level variable.
    fn commit_variable(
        &mut self,
        commitment: &Commitment,
    ) -> Result<(CompressedRistretto, r1cs::Variable), VMError>;
}

/// No-op [`Delegate`] used by internal-context steps. CS opcodes
/// route through `require_external()` and never reach these methods.
struct InternalDelegate;

impl Delegate for InternalDelegate {
    type CS = r1cs::Verifier<merlin::Transcript>;
    type BatchVerifier = musig::BatchVerifier<rand::rngs::ThreadRng>;

    fn cs(&mut self) -> &mut Self::CS {
        unreachable!("InternalDelegate::cs — CS opcodes are external-only")
    }
    fn batch_verifier(&mut self) -> &mut Self::BatchVerifier {
        unreachable!("InternalDelegate::batch_verifier — sig batching is external-only")
    }
    fn commit_variable(
        &mut self,
        _commitment: &Commitment,
    ) -> Result<(CompressedRistretto, r1cs::Variable), VMError> {
        unreachable!("InternalDelegate::commit_variable — commit/expr/mix are external-only")
    }
}

/// Base cost charged for every fetched instruction, executed or skip-scanned.
/// Expensive handlers add their calibrated work charge below.
const GAS_PER_INSTRUCTION: u64 = 1;

/// Variable-size heap work is charged monotonically in the current frame.
/// Drops do not refund it, so cumulative charged growth is a deterministic
/// upper bound on peak live allocation.
const GAS_PER_ALLOC_BYTE: u64 = 1;
const GAS_PER_ALLOC_ITEM: u64 = 1;

/// Warm reads still validate/decode; discount their body work by four, not to zero.
const GAS_CELL_READ: u64 = 1;
const GAS_CELL_WARM_DIVISOR: u64 = 4;

// Calibrated with `cargo bench -p flamevm --bench gas` on 2026-08-23.
// One gas represents roughly 100 ns of verifier work on the reference machine;
// every value is rounded upward. Memory charges remain deliberately more
// conservative because they prove a bound rather than model an allocator.
const GAS_HASH_BASE: u64 = 2;
const GAS_HASH_BLOCK: u64 = 2;
const GAS_POINT_DECOMPRESS: u64 = 25;
const GAS_SIGNATURE_VERIFY: u64 = 350;
const GAS_EXTERNAL_FINALIZE_BASE: u64 = 2_000;
const GAS_R1CS_ITEM: u64 = 120;
const GAS_MSM_VERIFY_BASE: u64 = 220;
const GAS_MSM_VERIFY_TERM: u64 = 12 + GAS_POINT_DECOMPRESS;
// Policy premium for non-batchable, data-dependent execution; not a measured timing ratio.
const GAS_MSM_EVAL_FACTOR: u64 = 4;

fn alloc_byte_gas(n: usize) -> Result<u64, VMError> {
    u64::try_from(n)
        .map(|n| n.saturating_mul(GAS_PER_ALLOC_BYTE))
        .map_err(|_| VMError::OutOfGas)
}

fn alloc_item_gas(n: usize) -> Result<u64, VMError> {
    u64::try_from(n)
        .map(|n| n.saturating_mul(GAS_PER_ALLOC_ITEM))
        .map_err(|_| VMError::OutOfGas)
}

fn linear_gas(base: u64, per_item: u64, n: usize) -> Result<u64, VMError> {
    let n = u64::try_from(n).map_err(|_| VMError::OutOfGas)?;
    base.checked_add(n.checked_mul(per_item).ok_or(VMError::OutOfGas)?)
        .ok_or(VMError::OutOfGas)
}

/// Full opcode price: instruction, term traversal, crypto work and two batch vectors.
fn msm_verify_gas(terms: usize) -> Result<u64, VMError> {
    linear_gas(
        GAS_PER_INSTRUCTION + GAS_MSM_VERIFY_BASE,
        GAS_MSM_VERIFY_TERM + 1 + 2 * GAS_PER_ALLOC_ITEM,
        terms,
    )
}

fn msm_eval_gas(terms: usize) -> Result<u64, VMError> {
    msm_verify_gas(terms)?
        .checked_mul(GAS_MSM_EVAL_FACTOR)
        .ok_or(VMError::OutOfGas)
}

fn hash_gas(n: usize, block_bytes: usize) -> Result<u64, VMError> {
    // Include one padding/finalization block. The deliberate extra block at an
    // exact boundary keeps the formula simple and conservatively priced.
    let blocks = n
        .checked_div(block_bytes)
        .and_then(|blocks| blocks.checked_add(1))
        .ok_or(VMError::OutOfGas)?;
    linear_gas(GAS_HASH_BASE, GAS_HASH_BLOCK, blocks)
}

// Conservatively bound all sixteen significant-level SHA256 preimages.
fn cell_hash_gas(bytes: usize, refs: usize) -> Result<u64, VMError> {
    hash_gas(bytes + 4 + 34 * refs, 64)?
        .checked_add(15 * hash_gas(36 + 34 * refs, 64)?)
        .ok_or(VMError::OutOfGas)
}

fn cell_view_info<'a>(
    bytes: usize,
    refs: impl ExactSizeIterator<Item = &'a cells::CellRef> + Clone,
) -> Result<[u64; 5], CellError> {
    let count = refs.len();
    let mask = refs
        .clone()
        .fold(0u16, |mask, reference| mask | reference.commitment().mask());
    let level = (u16::BITS - mask.leading_zeros()) as u8;
    let mut depth = 0;
    for reference in refs {
        depth = depth.max(
            reference
                .commitment()
                .depth(level)?
                .checked_add(1)
                .ok_or(CellError::DepthOverflow)?,
        );
    }
    Ok([
        bytes as u64,
        count as u64,
        u64::from(depth),
        u64::from(level),
        0,
    ])
}

fn factual_hash_gas(value: &Value) -> Result<u64, VMError> {
    match value {
        Value::Slice(slice) => cell_hash_gas(slice.remaining_bytes(), slice.remaining_refs()),
        Value::Builder(builder) => cell_hash_gas(builder.used_bytes(), builder.used_refs()),
        _ => Ok(0),
    }
}

fn shift_bytes(bytes: &mut [u8], count: usize, left: bool) {
    if count >= bytes.len() * 8 {
        bytes.fill(0);
        return;
    }
    let whole = count / 8;
    let bits = count % 8;
    if left {
        for index in 0..bytes.len() {
            let source = index + whole;
            let first = bytes.get(source).copied().unwrap_or(0);
            let second = bytes.get(source + 1).copied().unwrap_or(0);
            bytes[index] = (first << bits) | if bits == 0 { 0 } else { second >> (8 - bits) };
        }
    } else {
        for index in (0..bytes.len()).rev() {
            let first = index
                .checked_sub(whole)
                .and_then(|i| bytes.get(i))
                .copied()
                .unwrap_or(0);
            let second = index
                .checked_sub(whole + 1)
                .and_then(|i| bytes.get(i))
                .copied()
                .unwrap_or(0);
            bytes[index] = (first >> bits) | if bits == 0 { 0 } else { second << (8 - bits) };
        }
    }
}

fn r1cs_gas(items: usize) -> Result<u64, VMError> {
    linear_gas(0, GAS_R1CS_ITEM, items)
}

/// Maximum nested call/open/signcall depth. Re-entrancy is permitted
/// (ADR 0017), so without this a load-free A↔B cycle would be bounded
/// only by gas; the cap restores a structural bound (docs/flamevm.md §Design).
const MAX_CALL_DEPTH: usize = 64;

/// A frame's executable code. The prover holds decoded, witness-bearing
/// instructions; the verifier and internal actor execution hold raw
/// bytecode and decode one instruction at a time — never materializing a
/// `Vec<Instruction>`. See ADR 0015.
impl CallFrame {
    // The frame's code + cursor + lazy label table (fields `code` /
    // `cursor` / `labels`) are walked directly by these methods — exactly
    // Payload fetch and literal-reference consumption are independent streams.

    /// Returns the next instruction; `Ok(None)` at end of program. For
    /// `Script::Opaque` this decodes one instruction at the cursor and
    /// advances by its encoded length.
    pub(crate) fn next_instruction(&mut self) -> Result<Option<Instruction>, VMError> {
        self.prepare_program()?;
        if self.cursor >= self.program.as_ref().unwrap().len {
            return Ok(None);
        }
        let cursor = self.cursor;
        match &self.instructions {
            Some(instrs) => {
                let instr = instrs[self.instruction_cursor].clone();
                self.instruction_cursor += 1;
                self.cursor += instr.encoded_size();
                Ok(Some(instr))
            }
            None => {
                let bytes = self.program.as_ref().unwrap().bytes(cursor);
                let mut reader: &[u8] = bytes;
                let before = reader.len();
                let instr = Instruction::parse(&mut reader)?;
                let consumed = before - reader.len();
                self.cursor = cursor + consumed;
                Ok(Some(instr))
            }
        }
    }

    /// True iff the frame has reached its end. Test-only — production
    /// code drives to completion via the dispatch loop.
    #[cfg(test)]
    pub(crate) fn is_finished(&self) -> bool {
        match &self.program {
            Some(program) => self.cursor >= program.len,
            None => match self.code.as_ref().unwrap() {
                Script::Transparent(instructions) => instructions.is_empty(),
                Script::Opaque(bytes) => bytes.is_empty(),
                Script::Cell(cell) => !cell.is_pruned() && cell.payload().is_empty(),
            },
        }
    }

    /// Debits `n` gas from this frame's budget; `OutOfGas` when the
    /// budget is exhausted. Charged per fetched instruction (executed
    /// or skip-scanned), so prover (`Instrs`) and verifier (`Bytes`)
    /// meter identically — they walk the same instruction sequence.
    fn charge_gas(&mut self, n: u64) -> Result<(), VMError> {
        self.gas_used = self.gas_used.checked_add(n).ok_or(VMError::OutOfGas)?;
        if self.gas_used > self.gas_limit {
            return Err(VMError::OutOfGas);
        }
        Ok(())
    }

    /// Records `label n` at the current cursor (the position just after
    /// the label). `n` must be the next sequential index, except a
    /// re-visit (`n < len`, a loop back-edge) which must match the
    /// stored position. See ADR 0015.
    fn record_label(&mut self, n: usize) -> Result<(), VMError> {
        use core::cmp::Ordering;
        match n.cmp(&self.labels.len()) {
            Ordering::Equal => {
                self.labels.push((self.cursor, self.instruction_cursor));
                Ok(())
            }
            Ordering::Less if self.labels[n] == (self.cursor, self.instruction_cursor) => Ok(()),
            _ => Err(VMError::LabelOutOfOrder),
        }
    }

    /// Moves the cursor to `label n`: immediate if already recorded,
    /// else scans forward (recording labels, not executing) until it is
    /// reached. `LabelNotFound` if the scan hits end-of-program. See
    /// ADR 0015.
    fn jump_to_label(&mut self, n: usize) -> Result<(), VMError> {
        if n < self.labels.len() {
            (self.cursor, self.instruction_cursor) = self.labels[n];
            return Ok(());
        }
        // Scan forward via `next_instruction` (representation-agnostic:
        // decoded Vec or byte stream), recording labels, until `n`.
        // Each scanned instruction is charged like an executed one, so
        // long skips can't be free (ADR 0015 gas-on-scan).
        loop {
            self.charge_gas(GAS_PER_INSTRUCTION)?;
            match self.next_instruction()? {
                None => return Err(VMError::LabelNotFound),
                Some(Instruction::Label(m)) => {
                    let m = m as usize;
                    self.record_label(m)?;
                    if m == n {
                        return Ok(());
                    }
                }
                Some(_) => {}
            }
        }
    }
}

impl cells::GasMeter for CallFrame {
    fn charge(&mut self, amount: u64) -> Result<(), CellError> {
        self.charge_gas(amount)
            .map_err(|_| CellError::ResourceExhausted)
    }
}

/// Identity-bearing scope tag carried by every CallFrame.
pub enum CallKind {
    /// Outer scope of an external transaction.
    ExternalRoot,

    /// Outer scope of an internal tx; dispatched to the target's method.
    InternalRoot {
        actor: ActorID,
        caller: Option<ActorID>,
    },

    /// Synchronous actor-to-actor call inside an internal tx.
    ActorCall { actor: ActorID, caller: ActorID },

    /// `open` / `signcall` of a contract predicate. `external_context`
    /// snapshots the caller's `is_external()` at frame creation so
    /// `require_external` propagates correctly across the isolation
    /// boundary (ADR 0013).
    ContractOpen {
        predicate: Predicate,
        external_context: bool,
        /// Canonical id of the directly invoking actor, for `callerid`
        /// introspection only. This never grants actor authority.
        caller_id: Option<[u8; 32]>,
    },
}

impl CallKind {
    /// Returns the actor identity of this frame, if any. Used by the
    /// re-entrancy guard.
    pub fn actor(&self) -> Option<&ActorID> {
        match self {
            Self::InternalRoot { actor, .. } | Self::ActorCall { actor, .. } => Some(actor),
            Self::ExternalRoot | Self::ContractOpen { .. } => None,
        }
    }

    /// Returns only the caller's canonical id. `ContractOpen` stores this
    /// compact form for read-only attribution without inheriting the actor.
    pub fn caller_id(&self) -> Option<[u8; 32]> {
        match self {
            Self::InternalRoot { caller, .. } => caller.as_ref().map(ActorID::to_hash),
            Self::ActorCall { caller, .. } => Some(caller.to_hash()),
            Self::ContractOpen { caller_id, .. } => *caller_id,
            Self::ExternalRoot => None,
        }
    }
}

/// An isolated execution scope with its own stack, code, and gas budget.
/// Created by `call`, `open`, or the outermost frame of a transaction.
pub struct CallFrame {
    /// Isolated stack visible to scripts in this scope.
    pub(crate) stack: Vec<Value>,

    /// Only this actor's committed storage bodies; never another actor's cache.
    actor_cells: Arc<CellIndex>,

    /// Source consumed on first fetch; private instructions are only an overlay.
    code: Option<Script>,
    program: Option<Program>,
    entry_program: Option<Cell>,
    private_scripts: BTreeMap<CellID, Vec<Instruction>>,
    instructions: Option<Vec<Instruction>>,
    instruction_cursor: usize,
    /// Byte cursor into the program's Cell payloads.
    cursor: usize,
    /// `labels[n]` = (byte offset, private overlay index) after `label n`; filled lazily
    /// in appearance order. See ADR 0015.
    labels: Vec<(usize, usize)>,

    /// Identity / dispatch context for this frame.
    pub(crate) kind: CallKind,

    /// This frame's starting anchor (todo #4 — previously stored in
    /// every `CallKind` variant). Only the root internal frame uses it:
    /// `VM::new` seeds `last_anchor` from it. Child frames (`op_call` /
    /// contract-open) set `last_anchor` directly on entry, so theirs is
    /// informational. `None` for `ExternalRoot` (seeded by `op_input`).
    pub(crate) anchor: Option<Anchor>,

    /// Gas budget for this call.
    pub(crate) gas_limit: u64,
    pub(crate) gas_used: u64,

    /// Anchor that should replace `VM.last_anchor` when control
    /// returns to this frame from a child call. Populated at call
    /// entry as the `right` half of the parent's anchor split (the
    /// `left` half becomes the child frame's starting anchor).
    /// `None` when no call is in flight from this frame.
    pub(crate) post_call_anchor: Option<Anchor>,

    /// Snapshots taken at child-call entry, used to roll back side
    /// effects if the call fails. All `Vec` cursors and the fee
    /// accumulator are restored on failure; on success they're
    /// discarded.
    pub(crate) snap_txlog_len: usize,
    pub(crate) snap_deferred_sigs_len: usize,
    pub(crate) snap_deferred_multiplications: usize,
    pub(crate) snap_total_fee: CheckedFee,

    /// Entry-owned values returned to the caller when the child fails.
    /// Actor calls escrow one argument; contract calls escrow the original
    /// locked Contract followed by the explicit argument.
    pub(crate) snap_failure_values: Vec<Value>,

    /// Snapshot of the delegate's MSM/signature batch state taken
    /// when this frame's child was pushed. Restored on child
    /// failure so the polluting MSM appended by the failed callee
    /// is dropped from the global batch — keeping the caller's
    /// proof verifiable. `None` when no call is in flight (or in
    /// internal context where the delegate has no real batch).
    pub(crate) snap_batch: Option<musig::BatchSnapshot>,

    /// Snapshot of the delegate's R1CS constraint system taken
    /// when this frame's child was pushed. Restored on child
    /// failure via `bulletproofs::r1cs::CheckpointableConstraintSystem::
    /// rollback` — truncates the witness/constraint vectors and
    /// rewinds the Fiat–Shamir transcript so any allocations and
    /// constraints the failed callee made are dropped. `None` in
    /// internal context (no real CS).
    pub(crate) snap_cs: Option<r1cs::Checkpoint>,
}

impl CallFrame {
    /// Test/compatibility entry: compiles source instructions to Cell code,
    /// retaining their private witness overlay.
    #[cfg(test)]
    pub fn new(instructions: Vec<Instruction>, kind: CallKind, gas_limit: u64) -> Self {
        Self::from_code(Script::Transparent(instructions), kind, gas_limit)
    }

    /// Compatibility source-bytecode entry. Production execution operands use Cells.
    pub(crate) fn from_bytecode(bytecode: Vec<u8>, kind: CallKind, gas_limit: u64) -> Self {
        Self::from_code(Script::Opaque(bytecode), kind, gas_limit)
    }

    /// Sets this frame's starting anchor (todo #4). Chained at the
    /// internal-root / call / contract-open construction sites.
    pub(crate) fn with_anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = Some(anchor);
        self
    }

    pub(crate) fn from_code(code: Script, kind: CallKind, gas_limit: u64) -> Self {
        Self {
            stack: Vec::new(),
            actor_cells: Arc::new(CellIndex::new()),
            code: Some(code),
            program: None,
            entry_program: None,
            private_scripts: BTreeMap::new(),
            instructions: None,
            instruction_cursor: 0,
            cursor: 0,
            labels: Vec::new(),
            kind,
            anchor: None,
            gas_limit,
            gas_used: 0,
            post_call_anchor: None,
            snap_txlog_len: 0,
            snap_deferred_sigs_len: 0,
            snap_deferred_multiplications: 0,
            snap_total_fee: CheckedFee::zero(),
            snap_failure_values: Vec::new(),
            snap_batch: None,
            snap_cs: None,
        }
    }

    /// Enters authenticated program Cells without flattening their bytecode.
    pub(crate) fn from_cell(root: Cell, kind: CallKind, gas_limit: u64) -> Result<Self, VMError> {
        let program = Program::new(root.clone())?;
        let mut frame = Self::from_bytecode(Vec::new(), kind, gas_limit);
        frame.code = None;
        frame.entry_program = Some(root);
        frame.program = Some(program);
        Ok(frame)
    }

    fn prepare_program(&mut self) -> Result<(), VMError> {
        if self.program.is_none() {
            let code = self.code.as_ref().expect("unprepared frame has source");
            let root = match code {
                Script::Transparent(instructions) => {
                    let (root, scripts) = crate::script::compile_program(instructions)?;
                    self.private_scripts = scripts;
                    root
                }
                _ => code.to_cell()?,
            };
            self.instructions = self.private_scripts.get(&root.id()).cloned();
            let program = Program::new(root.clone())?;
            self.entry_program = Some(root);
            self.code = None;
            self.program = Some(program);
        }
        Ok(())
    }
}

/// Outcome of a successful transaction execution. Returned by both
/// `Prover::prove` and `Verifier::verify`.
pub struct TxResult {
    /// Canonical 32-byte transaction id.
    pub txid: TxID,

    /// Full txlog including the `Header` entry at index 0.
    pub txlog: Vec<TxEntry>,

    /// Aggregate fee in flames recorded by `op_fee`.
    pub total_fee: u64,

    /// Gas spent by instructions, logical allocation, scheduled finalization,
    /// and asynchronous message grants.
    pub gas_used: u64,

    /// Exact number of multiplication gates in the final constraint system.
    pub multiplications: usize,

    /// Compatibility byte-only source when representable, otherwise root
    /// payload only. Never a standalone native graph; verify using `program`.
    pub bytecode: Vec<u8>,
    /// Executed root program, with literal Cell operands and explicit continuations.
    pub program: Cell,

    /// The immutable lookup snapshot committed by the external transaction.
    /// Inline program bodies are also available; use ExternalTx::execution_cells
    /// when delivering induced actor transactions.
    pub cells: Arc<CellIndex>,

    /// R1CS proof. `Some` on the prover side, `None` on the verifier
    /// side (the verifier consumed it during `cs.verify`).
    pub proof: Option<R1CSProof>,

    /// Deferred-signature records — `Explicit` items (already
    /// batch-verified for the verifier; surfaced for caller
    /// inspection) and `TxBound` items (the caller used these to
    /// build the aggregate multi-signature; verifier sees them
    /// after the fact for the same audit shape).
    pub deferred_sigs: Vec<DeferredSig>,
}

/// Manual Debug — the linear `Contract` in `txlog` blocks `#[derive]`.
impl core::fmt::Debug for TxResult {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TxResult")
            .field("txid", &self.txid)
            .field("txlog.len", &self.txlog.len())
            .field("total_fee", &self.total_fee)
            .field("gas_used", &self.gas_used)
            .field("multiplications", &self.multiplications)
            .field("bytecode.len", &self.bytecode.len())
            .field("proof_present", &self.proof.is_some())
            .field("deferred_sigs.len", &self.deferred_sigs.len())
            .finish()
    }
}

pub(crate) struct VM {
    header: TxHeader,
    block_height: u64,
    cells: Arc<CellIndex>,
    /// Serialized lookup snapshot, separate from the inline program bodies.
    witnesses: Arc<CellIndex>,
    program_scope_ready: bool,
    contract_witnesses: BTreeMap<ContractID, Contract>,
    script_witnesses: BTreeMap<CellID, Vec<Instruction>>,
    value_witnesses: BTreeMap<CellID, Value>,

    /// Per-tx current anchor. `None` for fresh ExternalRoot txs (the
    /// first `op_input` seeds it); `Some(M)` at the start of an
    /// internal tx (where `M` is the delivering Message's anchor —
    /// itself a split-child from the originating tx's `op_send`).
    /// Consumed-and-replaced via split by `contract` / `output` / `send`;
    /// `call` / `open` / `signcall` split it into disjoint callee and
    /// caller-continuation subtrees.
    pub(crate) last_anchor: Option<Anchor>,

    current_call: CallFrame,
    call_stack: Vec<CallFrame>,

    /// Pricing history for this execution, not a source of available Cell bodies.
    resolved_cells: BTreeSet<CellID>,

    /// Effects committed by the external body or the derived internal ID.
    pub(crate) txlog: Vec<TxEntry>,

    /// Running per-tx fee accumulator (overflow → `FeeTooHigh`).
    total_fee: CheckedFee,

    /// Signature checks deferred to `Delegate::finalize`.
    deferred_sigs: Vec<DeferredSig>,

    /// Multiplications added by deferred randomized constraints. Ordinary
    /// gates are read from the finalized constraint system by the delegate.
    deferred_multiplications: usize,
    #[cfg(feature = "debugger")]
    debug_error: Option<std::string::String>,
}

/// Logical cold/warm pricing is independent of host residency. Warmth never
/// authorizes a body absent from the current actor/transaction scope.
struct ExecutionCells<'a> {
    external: &'a CellIndex,
    actor: &'a CellIndex,
    // Explicit address witnesses can use sender storage; callee execution cannot.
    address_sender: Option<&'a CellIndex>,
    resolved_cells: &'a mut BTreeSet<CellID>,
    gas_used: &'a mut u64,
    gas_limit: u64,
}

impl ExecutionCells<'_> {
    fn charge(&mut self, gas: u64) -> Result<(), CellError> {
        *self.gas_used = self
            .gas_used
            .checked_add(gas)
            .ok_or(CellError::ResourceExhausted)?;
        if *self.gas_used > self.gas_limit {
            return Err(CellError::ResourceExhausted);
        }
        Ok(())
    }
}

impl CellReader for ExecutionCells<'_> {
    fn read(&mut self, id: CellID, resident: Option<&Arc<Cell>>) -> Result<Arc<Cell>, CellError> {
        self.charge(GAS_CELL_READ)?;
        let cell = match resident {
            Some(cell) => Arc::clone(cell),
            None => self
                .actor
                .get(&id)
                .or_else(|| self.address_sender.and_then(|sender| sender.get(&id)))
                .or_else(|| self.external.get(&id))
                .ok_or(CellError::MissingCell(id))?,
        };
        if cell.id() != id {
            return Err(CellError::CellHashMismatch {
                expected: id,
                actual: cell.id(),
            });
        }
        let body_gas = (cell.record_size() as u64).saturating_add(cell.refs().len() as u64);
        let warm = self.resolved_cells.contains(&id);
        self.charge(if warm {
            (body_gas + GAS_CELL_WARM_DIVISOR - 1) / GAS_CELL_WARM_DIVISOR
        } else {
            body_gas
        })?;
        if !warm {
            self.resolved_cells.insert(id);
        }
        Ok(cell)
    }
}

fn code_materialization_gas(cell: &Cell) -> u64 {
    // Charge copies and hashes at every level even when existing bodies can be reused.
    (cell.record_size() as u64)
        .saturating_mul(2 * GAS_PER_ALLOC_BYTE)
        .saturating_mul(u64::from(cell.level_mask().count_ones()) + 1)
}

struct CodeCells<'a>(ExecutionCells<'a>);

impl CellReader for CodeCells<'_> {
    fn read(&mut self, id: CellID, resident: Option<&Arc<Cell>>) -> Result<Arc<Cell>, CellError> {
        let cell = self.0.read(id, resident)?;
        self.0.charge(code_materialization_gas(&cell))?;
        Ok(cell)
    }
}

/// Called only under the transaction/call checkpoint, before actor execution.
fn prepare_actor_address(
    registry: &mut dyn ActorRegistry,
    address: &ActorAddress,
    external: &CellIndex,
    sender: Option<&CellIndex>,
    resolved_cells: &mut BTreeSet<CellID>,
    gas_used: &mut u64,
    gas_limit: u64,
) -> Result<Option<TxEntry>, VMError> {
    let actor = address.to_actor_id();
    let exists = registry.exists(&actor);
    let actor_cells = if exists {
        registry.actor_cells(&actor)?
    } else {
        Arc::new(CellIndex::new())
    };
    let mut reader = CodeCells(ExecutionCells {
        external,
        actor: &actor_cells,
        address_sender: sender,
        resolved_cells,
        gas_used,
        gas_limit,
    });
    match address {
        ActorAddress::Init { ctor } if !exists => {
            let root = cells::read_cell(&mut reader, ctor)?;
            let code = root.load_graph_scoped(&mut reader)?;
            Program::new(Arc::new(code.clone()))?;
            registry.deploy(actor, code.clone(), empty_state())?;
            Ok(Some(TxEntry::ActorDeploy { actor, code }))
        }
        ActorAddress::Restore { state, .. } if exists => {
            reader.0.charge(actor_cells.len() as u64)?;
            let root = cells::read_cell(&mut reader, state)?;
            let proof = root.load_graph_scoped(&mut reader)?;
            registry.restore_with_cells(&actor, &proof.clone().into(), &mut reader)?;
            Ok(Some(TxEntry::ActorRestore {
                actor,
                state: proof,
            }))
        }
        _ if !exists => Err(VMError::ActorNotFound),
        _ if registry.is_frozen(&actor)? => Err(VMError::ActorFrozen),
        _ => Ok(None),
    }
}

impl VM {
    fn resolver(&mut self) -> ExecutionCells<'_> {
        ExecutionCells {
            external: &self.cells,
            actor: &self.current_call.actor_cells,
            address_sender: None,
            resolved_cells: &mut self.resolved_cells,
            gas_limit: self.current_call.gas_limit,
            gas_used: &mut self.current_call.gas_used,
        }
    }

    fn with_cells(mut self, cells: Arc<CellIndex>) -> Result<Self, CellError> {
        if matches!(self.current_call.kind, CallKind::ExternalRoot) {
            // Reject unencodable program data before executing private witnesses.
            cells.to_cell()?;
        }
        self.witnesses = Arc::clone(&cells);
        self.cells = cells;
        Ok(self)
    }
    /// Executes an external transaction script with the given delegate,
    /// then calls `delegate.finalize`. Crate-internal: the public path
    /// is `ScriptBuilder::build_tx` / `ExternalTx::verify`.
    #[cfg(test)]
    pub(crate) fn execute_external<D: Delegate>(
        header: TxHeader,
        script: Vec<u8>,
        gas_limit: u64,
        mut delegate: D,
    ) -> Result<TxResult, VMError> {
        let mut frame = CallFrame::from_bytecode(script.clone(), CallKind::ExternalRoot, gas_limit);
        frame.charge_gas(alloc_byte_gas(script.len())?)?;
        frame.charge_gas(GAS_EXTERNAL_FINALIZE_BASE)?;
        let mut vm = Self::new(header, frame);
        while vm.step_external(&mut delegate)? {}
        vm.into_result(script, None)
    }

    /// Runs an external-root program through the VM without finalizing
    /// the delegate. Used by `Prover` / `Verifier` which take a ScriptBuilder
    /// (with witnesses inline on the prover side).
    pub(crate) fn run<D: Delegate>(
        header: TxHeader,
        program: ScriptBuilder,
        gas_limit: u64,
        delegate: &mut D,
    ) -> Result<TxResult, VMError> {
        let mut vm = Self::prepare_external(header, program, gas_limit)?;
        while vm.step_external(delegate)? {}
        vm.into_result(Vec::new(), None)
    }

    fn prepare_external(
        header: TxHeader,
        program: ScriptBuilder,
        gas_limit: u64,
    ) -> Result<Self, VMError> {
        // Freeze public availability before any private witness is executed.
        let cells = Arc::new(program.cell_witnesses()?);
        let contract_witnesses = program.contract_witnesses();
        let script_witnesses = program.script_witnesses()?;
        let (root, private_scripts) = crate::script::compile_program(program.instructions())?;
        let bytecode = root.payload();
        let initial_gas =
            alloc_byte_gas(bytecode.len())?.saturating_add(GAS_EXTERNAL_FINALIZE_BASE);
        if initial_gas > gas_limit {
            return Err(VMError::OutOfGas);
        }
        let mut frame = CallFrame::from_cell(root, CallKind::ExternalRoot, gas_limit)?;
        frame.instructions = private_scripts
            .get(&frame.entry_program.as_ref().unwrap().id())
            .cloned();
        frame.private_scripts = private_scripts;
        frame.charge_gas(initial_gas)?;
        let mut vm = Self::new(header, frame).with_cells(cells)?;
        vm.contract_witnesses = contract_witnesses;
        vm.script_witnesses = script_witnesses;
        Ok(vm)
    }

    /// Verifier entry: fetches instructions from Cell payloads on demand.
    pub(crate) fn run_program_with_cells<D: Delegate>(
        header: TxHeader,
        program: Cell,
        gas_limit: u64,
        delegate: &mut D,
        cells: &CellIndex,
    ) -> Result<TxResult, VMError> {
        let bytecode = program.payload().to_vec();
        let initial_gas =
            alloc_byte_gas(bytecode.len())?.saturating_add(GAS_EXTERNAL_FINALIZE_BASE);
        if initial_gas > gas_limit {
            return Err(VMError::OutOfGas);
        }
        let mut frame = CallFrame::from_cell(program, CallKind::ExternalRoot, gas_limit)?;
        frame.charge_gas(initial_gas)?;
        let mut vm = Self::new(header, frame).with_cells(Arc::new(cells.clone()))?;
        while vm.step_external(delegate)? {}
        vm.into_result(bytecode, None)
    }

    /// Executes an internal transaction. On clean exit runs the tx-end
    /// self-destruct sweep against `registry`.
    #[cfg(test)]
    pub fn execute_internal(
        header: TxHeader,
        message: Message,
        registry: &mut dyn ActorRegistry,
        block: &BlockContext,
    ) -> Result<TxResult, VMError> {
        Self::execute_internal_with_cells(
            header,
            message,
            registry,
            block,
            Arc::new(CellIndex::new()),
        )
    }

    pub fn execute_internal_with_cells(
        header: TxHeader,
        message: Message,
        registry: &mut dyn ActorRegistry,
        block: &BlockContext,
        cells: Arc<CellIndex>,
    ) -> Result<TxResult, VMError> {
        // Message admission already paid for public encoding. Start every delivery
        // from that public form, never native maps, private openings or warm caches.
        // Trusted decoding keeps authenticated Dict branches lazy.
        let payload_cell = message.payload().to_cell()?;
        let payload = Value::from_trusted_cell(&payload_cell, &mut ())?;
        let mut initial_gas = payload
            .encoding_gas()?
            .checked_mul(4)
            .and_then(|gas| gas.checked_add(payload.clone_gas().saturating_mul(2)))
            .ok_or(VMError::OutOfGas)?;
        if initial_gas > message.gas {
            return Err(VMError::OutOfGas);
        }
        let message_cell = message.to_cell()?;
        initial_gas = initial_gas
            .saturating_add(alloc_byte_gas(message_cell.record_size())?)
            .saturating_add(alloc_item_gas(1)?);
        if initial_gas > message.gas {
            return Err(VMError::OutOfGas);
        }

        let mut resolved_cells = BTreeSet::new();
        registry.push_checkpoint();
        let result = (|| {
            let target = message.target.to_actor_id();
            let address_effect = prepare_actor_address(
                registry,
                &message.target,
                &cells,
                None,
                &mut resolved_cells,
                &mut initial_gas,
                message.gas,
            )?;
            initial_gas = initial_gas.saturating_add(
                registry
                    .actor_code_bytes(&target)?
                    .saturating_mul(GAS_PER_ALLOC_BYTE),
            );
            if initial_gas > message.gas {
                return Err(VMError::OutOfGas);
            }
            let actor_cells = registry.actor_cells(&target)?;
            let mut reader = ExecutionCells {
                external: &cells,
                actor: &actor_cells,
                address_sender: None,
                resolved_cells: &mut resolved_cells,
                gas_used: &mut initial_gas,
                gas_limit: message.gas,
            };
            let script = registry.load_program_with_cells(&target, &mut reader)?;
            let kind = CallKind::InternalRoot {
                actor: target,
                caller: message.caller,
            };
            let mut frame =
                CallFrame::from_cell(script, kind, message.gas)?.with_anchor(message.anchor);
            frame.actor_cells = actor_cells;
            frame.charge_gas(initial_gas)?;
            frame.stack.push(payload);
            let mut vm = Self::new(header, frame).with_cells(cells)?;
            vm.resolved_cells = resolved_cells;
            vm.block_height = block.height;
            vm.txlog.push(TxEntry::Receive(message_cell.id()));
            if let Some(effect) = address_effect {
                vm.txlog.push(effect);
            }

            while vm.step_internal_with_registry(registry)? {}
            for actor in registry.commit_tx_destructions() {
                vm.txlog.push(TxEntry::ActorDestroy { actor });
            }
            if registry.exists(&target) {
                registry.validate_actor_storage(&target, block.height)?;
            }
            vm.into_result(Vec::new(), None)
        })();
        match result {
            Ok(result) => {
                registry.pop_checkpoint_commit();
                Ok(result)
            }
            Err(error) => {
                registry.pop_checkpoint_rollback();
                Err(match error {
                    VMError::Cell(CellError::ResourceExhausted) => VMError::OutOfGas,
                    other => other,
                })
            }
        }
    }

    fn new(header: TxHeader, initial_call: CallFrame) -> Self {
        // Header is the first txlog entry so TxID binds to version + locktime.
        let cells = Arc::new(CellIndex::new());
        let txlog = vec![TxEntry::Header(header)];
        // Seed last_anchor from the root frame's kind: ExternalRoot →
        // None (op_input must seed); InternalRoot → Some(Message.anchor)
        // (already unique from prior tx's op_send split).
        let last_anchor = initial_call.anchor;
        Self {
            header,
            block_height: 0,
            witnesses: Arc::clone(&cells),
            cells,
            program_scope_ready: false,
            contract_witnesses: BTreeMap::new(),
            script_witnesses: BTreeMap::new(),
            value_witnesses: BTreeMap::new(),
            last_anchor,
            current_call: initial_call,
            call_stack: Vec::new(),
            resolved_cells: BTreeSet::new(),
            txlog,
            total_fee: CheckedFee::zero(),
            deferred_sigs: Vec::new(),
            deferred_multiplications: 0,
            #[cfg(feature = "debugger")]
            debug_error: None,
        }
    }

    /// Splits `last_anchor`: returns the `left` half (to embed in
    /// a fresh unique-anchored entity — contract, message), and writes
    /// the `right` half back. Hard-fails `AnchorMissing` if no
    /// anchor has been claimed yet.
    fn consume_anchor(&mut self) -> Result<Anchor, VMError> {
        let parent = self.last_anchor.ok_or(VMError::AnchorMissing)?;
        let (left, right) = parent.split();
        self.last_anchor = Some(right);
        Ok(left)
    }

    /// Splits the current anchor for a call entry: returns the
    /// `left` half (becomes the callee's starting `last_anchor`) and
    /// stores the `right` half in the parent frame's
    /// `post_call_anchor` slot — so when control returns to the
    /// parent (success *or* failure), the parent's anchor chain
    /// resumes deterministically from `right`. Also stashes the
    /// snapshots needed to roll back side effects on failure.
    /// `Anchor` flows uninitialized → `AnchorMissing`.
    fn split_anchor_for_call(&mut self) -> Result<Anchor, VMError> {
        let parent_anchor = self.last_anchor.ok_or(VMError::AnchorMissing)?;
        let (left, right) = parent_anchor.split();
        self.current_call.post_call_anchor = Some(right);
        // Snapshot effect counters for failure rollback.
        self.current_call.snap_txlog_len = self.txlog.len();
        self.current_call.snap_deferred_sigs_len = self.deferred_sigs.len();
        self.current_call.snap_deferred_multiplications = self.deferred_multiplications;
        self.current_call.snap_total_fee = self.total_fee;
        Ok(left)
    }

    /// External IDs bind the body; derived internal IDs retain the effect root.
    fn into_result(
        mut self,
        bytecode: Vec<u8>,
        proof: Option<R1CSProof>,
    ) -> Result<TxResult, VMError> {
        let txlog = mem::take(&mut self.txlog);
        let deferred_sigs = mem::take(&mut self.deferred_sigs);
        self.current_call.prepare_program()?;
        let program = self.current_call.entry_program.as_ref().unwrap().clone();
        let txid = if matches!(self.current_call.kind, CallKind::ExternalRoot) {
            TxID(
                crate::tx::external_program_body(self.header, &program, &self.witnesses, &txlog)?
                    .id(),
            )
        } else {
            TxID::from_log(&txlog)
        };
        let bytecode = if matches!(self.current_call.kind, CallKind::ExternalRoot) {
            crate::code_from_cell(&program, &mut (), usize::MAX)
                .unwrap_or_else(|_| program.payload().to_vec())
        } else {
            bytecode
        };
        Ok(TxResult {
            txid,
            txlog,
            total_fee: self.total_fee.total(),
            // Root frame's instruction and allocation gas (spec §gas).
            gas_used: self.current_call.gas_used,
            multiplications: self.deferred_multiplications,
            bytecode,
            program,
            cells: self.witnesses,
            proof,
            deferred_sigs,
        })
    }

    /// Gate for external-only opcodes (CS-touching handlers).
    fn require_external(&self) -> Result<(), VMError> {
        if self.is_external() {
            Ok(())
        } else {
            Err(VMError::ExternalOnly)
        }
    }

    /// True iff CS-touching opcodes are permitted. `ExternalRoot` is
    /// always external; `ContractOpen` inherits the caller's snapshot.
    fn is_external(&self) -> bool {
        match self.current_call.kind {
            CallKind::ExternalRoot => true,
            CallKind::ContractOpen {
                external_context, ..
            } => external_context,
            _ => false,
        }
    }

    /// External-context step.
    pub(crate) fn step_external<D: Delegate>(&mut self, delegate: &mut D) -> Result<bool, VMError> {
        self.step(delegate, None)
    }

    /// Internal-context step without a registry — registry-touching
    /// opcodes error `RegistryUnavailable`. Test-only.
    #[cfg(test)]
    pub(crate) fn step_internal(&mut self) -> Result<bool, VMError> {
        let mut stub = InternalDelegate;
        self.step(&mut stub, None)
    }

    /// Internal-context step with a live registry handle.
    pub(crate) fn step_internal_with_registry(
        &mut self,
        registry: &mut dyn ActorRegistry,
    ) -> Result<bool, VMError> {
        let mut stub = InternalDelegate;
        self.step(&mut stub, Some(registry))
    }

    /// Executes one instruction. Returns `Ok(true)` to continue,
    /// `Ok(false)` to stop. Errors that occur inside a nested
    /// call frame are caught — the frame is unwound via
    /// `fail_current_call` and its entry escrow plus status `0` is
    /// pushed onto the parent's stack. Errors at the outermost frame
    /// propagate to the caller (kill the tx).
    fn step<D: Delegate>(
        &mut self,
        delegate: &mut D,
        registry: Option<&mut dyn ActorRegistry>,
    ) -> Result<bool, VMError> {
        // Depth before the step. If the step pushes a frame
        // (`call`/`open`/`signcall` succeeded in entering), depth
        // grows by 1 and we snapshot the delegate's batch on the
        // *parent* frame — so a future failure can restore the
        // pre-call batch state. Snapshots only happen in external
        // context (`InternalDelegate::batch_verifier` panics, so we
        // can't query it in internal mode — see `is_external`).
        #[cfg(feature = "debugger")]
        {
            self.debug_error = None;
        }
        let depth_before = self.call_stack.len();
        // Reborrow for step_inner, then hand the original to post_step
        // (one borrow live at a time — borrow checker requires the split).
        let (result, registry) = match registry {
            None => (self.step_inner(delegate, None), None),
            Some(r) => {
                let prepared = if let Some(actor) = self.current_call.kind.actor() {
                    r.actor_cells(actor)
                        .map(|cells| self.current_call.actor_cells = cells)
                } else {
                    Ok(())
                };
                let result = prepared.and_then(|()| self.step_inner(delegate, Some(&mut *r)));
                (result, Some(r))
            }
        };
        self.post_step(delegate, registry, depth_before, result)
    }

    /// Shared step post-processing: snapshot/checkpoint on frame entry,
    /// checkpoint-commit on clean exit, fail_current_call on error.
    fn post_step<D: Delegate>(
        &mut self,
        delegate: &mut D,
        mut registry: Option<&mut dyn ActorRegistry>,
        depth_before: usize,
        result: Result<bool, VMError>,
    ) -> Result<bool, VMError> {
        match result {
            Ok(cont) => {
                let depth_after = self.call_stack.len();
                if depth_after > depth_before {
                    self.snapshot_parent_on_push(delegate);
                    // Open an undo frame so a future failure of the
                    // newly-pushed frame can roll back any `op_save`/
                    // `op_load` state moves (a failed `load` restores
                    // the checked-out state, so the actor isn't
                    // spuriously self-destructed) — closes F1 of the
                    // op_save audit.
                    // ActorCall already checkpoints its address resolution.
                    if !matches!(self.current_call.kind, CallKind::ActorCall { .. }) {
                        if let Some(r) = registry.as_mut() {
                            r.push_checkpoint();
                        }
                    }
                } else if depth_after < depth_before {
                    // Clean exit: keep effects, drop the snapshot.
                    if let Some(r) = registry.as_mut() {
                        r.pop_checkpoint_commit();
                    }
                }
                Ok(cont)
            }
            Err(e) => {
                let e = match e {
                    VMError::Cell(CellError::ResourceExhausted) => VMError::OutOfGas,
                    other => other,
                };
                if matches!(e, VMError::OutOfGas) {
                    self.current_call.gas_used = self.current_call.gas_limit;
                }
                #[cfg(feature = "debugger")]
                {
                    self.debug_error = Some(e.to_string());
                }
                if self.call_stack.is_empty() {
                    return Err(e);
                }
                self.fail_current_call(delegate, registry);
                Ok(true)
            }
        }
    }

    /// On call entry (depth grew), snapshot the delegate's batch + CS
    /// onto the parent frame so a callee failure can roll them back.
    /// No-op outside external context (internal delegates have no batch).
    fn snapshot_parent_on_push<D: Delegate>(&mut self, delegate: &mut D) {
        if !self.is_external() {
            return;
        }
        use musig::BatchCheckpoint;
        use r1cs::CheckpointableConstraintSystem;
        let batch_snap = delegate.batch_verifier().snapshot();
        let cs_snap = delegate.cs().checkpoint();
        if let Some(parent) = self.call_stack.last_mut() {
            parent.snap_batch = Some(batch_snap);
            parent.snap_cs = Some(cs_snap);
        }
    }

    /// Single-step body, separated from `step` so the error path
    /// can be uniformly caught.
    fn step_inner<D: Delegate>(
        &mut self,
        delegate: &mut D,
        registry: Option<&mut dyn ActorRegistry>,
    ) -> Result<bool, VMError> {
        self.current_call.charge_gas(GAS_PER_INSTRUCTION)?;
        if !self.program_scope_ready && matches!(self.current_call.kind, CallKind::ExternalRoot) {
            self.current_call.prepare_program()?;
            let inline = CellIndex::collect(Arc::new(
                self.current_call.entry_program.as_ref().unwrap().clone(),
            ))?;
            let mut available = self.witnesses.as_ref().clone();
            available.extend(&inline)?;
            self.cells = Arc::new(available);
            self.program_scope_ready = true;
        }
        let Some(instr) = self.current_call.next_instruction()? else {
            return self.finish_call(registry);
        };
        use Instruction as I;
        match instr {
            I::PushInt(i) => {
                self.push_value(Value::Scalar(i));
                Ok(())
            }
            I::CellLiteral(literal) => {
                let reference = self.current_call.program.as_mut().unwrap().load_ref()?;
                if literal.id() != reference.id() {
                    return Err(VMError::TaprootProofMismatch);
                }
                let witness = literal.witness.clone();
                self.push_value(Value::Cell(crate::CellValue { reference, witness }));
                Ok(())
            }
            I::PushPoint(p) => {
                // Witness-bearing variants flow through unchanged;
                // verifier-side decoded form is `Point::Opaque`.
                self.push_value(Value::Point(p));
                Ok(())
            }
            I::PushToken => self.op_pushtoken(),
            I::PushCell(literal) => {
                let reference = self.current_call.program.as_mut().unwrap().load_ref()?;
                let witness = if let Some(literal) = literal {
                    if literal.id() != reference.id() {
                        return Err(VMError::TaprootProofMismatch);
                    }
                    literal.witness
                } else {
                    None
                };
                self.push_value(Value::Cell(crate::CellValue { reference, witness }));
                Ok(())
            }
            I::Exec => self.op_exec(),
            I::Drop => self.op_drop(),
            I::Nop => self.op_nop(),
            I::Dup => self.op_dup(),
            I::Roll => self.op_roll(),
            I::DupK(k) => self.op_dup_k(k as usize),
            I::RollK(k) => self.op_roll_k(k as usize),

            I::Builder => {
                self.push_value(Value::Builder(CellBuilder::new()));
                Ok(())
            }
            I::Slice => self.op_slice(),
            I::EndCell => self.op_endcell(),
            I::LoadRef => self.op_load_ref(),
            I::CellInfo => self.op_cell_info(),
            I::RunSlice => {
                let slice = self
                    .current_call
                    .program
                    .as_ref()
                    .unwrap()
                    .slice(self.current_call.cursor)?;
                self.push_value(Value::Slice(slice));
                Ok(())
            }
            I::ReadUint => {
                let n = self.pop_byte_count(32)?;
                self.op_read_uint(n)
            }
            I::ReadScalar => self.op_read_uint(32),
            I::ReadBytes => self.op_read_bytes(),
            I::ReadPoint => self.op_read_point(),
            I::WriteUint => {
                let n = self.pop_byte_count(32)?;
                self.op_write_uint(n)
            }
            I::WriteScalar => self.op_write_uint(32),
            I::WritePoint => self.op_write_point(),
            I::WriteRef => self.op_write_ref(),
            I::AppendBytes => self.op_append_bytes(),
            I::AppendRefs => self.op_append_refs(),
            I::WriteZeros => self.op_write_zeros(),
            I::BitNot => self.op_bit_not(),
            I::BitOr => self.op_bitwise(|a, b| a | b),
            I::BitAnd => self.op_bitwise(|a, b| a & b),
            I::BitXor => self.op_bitwise(|a, b| a ^ b),
            I::ShiftLeft => self.op_shift(true),
            I::ShiftRight => self.op_shift(false),
            I::Keccak256 => self.op_hash::<sha3::Keccak256>(136),

            I::Abs => self.op_abs(),
            I::Eq => self.op_eq(delegate),
            I::Neg => self.op_neg(delegate),
            I::Add => self.op_add(delegate),
            I::Mul => self.op_mul(delegate),
            I::DivMod => self.op_divmod(),
            I::Mod252 => self.op_mod252(),
            I::Not => self.op_not(delegate),
            I::And => self.op_and(delegate),
            I::Or => self.op_or(delegate),
            I::Size => self.op_size(),

            I::Dict => self.op_dict(),
            I::Put => self.op_put(),
            I::Replace => self.op_replace(),
            I::Get => self.op_get(),
            I::GetOpt => self.op_getopt(),
            I::GetDup => self.op_getdup(),
            I::First => self.op_first(),
            I::Last => self.op_last(),
            I::Next => self.op_next(),

            I::Transcript => self.op_transcript(),
            I::TWrite => self.op_twrite(),
            I::TRead => self.op_tread(),
            I::Sha256 => self.op_hash::<sha2::Sha256>(64),
            I::Sha512 => self.op_hash::<sha2::Sha512>(128),
            I::Sha3 => self.op_hash::<sha3::Sha3_256>(136),
            I::CellHash => self.op_cell_hash(),
            I::EvalPoint => self.op_eval_point(),
            I::Log => self.op_log(),

            I::Amount => self.op_amount(),
            I::IssuePriv => self.op_issuepriv(delegate),
            I::IssuePub => self.op_issuepub(),
            I::Retire => self.op_retire(),
            I::Borrow => self.op_borrow(delegate),
            I::Merge => self.op_merge(),
            I::Split => self.op_split(),
            I::IssuePubFlv => self.op_issuepubflv(),
            I::IssuePrivFlv => self.op_issueprivflv(),

            I::Alloc(w) => self.op_alloc(w, delegate),
            I::Expr => self.op_expr(delegate),
            I::Range => self.op_range(delegate),
            I::Commit => self.op_commit(),
            I::Decrypt => self.op_decrypt(delegate),
            I::Mix => self.op_mix(delegate),
            I::Fee => self.op_fee(delegate),
            I::Verify => self.op_verify(delegate),

            I::Label(n) => self.op_label(n),
            I::Jump(n) => self.op_jump(n),
            I::JumpIf(n) => self.op_jumpif(n),
            I::Return => self.op_return(registry),
            I::Type => self.op_type(),

            I::Input => self.op_input(),
            I::Contract => self.op_contract(),
            I::Output => self.op_output(),
            I::Open => self.op_open(),
            I::Send => self.op_send(),
            I::Call => self.op_call(registry),
            I::Load => self.op_load(registry),
            I::Save => self.op_save(registry),
            I::Setcode => self.op_setcode(registry),
            I::AddStorage => self.op_addstorage(registry),
            I::QuoteStorage => self.op_quotestorage(registry),
            I::Signtx => self.op_signtx(),
            I::Signcall => self.op_signcall(),

            I::Timelock => self.op_timelock(),
            // version → tx header version (spec §version).
            I::Version => {
                self.push_value(Value::Scalar(Scalar::from(self.header.version as u64)));
                Ok(())
            }
            I::Selfid => self.op_selfid(),
            I::Anchor => self.op_anchor(),
            I::Gas => self.op_gas(),
            I::Usage => self.op_usage(registry),
            I::Callerid => self.op_callerid(),
            // gaslimit → immutable frame budget.
            I::Gaslimit => {
                self.push_value(Value::Scalar(Scalar::from(self.current_call.gas_limit)));
                Ok(())
            }
            I::Capacity => self.op_capacity(registry),
            I::Height => {
                self.push_value(Value::Scalar(Scalar::from(self.block_height)));
                Ok(())
            }

            I::Ext(b) => Err(VMError::UnknownOpcode(b)),
        }?;
        Ok(true)
    }

    /// Pops the current frame back to its caller on clean exit. Stack
    /// must be empty (use `value return` to send a result across the boundary).
    /// Leftover gas is refunded to the parent. Implicit clean exits
    /// push `{0, 1}` onto the parent's stack (unit result and success).
    /// Shared clean-exit epilogue: refund leftover gas to the parent
    /// (already swapped in by the caller), apply the parent's post-call
    /// anchor, and discard the entry-time batch/CS snapshots
    /// (failure-path only). Order is load-bearing — identical in
    /// `finish_call` and `op_return`.
    fn clean_exit_to_parent(&mut self, leftover_gas: u64) {
        self.current_call.gas_used = self.current_call.gas_used.saturating_sub(leftover_gas);
        if let Some(post) = self.current_call.post_call_anchor.take() {
            self.last_anchor = Some(post);
        }
        self.current_call.snap_batch = None;
        self.current_call.snap_cs = None;
        self.current_call.snap_failure_values.clear();
    }

    fn validate_actor_call_storage(
        &self,
        registry: Option<&dyn ActorRegistry>,
    ) -> Result<(), VMError> {
        let CallKind::ActorCall { actor, .. } = &self.current_call.kind else {
            return Ok(());
        };
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        match registry.actor_code_bytes(actor) {
            // Checked-out state is dismantled at transaction completion.
            Err(VMError::ActorEmpty) => Ok(()),
            Ok(_) => registry.validate_actor_storage(actor, self.block_height),
            Err(error) => Err(error),
        }
    }

    fn finish_call(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<bool, VMError> {
        if !self.current_call.stack.is_empty() {
            return Err(VMError::StackNotClean);
        }
        self.validate_actor_call_storage(registry.as_deref())?;
        let leftover_gas = self
            .current_call
            .gas_limit
            .saturating_sub(self.current_call.gas_used);

        if let Some(parent) = self.call_stack.pop() {
            self.current_call = parent;
            self.clean_exit_to_parent(leftover_gas);
            // An implicit return produces the unit Scalar and success marker.
            self.current_call
                .stack
                .push(Value::Scalar(Scalar::from(0u64)));
            self.current_call.stack.push(Value::Scalar(Scalar::ONE));
            return Ok(true);
        }
        // Outermost call returned: entire tx complete.
        Ok(false)
    }

    /// Discards the current frame on failure: pops it without
    /// preserving its effects, rolls back side-effects from the
    /// parent's snapshot (txlog tail, deferred-sigs tail, total_fee,
    /// delegate batch state, R1CS constraint system), applies the
    /// parent's `post_call_anchor`, and restores entry-owned values
    /// followed by a zero status. Caller's effects up to the
    /// failed call are preserved; the parent script continues at
    /// the instruction after the call.
    ///
    /// Actor-state rollback is wired via the registry's checkpoint
    /// stack (push on frame entry, restore here). Closes the F1 audit
    /// finding — a failed callee's `op_save`/`op_load` state moves are
    /// undone (a checked-out state is restored to present), so the
    /// registry stays consistent with the truncated txlog.
    fn fail_current_call<D: Delegate>(
        &mut self,
        delegate: &mut D,
        mut registry: Option<&mut dyn ActorRegistry>,
    ) {
        let leftover_gas = self
            .current_call
            .gas_limit
            .saturating_sub(self.current_call.gas_used);
        // Cannot fail the outermost frame — caller of this helper
        // must ensure call_stack is non-empty.
        let parent = self
            .call_stack
            .pop()
            .expect("fail_current_call: outermost frame errors must propagate");
        self.current_call = parent;
        self.current_call.gas_used = self.current_call.gas_used.saturating_sub(leftover_gas);
        let failure_values = mem::take(&mut self.current_call.snap_failure_values);
        // Roll back side effects via the snapshots taken at call
        // entry.
        self.txlog.truncate(self.current_call.snap_txlog_len);
        self.deferred_sigs
            .truncate(self.current_call.snap_deferred_sigs_len);
        self.deferred_multiplications = self.current_call.snap_deferred_multiplications;
        self.total_fee = self.current_call.snap_total_fee;
        // Restore actor registry state (F1).
        if let Some(r) = registry.as_mut() {
            r.pop_checkpoint_rollback();
        }
        // Restore the delegate's MSM/sig batch to its pre-call state
        // so any non-identity statements the failed callee appended
        // are discarded. Only in external context — internal-mode
        // delegates have no real batch (snapshot was never taken).
        if let Some(snap) = self.current_call.snap_batch.take() {
            use musig::BatchCheckpoint;
            delegate.batch_verifier().restore(&snap);
        }
        // Restore the delegate's R1CS state too — drops the
        // witness/constraint vectors and rewinds the Fiat–Shamir
        // transcript so the proof binds only to the parent's CS.
        if let Some(snap) = self.current_call.snap_cs.take() {
            use r1cs::CheckpointableConstraintSystem;
            delegate.cs().rollback(snap);
        }
        // Apply parent's post-call anchor (caller's right half of
        // the entry split — independent of whatever the callee did
        // with its left half).
        if let Some(post) = self.current_call.post_call_anchor.take() {
            self.last_anchor = Some(post);
        }
        self.push_failed_values(failure_values);
    }

    /// Pushes a value onto the current call's stack.
    fn push_value(&mut self, v: Value) {
        self.current_call.stack.push(v);
    }

    /// Restore the argument (and contextual locked Contract for opening), then failure.
    fn push_failed_values(&mut self, values: Vec<Value>) {
        self.current_call.stack.extend(values);
        self.current_call.stack.push(Value::Scalar(Scalar::ZERO));
    }

    /// Charge bounded serialization work before an opcode constructs Cell data.
    /// Copies includes subsequent effect/identity encoding of an admitted value.
    fn charge_value_encoding(&mut self, value: &Value, copies: u64) -> Result<(), VMError> {
        self.current_call.charge_gas(
            value
                .encoding_gas()?
                .checked_mul(copies)
                .ok_or(VMError::OutOfGas)?,
        )
    }

    fn charge_top_value_growth(&mut self, n: usize) -> Result<(), VMError> {
        let len = self.current_call.stack.len();
        if len < n {
            return Err(VMError::StackUnderflow);
        }
        let gas = self.current_call.stack[len - n..]
            .iter()
            .fold(0u64, |gas, value| gas.saturating_add(value.clone_gas()));
        self.current_call.charge_gas(gas)
    }

    /// Pops the top value from the current call's stack.
    fn pop_value(&mut self) -> Result<Value, VMError> {
        self.current_call.stack.pop().ok_or(VMError::StackUnderflow)
    }

    /// Converts a stack-popped `Scalar` index into a `usize`.
    fn scalar_to_stack_index(&self, i: Scalar) -> Result<usize, VMError> {
        let v = i.to_u64().ok_or(VMError::IndexOutOfRange)?;
        let idx = usize::try_from(v).map_err(|_| VMError::IndexOutOfRange)?;
        if idx >= self.current_call.stack.len() {
            return Err(VMError::IndexOutOfRange);
        }
        Ok(idx)
    }

    /// `pushtoken` — `flv → token`. Pops a `Scalar` flavor from
    /// the stack and pushes a zero-qty `ClearToken { qty: 0, flv }`. This
    /// is the canonical "empty bearer of a flavor" used as a starting
    /// point for issuance / borrow flows.
    fn op_pushtoken(&mut self) -> Result<(), VMError> {
        let flv = self.pop_value()?.to_scalar()?;
        self.push_value(Value::ClearToken(ClearToken::new(Scalar::ZERO, flv)));
        Ok(())
    }

    /// `drop` — pops the top and discards it if droppable.
    fn op_drop(&mut self) -> Result<(), VMError> {
        let v = self.pop_value()?;
        if !v.is_droppable() {
            // Put it back — failed drop must not silently consume the value
            // (matters if the caller catches the error and continues).
            self.push_value(v);
            return Err(VMError::TypeNotDroppable);
        }
        Ok(())
    }

    /// `dup` — pops `k`, then copies the now-`k`-th item from top
    /// onto the top.
    fn op_dup(&mut self) -> Result<(), VMError> {
        let k = self.pop_value()?.to_scalar()?;
        self.op_dup_k(self.scalar_to_stack_index(k)?)
    }

    /// `dup:k` — copies `stack[top - k]` onto the top.
    /// Requires the source value to be copyable.
    fn op_dup_k(&mut self, k: usize) -> Result<(), VMError> {
        let len = self.current_call.stack.len();
        if k >= len {
            return Err(VMError::IndexOutOfRange);
        }
        let idx = len - 1 - k;
        if !self.current_call.stack[idx].is_copyable() {
            return Err(VMError::TypeNotCopyable);
        }
        self.current_call
            .charge_gas(self.current_call.stack[idx].clone_gas())?;
        let copy = self.current_call.stack[idx].try_clone()?;
        self.push_value(copy);
        Ok(())
    }

    /// `roll` — pops `k`, then moves the `k`-th item from top to
    /// the top.
    fn op_roll(&mut self) -> Result<(), VMError> {
        let k = self.pop_value()?.to_scalar()?;
        self.op_roll_k(self.scalar_to_stack_index(k)?)
    }

    /// `roll:k` — removes `stack[top - k]` and pushes it
    /// back as the new top. `roll:0` is a no-op.
    fn op_roll_k(&mut self, k: usize) -> Result<(), VMError> {
        let stack = &mut self.current_call.stack;
        if k >= stack.len() {
            return Err(VMError::IndexOutOfRange);
        }
        let idx = stack.len() - 1 - k;
        let v = stack.remove(idx);
        stack.push(v);
        Ok(())
    }

    /// Byte labels and data are zero-ref Cell/Slice/Builder payloads.
    // shortcut: transcript work uses the existing 136-byte hash price; recalibrate with Merlin benchmarks.
    fn op_transcript(&mut self) -> Result<(), VMError> {
        let label = self.pop_byte_source()?;
        self.current_call
            .charge_gas(hash_gas(label.byte_payload()?.len(), 136)?)?;
        self.push_value(Value::Merlin(Merlin::new(label.byte_payload()?)));
        Ok(())
    }

    fn op_twrite(&mut self) -> Result<(), VMError> {
        let data = self.pop_byte_source()?;
        let label = self.pop_byte_source()?;
        let mut merlin = self.pop_value()?.to_merlin()?;
        let length = label
            .byte_payload()?
            .len()
            .checked_add(data.byte_payload()?.len())
            .ok_or(VMError::OutOfGas)?;
        self.current_call.charge_gas(hash_gas(length, 136)?)?;
        merlin.write_bytes(label.byte_payload()?, data.byte_payload()?);
        self.push_value(Value::Merlin(merlin));
        Ok(())
    }

    fn op_tread(&mut self) -> Result<(), VMError> {
        let count = self.pop_byte_count(cells::MAX_CELL_PAYLOAD)?;
        let mut builder = self.pop_value()?.into_builder()?;
        let label = self.pop_byte_source()?;
        let mut merlin = self.pop_value()?.to_merlin()?;
        if count > builder.remaining_bytes() {
            return Err(CellError::PayloadCapacity.into());
        }
        self.current_call.charge_gas(hash_gas(
            label
                .byte_payload()?
                .len()
                .checked_add(count)
                .ok_or(VMError::OutOfGas)?,
            136,
        )?)?;
        self.charge_alloc_bytes(count)?;
        builder.store_zeros(count)?;
        let start = builder.used_bytes() - count;
        merlin.read_into(label.byte_payload()?, &mut builder.payload_mut()[start..]);
        self.push_value(Value::Merlin(merlin));
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    /// Hashes source payload bytes and appends the digest to the provided Builder.
    fn op_hash<H: sha2::Digest>(&mut self, block_bytes: usize) -> Result<(), VMError> {
        let mut builder = self.pop_value()?.into_builder()?;
        let source = self.pop_byte_source()?;
        let length = <H as sha2::Digest>::output_size();
        if length > builder.remaining_bytes() {
            return Err(CellError::PayloadCapacity.into());
        }
        self.current_call
            .charge_gas(hash_gas(source.byte_payload()?.len(), block_bytes)?)?;
        self.charge_alloc_bytes(length)?;
        let digest = H::digest(source.byte_payload()?);
        builder.store_bytes(&digest)?;
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    /// Plain bytes never silently flatten a graph or discard its references.
    fn pop_byte_source(&mut self) -> Result<Value, VMError> {
        let value = self.pop_value()?;
        self.byte_source(value)
    }

    fn byte_source(&mut self, value: Value) -> Result<Value, VMError> {
        let source = match value {
            Value::Cell(reference) => {
                let cell = cells::read_cell(&mut self.resolver(), &reference)?;
                Value::Slice(crate::Slice::new(cell)?)
            }
            value @ (Value::Slice(_) | Value::Builder(_)) => value,
            _ => return Err(VMError::TypeNotByteSource),
        };
        if source.byte_refs()? != 0 {
            return Err(CellError::InvalidFormat.into());
        }
        Ok(source)
    }

    /// Restore private annotations only after checking the authenticated public bytes.
    fn pop_annotated_bytes(&mut self) -> Result<(Value, Option<Arc<crate::CellWitness>>), VMError> {
        let value = self.pop_value()?;
        let witness = match &value {
            Value::Cell(cell) => cell.witness.clone(),
            _ => None,
        };
        let source = self.byte_source(value)?;
        // Identical public charge with or without a private annotation.
        self.charge_alloc_bytes(source.byte_payload()?.len())?;
        if let Some(witness) = &witness {
            let expected = witness.to_cell()?;
            if expected.is_pruned()
                || !expected.refs().is_empty()
                || expected.payload() != source.byte_payload()?
            {
                return Err(VMError::Cell(CellError::CellCommitmentMismatch(
                    expected.id(),
                )));
            }
        }
        Ok((source, witness))
    }

    fn pop_commitment(&mut self) -> Result<Commitment, VMError> {
        let (source, witness) = self.pop_annotated_bytes()?;
        let bytes = array32(source.byte_payload()?).ok_or(VMError::TypeNotPoint)?;
        if let Some(witness) = witness {
            if let crate::CellWitness::Point(Point::Commitment(commitment)) = witness.as_ref() {
                return Ok(commitment.as_ref().clone());
            }
        }
        Ok(Commitment::Closed(CompressedRistretto(bytes)))
    }

    /// Append a cached hash at an explicit level, without loading the Cell body.
    fn op_cell_hash(&mut self) -> Result<(), VMError> {
        let mut builder = self.pop_value()?.into_builder()?;
        let level = self.pop_byte_count(cells::MAX_CELL_LEVEL as usize)? as u8;
        let reference = self.pop_value()?.into_cell_ref()?;
        if builder.remaining_bytes() < 32 {
            return Err(CellError::PayloadCapacity.into());
        }
        self.charge_alloc_bytes(32)?;
        builder.store_bytes(&reference.commitment().hash(level)?)?;
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    /// Log raw payload bytes, never a graph or a decoded bearer value.
    fn op_log(&mut self) -> Result<(), VMError> {
        let source = self.pop_byte_source()?;
        self.charge_alloc_bytes(source.byte_payload()?.len())?;
        self.txlog
            .push(TxEntry::Data(source.byte_payload()?.to_vec()));
        Ok(())
    }

    /// `dict` — `... val key val key n → dict`. Pops `n`, then `n`
    /// key/value pairs (key on top of each pair). Duplicate keys error.
    /// Each successful insertion updates the Dict's sticky capability
    /// flags; non-portable values are allowed until a storage or transfer
    /// boundary is crossed.
    fn op_dict(&mut self) -> Result<(), VMError> {
        let n = self.pop_byte_count(usize::MAX)?;
        self.charge_alloc_items(n)?;
        let mut dict = Dict::new();
        for _ in 0..n {
            let key = self.pop_value()?.to_scalar()?;
            let value = self.pop_value()?;
            self.current_call.charge_gas(dict.native_access_gas())?;
            if dict
                .insert_strict_resolved(key, value, &mut self.resolver())
                .map_err(|(error, _value)| VMError::from(error))?
                .is_err()
            {
                return Err(VMError::DictKeyOccupied);
            }
        }
        self.push_value(Value::Dict(dict));
        Ok(())
    }

    /// `put` — `dict k v → dict'`. Strict insert; fails on
    /// occupied key.
    fn op_put(&mut self) -> Result<(), VMError> {
        let v = self.pop_value()?;
        let k = self.pop_value()?.to_scalar()?;
        let mut dict = self.pop_value()?.to_dict()?;
        self.charge_alloc_items(1)?;
        if dict.is_native() {
            self.current_call.charge_gas(dict.native_access_gas())?;
        } else {
            self.charge_value_encoding(&v, 1)?;
        }
        if dict
            .insert_strict_resolved(k, v, &mut self.resolver())
            .map_err(|(error, _value)| VMError::from(error))?
            .is_err()
        {
            return Err(VMError::DictKeyOccupied);
        }
        self.push_value(Value::Dict(dict));
        Ok(())
    }

    /// `replace` — `dict k v → dict' {prev 1 | 0}`. Overwrites
    /// the slot, returning the prior value (if any) as an optional.
    /// Stack order matches `put`: `v` on top, `k` below.
    fn op_replace(&mut self) -> Result<(), VMError> {
        let v = self.pop_value()?;
        let k = self.pop_value()?.to_scalar()?;
        let mut dict = self.pop_value()?.to_dict()?;
        let native = dict.is_native();
        self.charge_alloc_items(1)?;
        if native {
            self.current_call.charge_gas(dict.native_access_gas())?;
        } else {
            self.charge_value_encoding(&v, 1)?;
        }
        let prev = dict
            .insert_resolved(k, v, &mut self.resolver())
            .map_err(|(error, _value)| VMError::from(error))?;
        let prev = if native { prev } else {
            prev.map(|value| self.restore_private_value(value)).transpose()?
        };
        self.push_value(Value::Dict(dict));
        match prev {
            Some(prev_v) => {
                self.push_value(prev_v);
                self.push_value(Value::Scalar(Scalar::from(1u64)));
            }
            None => {
                self.push_value(Value::Scalar(Scalar::ZERO));
            }
        }
        Ok(())
    }

    /// `get` — `dict k → dict' k v`. Removes and returns the
    /// value at `k`. Fails if the key is missing.
    fn op_get(&mut self) -> Result<(), VMError> {
        let k = self.pop_value()?.to_scalar()?;
        let mut dict = self.pop_value()?.to_dict()?;
        let native = dict.is_native();
        if native {
            self.current_call.charge_gas(dict.native_access_gas())?;
        }
        let v = dict
            .remove_resolved(&k, &mut self.resolver())?
            .ok_or(VMError::DictKeyNotFound)?;
        let v = if native { v } else { self.restore_private_value(v)? };
        self.push_value(Value::Dict(dict));
        self.push_value(Value::Scalar(k));
        self.push_value(v);
        Ok(())
    }

    /// `getopt` — `dict k → dict' {v 1 | 0}`. Like `get`, but
    /// soft-fails (pushes `0`) when the key is missing.
    fn op_getopt(&mut self) -> Result<(), VMError> {
        let k = self.pop_value()?.to_scalar()?;
        let mut dict = self.pop_value()?.to_dict()?;
        let native = dict.is_native();
        if native {
            self.current_call.charge_gas(dict.native_access_gas())?;
        }
        let v = dict.remove_resolved(&k, &mut self.resolver())?;
        let v = if native { v } else {
            v.map(|value| self.restore_private_value(value)).transpose()?
        };
        self.push_value(Value::Dict(dict));
        self.push_optional_value(v);
        Ok(())
    }

    /// `getdup` — `dict k → dict {v 1 | 0}`. Copies the value
    /// without consuming it. Soft-fails with `0` on missing key; hard
    /// errors if the value exists but isn't copyable.
    fn op_getdup(&mut self) -> Result<(), VMError> {
        let k = self.pop_value()?.to_scalar()?;
        let mut dict = self.pop_value()?.to_dict()?;
        let native = dict.is_native();
        if native {
            self.current_call.charge_gas(dict.native_access_gas())?;
        }
        let copied = match dict.get_resolved(&k, &mut self.resolver())? {
            Some(v) => {
                if !v.is_copyable() {
                    return Err(VMError::TypeNotCopyable);
                }
                self.current_call.charge_gas(v.clone_gas())?;
                let copied = v.try_clone()?;
                Some(if native { copied } else { self.restore_private_value(copied)? })
            }
            None => None,
        };
        self.push_value(Value::Dict(dict));
        self.push_optional_value(copied);
        Ok(())
    }

    /// Dict-lookup epilogue: pushes `Some(v)` as `v, 1`; `None` as `0`.
    fn push_optional_value(&mut self, v: Option<Value>) {
        match v {
            Some(v) => {
                self.push_value(v);
                self.push_value(Value::Scalar(Scalar::from(1u64)));
            }
            None => self.push_value(Value::Scalar(Scalar::ZERO)),
        }
    }

    /// `first` — `dict → dict {k 1 | 0}`. Pushes the smallest key
    /// alongside a flag, or `0` if the dict is empty.
    fn op_first(&mut self) -> Result<(), VMError> {
        let dict = self.pop_value()?.to_dict()?;
        if dict.is_native() {
            self.current_call.charge_gas(dict.native_access_gas())?;
        }
        let k = dict.first_key_resolved(&mut self.resolver())?;
        self.push_value(Value::Dict(dict));
        self.push_optional_value(k.map(Value::Scalar));
        Ok(())
    }

    /// `last` — `dict → dict {k 1 | 0}`. Mirror of `first`.
    fn op_last(&mut self) -> Result<(), VMError> {
        let dict = self.pop_value()?.to_dict()?;
        if dict.is_native() {
            self.current_call.charge_gas(dict.native_access_gas())?;
        }
        let k = dict.last_key_resolved(&mut self.resolver())?;
        self.push_value(Value::Dict(dict));
        self.push_optional_value(k.map(Value::Scalar));
        Ok(())
    }

    /// `next` — `dict k → dict {k' 1 | 0}`. Smallest key strictly
    /// greater than `k`, or `0` if no such key exists.
    fn op_next(&mut self) -> Result<(), VMError> {
        let k = self.pop_value()?.to_scalar()?;
        let dict = self.pop_value()?.to_dict()?;
        if dict.is_native() {
            self.current_call.charge_gas(dict.native_access_gas())?;
        }
        let next_k = dict.next_key_after_resolved(&k, &mut self.resolver())?;
        self.push_value(Value::Dict(dict));
        self.push_optional_value(next_k.map(Value::Scalar));
        Ok(())
    }

    /// Helper: converts a stack-popped count into a `usize` ≤ `max`.
    /// Returns `IndexOutOfRange` on overflow or above `max`.
    fn pop_byte_count(&mut self, max: usize) -> Result<usize, VMError> {
        let n_int = self.pop_value()?.to_scalar()?;
        let n_u64 = n_int.to_u64().ok_or(VMError::IndexOutOfRange)?;
        let n = usize::try_from(n_u64).map_err(|_| VMError::IndexOutOfRange)?;
        if n > max {
            return Err(VMError::IndexOutOfRange);
        }
        Ok(n)
    }

    fn op_slice(&mut self) -> Result<(), VMError> {
        let reference = self.pop_value()?.into_cell_ref()?;
        let cell = cells::read_cell(&mut self.resolver(), &reference)?;
        self.push_value(Value::Slice(crate::Slice::new(cell)?));
        Ok(())
    }

    fn op_endcell(&mut self) -> Result<(), VMError> {
        let builder = self.pop_value()?.into_builder()?;
        self.current_call
            .charge_gas(cell_hash_gas(builder.used_bytes(), builder.used_refs())?)?;
        self.push_value(Value::Cell(builder.build().into()));
        Ok(())
    }

    fn op_load_ref(&mut self) -> Result<(), VMError> {
        let index = self.pop_byte_count(cells::MAX_CELL_REFS - 1)?;
        let mut slice = self.pop_value()?.into_slice()?;
        let reference = slice.load_ref(index).ok_or(VMError::IndexOutOfRange)?;
        self.push_value(Value::Slice(slice));
        self.push_value(Value::Cell(reference.into()));
        Ok(())
    }

    fn op_cell_info(&mut self) -> Result<(), VMError> {
        let refs = match self.current_call.stack.last() {
            Some(Value::Cell(_)) => 0,
            Some(Value::Slice(slice)) => slice.remaining_refs(),
            Some(Value::Builder(builder)) => builder.used_refs(),
            Some(_) => return Err(VMError::TypeNotByteSource),
            None => return Err(VMError::StackUnderflow),
        };
        self.current_call.charge_gas(refs as u64)?;
        let stats = match self.current_call.stack.last().unwrap() {
            Value::Cell(cell) => {
                let reference = cell.reference.clone();
                let cell = cells::read_cell(&mut self.resolver(), &reference)?;
                let level = cell.level();
                [
                    cell.payload().len() as u64,
                    cell.refs().len() as u64,
                    u64::from(cell.depth(level)?),
                    u64::from(level),
                    u64::from(cell.is_pruned()),
                ]
            }
            Value::Slice(slice) => cell_view_info(slice.remaining_bytes(), slice.refs())?,
            Value::Builder(builder) => cell_view_info(builder.used_bytes(), builder.refs().iter())?,
            _ => unreachable!(),
        };
        for value in stats {
            self.push_value(Value::Scalar(Scalar::from(value)));
        }
        Ok(())
    }

    /// Short/noncanonical input leaves the cursor unchanged and returns zero.
    fn op_read_uint(&mut self, count: usize) -> Result<(), VMError> {
        let mut slice = self.pop_value()?.into_slice()?;
        let mut bytes = [0; 32];
        let value = if slice.remaining_bytes() >= count {
            bytes[..count].copy_from_slice(&slice.bytes()[..count]);
            Scalar::from_bytes(bytes)
        } else {
            None
        };
        if let Some(value) = value {
            slice.advance_bytes(count);
            self.push_value(Value::Slice(slice));
            self.push_value(Value::Scalar(value));
            self.push_value(Value::Scalar(Scalar::ONE));
        } else {
            self.push_value(Value::Slice(slice));
            self.push_value(Value::Scalar(Scalar::ZERO));
        }
        Ok(())
    }

    fn op_read_bytes(&mut self) -> Result<(), VMError> {
        let count = self.pop_byte_count(usize::MAX)?;
        let mut builder = self.pop_value()?.into_builder()?;
        let mut slice = self.pop_value()?.into_slice()?;
        if slice.remaining_bytes() < count {
            self.push_value(Value::Slice(slice));
            self.push_value(Value::Builder(builder));
            self.push_value(Value::Scalar(Scalar::ZERO));
            return Ok(());
        }
        if count > builder.remaining_bytes() {
            return Err(CellError::PayloadCapacity.into());
        }
        self.charge_alloc_bytes(count)?;
        builder.store_bytes(&slice.bytes()[..count])?;
        slice.advance_bytes(count);
        self.push_value(Value::Slice(slice));
        self.push_value(Value::Builder(builder));
        self.push_value(Value::Scalar(Scalar::ONE));
        Ok(())
    }

    fn op_read_point(&mut self) -> Result<(), VMError> {
        let mut slice = self.pop_value()?.into_slice()?;
        if slice.remaining_bytes() < 32 {
            self.push_value(Value::Slice(slice));
            self.push_value(Value::Scalar(Scalar::ZERO));
            return Ok(());
        }
        let point = Point::from_bytes(array32(&slice.bytes()[..32]).unwrap());
        slice.advance_bytes(32);
        self.push_value(Value::Slice(slice));
        self.push_value(Value::Point(point));
        self.push_value(Value::Scalar(Scalar::ONE));
        Ok(())
    }

    fn op_write_point(&mut self) -> Result<(), VMError> {
        let point = self.pop_value()?.to_point()?;
        let mut builder = self.pop_value()?.into_builder()?;
        if builder.remaining_bytes() < 32 {
            return Err(CellError::PayloadCapacity.into());
        }
        self.charge_alloc_bytes(32)?;
        builder.store_bytes(&point.to_bytes())?;
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    fn op_write_ref(&mut self) -> Result<(), VMError> {
        let reference = self.pop_value()?.into_cell_ref()?;
        let mut builder = self.pop_value()?.into_builder()?;
        if builder.remaining_refs() == 0 {
            return Err(CellError::ReferenceCapacity.into());
        }
        reference.validate_child()?;
        self.charge_alloc_items(1)?;
        builder.store_ref(reference)?;
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    fn op_write_uint(&mut self, count: usize) -> Result<(), VMError> {
        let value = self.pop_value()?.to_scalar()?;
        let mut builder = self.pop_value()?.into_builder()?;
        if count > builder.remaining_bytes() {
            return Err(CellError::PayloadCapacity.into());
        }
        self.charge_alloc_bytes(count)?;
        builder.store_bytes(&value.to_bytes()[..count])?;
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    fn op_append_bytes(&mut self) -> Result<(), VMError> {
        let mut builder = self.pop_value()?.into_builder()?;
        let mut slice = self.pop_value()?.into_slice()?;
        let count = slice.remaining_bytes();
        if count > builder.remaining_bytes() {
            return Err(CellError::PayloadCapacity.into());
        }
        self.charge_alloc_bytes(count)?;
        builder.store_bytes(slice.bytes())?;
        slice.advance_bytes(count);
        self.push_value(Value::Slice(slice));
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    fn op_append_refs(&mut self) -> Result<(), VMError> {
        let mut builder = self.pop_value()?.into_builder()?;
        let mut slice = self.pop_value()?.into_slice()?;
        let count = slice.remaining_refs();
        if count > builder.remaining_refs() {
            return Err(CellError::ReferenceCapacity.into());
        }
        for reference in slice.refs() {
            reference.validate_child()?;
        }
        self.charge_alloc_items(count)?;
        for reference in slice.refs() {
            builder.store_ref(reference.clone())?;
        }
        slice.advance_refs(count);
        self.push_value(Value::Slice(slice));
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    /// Charges variable byte allocation before it occurs. Allocation gas is
    /// monotonic: freeing memory never refunds gas, so total charged growth
    /// bounds the frame's peak live heap without allocator-specific tracking.
    fn charge_alloc_bytes(&mut self, n: usize) -> Result<(), VMError> {
        self.current_call.charge_gas(alloc_byte_gas(n)?)
    }

    /// Charges variable-sized collection growth before reserving its items.
    fn charge_alloc_items(&mut self, n: usize) -> Result<(), VMError> {
        self.current_call.charge_gas(alloc_item_gas(n)?)
    }

    fn op_write_zeros(&mut self) -> Result<(), VMError> {
        let count = self.pop_byte_count(cells::MAX_CELL_PAYLOAD)?;
        let mut builder = self.pop_value()?.into_builder()?;
        if count > builder.remaining_bytes() {
            return Err(CellError::PayloadCapacity.into());
        }
        self.charge_alloc_bytes(count)?;
        builder.store_zeros(count)?;
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    /// In-place payload transform; references and payload length never change.
    fn op_bit_not(&mut self) -> Result<(), VMError> {
        let mut builder = self.pop_value()?.into_builder()?;
        self.current_call.charge_gas(builder.used_bytes() as u64)?;
        for byte in builder.payload_mut() {
            *byte = !*byte;
        }
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    fn op_bitwise(&mut self, operation: fn(u8, u8) -> u8) -> Result<(), VMError> {
        if matches!(self.current_call.stack.last(), Some(Value::Builder(_))) {
            return Err(VMError::TypeNotCellOrSlice);
        }
        let source = self.pop_byte_source()?;
        let mut builder = self.pop_value()?.into_builder()?;
        if builder.used_bytes() != source.byte_payload()?.len() {
            return Err(VMError::BitwiseSizeMismatch);
        }
        self.current_call.charge_gas(builder.used_bytes() as u64)?;
        for (byte, other) in builder.payload_mut().iter_mut().zip(source.byte_payload()?) {
            *byte = operation(*byte, *other);
        }
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    /// Fixed-width, big-endian payload shift. Displaced bits are discarded.
    fn op_shift(&mut self, left: bool) -> Result<(), VMError> {
        let count = self.pop_byte_count(256)?;
        let mut builder = self.pop_value()?.into_builder()?;
        self.current_call.charge_gas(builder.used_bytes() as u64)?;
        shift_bytes(builder.payload_mut(), count, left);
        self.push_value(Value::Builder(builder));
        Ok(())
    }

    /// `abs` — interprets a scalar in the centered interval `[-H, H]`,
    /// where `H = (ℓ - 1) / 2`, and pushes its magnitude followed by
    /// `0` for a non-negative value or `1` for a negative value.
    fn op_abs(&mut self) -> Result<(), VMError> {
        let v = self.pop_value()?.to_scalar()?;
        let sign = u64::from(v.is_centered_negative());
        self.push_value(Value::Scalar(v.centered_abs()));
        self.push_value(Value::Scalar(Scalar::from(sign)));
        Ok(())
    }

    /// _a b_ **eq** → _a b {0|1}_ (cleartext) | _a b_ **eq** → _constraint_ (CS)
    fn op_eq<D: Delegate>(&mut self, _delegate: &mut D) -> Result<(), VMError> {
        let n = self.current_call.stack.len();
        if n < 2 {
            return Err(VMError::StackUnderflow);
        }
        // CS-lift only when an operand is actually a CS type (Variable /
        // Expression). A "non-Scalar" test would wrongly route Cell /
        // Point comparisons (e.g. the `anchor … eq verify` binding idiom)
        // into `to_expression()` and hard-fail in external context.
        let cs_involved = matches!(
            self.current_call.stack[n - 1],
            Value::Variable(_) | Value::Expression(_)
        ) || matches!(
            self.current_call.stack[n - 2],
            Value::Variable(_) | Value::Expression(_)
        );
        if self.is_external() && cs_involved {
            self.charge_top_value_growth(2)?;
            let b = self.pop_value()?.to_expression()?;
            let a = self.pop_value()?.to_expression()?;
            self.push_value(Value::Constraint(Constraint::eq(a, b)));
        } else {
            if self.current_call.stack[n - 1].is_cell_container()
                && self.current_call.stack[n - 2].is_cell_container()
            {
                let gas = factual_hash_gas(&self.current_call.stack[n - 1])?
                    .checked_add(factual_hash_gas(&self.current_call.stack[n - 2])?)
                    .ok_or(VMError::OutOfGas)?;
                self.current_call.charge_gas(gas)?;
            } else {
                self.charge_top_value_growth(2)?;
            }
            let eq = self.current_call.stack[n - 1].try_eq(&self.current_call.stack[n - 2])?;
            let bit = if eq { 1u64 } else { 0u64 };
            self.push_value(Value::Scalar(Scalar::from(bit)));
        }
        Ok(())
    }

    /// _x_ **neg** → _-x_  (Scalar cleartext or Expression LC negate)
    fn op_neg<D: Delegate>(&mut self, _delegate: &mut D) -> Result<(), VMError> {
        self.charge_top_value_growth(1)?;
        let v = self.pop_value()?.neg()?;
        self.push_value(v);
        Ok(())
    }

    /// _x y_ **add** → _z_  (cleartext modulo ℓ, or LC sum)
    fn op_add<D: Delegate>(&mut self, _delegate: &mut D) -> Result<(), VMError> {
        self.charge_top_value_growth(2)?;
        let b = self.pop_value()?;
        let a = self.pop_value()?;
        let r = a.add(b, self.is_external())?;
        self.push_value(r);
        Ok(())
    }

    /// _x y_ **mul** → _z_  (cleartext modulo ℓ, MSM scalar-point lift,
    /// or CS multiplier)
    fn op_mul<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        use crate::msm::MultiscalarMul;
        self.charge_top_value_growth(2)?;
        let b = self.pop_value()?;
        let a = self.pop_value()?;
        match (a, b) {
            (Value::Scalar(x), Value::Scalar(y)) => {
                self.push_value(Value::Scalar(x * y));
                Ok(())
            }
            // scalar * point / point * scalar → MSM with one term.
            (Value::Scalar(s), Value::Point(p)) | (Value::Point(p), Value::Scalar(s)) => {
                self.push_value(Value::MultiscalarMul(MultiscalarMul::term(
                    s.to_dalek(),
                    p.to_compressed(),
                )));
                Ok(())
            }
            // scalar * MSM / MSM * scalar → scale coefficients.
            (Value::Scalar(s), Value::MultiscalarMul(m))
            | (Value::MultiscalarMul(m), Value::Scalar(s)) => {
                self.push_value(Value::MultiscalarMul(m.scaled(s.to_dalek())));
                Ok(())
            }
            // Quadratic group-element products are not defined in
            // Sigma-protocol semantics.
            (Value::Point(_), Value::Point(_))
            | (Value::Point(_), Value::MultiscalarMul(_))
            | (Value::MultiscalarMul(_), Value::Point(_))
            | (Value::MultiscalarMul(_), Value::MultiscalarMul(_)) => Err(VMError::TypeNotScalar),
            (a, b) if self.is_external() => {
                let aexpr = a.to_expression()?;
                let bexpr = b.to_expression()?;
                if matches!(&aexpr, Expression::LinearCombination(_, _))
                    && matches!(&bexpr, Expression::LinearCombination(_, _))
                {
                    self.current_call.charge_gas(r1cs_gas(1)?)?;
                }
                let product = aexpr.multiply(bexpr, delegate.cs());
                self.push_value(Value::Expression(product));
                Ok(())
            }
            _ => Err(VMError::TypeNotScalar),
        }
    }

    /// `divmod` — `x z → d r`. Centered integer division, truncating
    /// toward zero: `x = d*z + r` over integers, `|r| < |z|`, and a
    /// nonzero remainder has the dividend's sign. Errors on zero divisor.
    fn op_divmod(&mut self) -> Result<(), VMError> {
        let z = self.pop_value()?.to_scalar()?;
        let x = self.pop_value()?.to_scalar()?;
        let (d, r) = x.div_rem(z).ok_or(VMError::DivByZero)?;
        self.push_value(Value::Scalar(d));
        self.push_value(Value::Scalar(r));
        Ok(())
    }

    /// Reduce up to 64 little-endian bytes to a Scalar; no implicit graph traversal.
    fn op_mod252(&mut self) -> Result<(), VMError> {
        let source = self.pop_byte_source()?;
        let bytes = source.byte_payload()?;
        if bytes.len() > 64 {
            return Err(VMError::BytesTooLongForModReduction);
        }
        let mut buf = [0u8; 64];
        buf[..bytes.len()].copy_from_slice(bytes);
        let scalar = DalekScalar::from_bytes_mod_order_wide(&buf);
        self.push_value(Value::Scalar(Scalar::from(scalar)));
        Ok(())
    }

    /// _x_ **not** → _y_  (Scalar logical, or Constraint negation)
    fn op_not<D: Delegate>(&mut self, _delegate: &mut D) -> Result<(), VMError> {
        self.charge_top_value_growth(1)?;
        let v = self.pop_value()?.not()?;
        self.push_value(v);
        Ok(())
    }

    /// _a b_ **and** → _c_  (Scalar logical, or Constraint conjunction)
    fn op_and<D: Delegate>(&mut self, _delegate: &mut D) -> Result<(), VMError> {
        self.charge_top_value_growth(2)?;
        let b = self.pop_value()?;
        let a = self.pop_value()?;
        let r = a.and(b, self.is_external())?;
        self.push_value(r);
        Ok(())
    }

    /// _a b_ **or** → _c_  (Scalar logical, or Constraint disjunction)
    fn op_or<D: Delegate>(&mut self, _delegate: &mut D) -> Result<(), VMError> {
        self.charge_top_value_growth(2)?;
        let b = self.pop_value()?;
        let a = self.pop_value()?;
        let r = a.or(b, self.is_external())?;
        self.push_value(r);
        Ok(())
    }

    /// `size` — peeks the top value and pushes its length as an
    /// `Scalar` (Cell/Slice/Builder byte count, Dict entry count). Other types
    /// error with `TypeHasNoLength`.
    fn op_size(&mut self) -> Result<(), VMError> {
        if matches!(self.current_call.stack.last(), Some(Value::Cell(_))) {
            let reference = self
                .current_call
                .stack
                .last()
                .unwrap()
                .clone()
                .into_cell_ref()?;
            let cell = cells::read_cell(&mut self.resolver(), &reference)?;
            if cell.is_pruned() {
                return Err(CellError::PrunedCell.into());
            }
            self.push_value(Value::Scalar(Scalar::from(cell.payload().len() as u64)));
            return Ok(());
        }
        let top = self
            .current_call
            .stack
            .last()
            .ok_or(VMError::StackUnderflow)?;
        let len = match top {
            Value::Dict(d) => d.len(),
            Value::Slice(s) => s.remaining_bytes(),
            Value::Builder(b) => b.used_bytes(),
            _ => return Err(VMError::TypeHasNoLength),
        };
        self.push_value(Value::Scalar(Scalar::from(len as u64)));
        Ok(())
    }

    /// `verify` — pop one and assert truthiness.
    ///
    /// - `Scalar`: errors `VerifyFailed` if zero, else pops.
    /// - `Constraint`: hands the constraint to the CS so the proof
    ///   commits to its truth. Requires external context.
    /// - `MultiscalarMul`: appends `sum(s_i · P_i) == identity` to
    ///   the delegate's `BatchVerifier` (alongside Schnorr/Musig
    ///   sigs). Requires external context.
    fn op_verify<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        self.charge_top_value_growth(1)?;
        match self.pop_value()? {
            Value::Scalar(v) => {
                if v.is_zero() {
                    return Err(VMError::VerifyFailed);
                }
                Ok(())
            }
            Value::Constraint(c) => {
                self.require_external()?;
                let multiplications = c.multiplier_count();
                self.current_call.charge_gas(r1cs_gas(multiplications)?)?;
                c.verify(delegate.cs())?;
                self.deferred_multiplications = self
                    .deferred_multiplications
                    .saturating_add(multiplications);
                Ok(())
            }
            Value::MultiscalarMul(m) => {
                self.require_external()?;
                let terms = m.into_terms();
                // Instruction and term traversal were already charged above.
                self.current_call.charge_gas(
                    msm_verify_gas(terms.len())? - GAS_PER_INSTRUCTION - terms.len() as u64,
                )?;
                let scalars: Vec<curve25519_dalek::scalar::Scalar> =
                    terms.iter().map(|(s, _)| *s).collect();
                let points: Vec<Option<curve25519_dalek::ristretto::RistrettoPoint>> =
                    terms.iter().map(|(_, p)| p.decompress()).collect();
                // basepoint_scalar = 0: no contribution from the
                // basepoint; the entire MSM must sum to identity.
                // The BatchVerifier multiplies the whole statement
                // by a fresh random scalar (RNG-based, owned by the
                // delegate) so unrelated batched statements can't
                // cancel each other. Per-frame rollback is achieved
                // not by batching per-frame, but by snapshotting the
                // delegate's batch on call entry (see `step`) and
                // restoring on call failure (see `fail_current_call`).
                musig::BatchVerification::append(
                    delegate.batch_verifier(),
                    curve25519_dalek::scalar::Scalar::ZERO,
                    scalars,
                    points,
                );
                Ok(())
            }
            other => {
                self.push_value(other);
                Err(VMError::TypeNotScalar)
            }
        }
    }

    /// Public MSM evaluation is allowed in both execution contexts and never touches a batch.
    fn op_eval_point(&mut self) -> Result<(), VMError> {
        let terms = match self.current_call.stack.last() {
            Some(Value::MultiscalarMul(msm)) => msm.len(),
            Some(_) => return Err(VMError::TypeNotMultiscalarMul),
            None => return Err(VMError::StackUnderflow),
        };
        self.current_call
            .charge_gas(msm_eval_gas(terms)? - GAS_PER_INSTRUCTION)?;
        let Value::MultiscalarMul(msm) = self.pop_value()? else {
            unreachable!()
        };
        self.push_value(Value::Point(msm.evaluate()?));
        Ok(())
    }

    /// **label:n** → ø — record a jump target. See ADR 0015.
    fn op_label(&mut self, n: u32) -> Result<(), VMError> {
        self.current_call.record_label(n as usize)
    }

    /// **jump:n** → ø — unconditional jump to label `n`.
    fn op_jump(&mut self, n: u32) -> Result<(), VMError> {
        self.current_call.jump_to_label(n as usize)
    }

    /// _x_ **jumpif:n** → ø — pop an Scalar; jump to label `n` iff `x ≠ 0`.
    fn op_jumpif(&mut self, n: u32) -> Result<(), VMError> {
        let x = self.pop_value()?.to_scalar()?;
        if x.is_zero() {
            Ok(())
        } else {
            self.current_call.jump_to_label(n as usize)
        }
    }

    /// _result_ **return** → ø
    ///
    /// Pops the frame, refunds leftover gas, and pushes one result
    /// followed by `1` (success marker) onto the caller's stack.
    /// Errors `ReturnAtRoot` at the outermost frame.
    fn op_return(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        // Outermost frame has no recipient for a result.
        if self.call_stack.is_empty() {
            return Err(VMError::ReturnAtRoot);
        }

        if self.current_call.stack.is_empty() {
            return Err(VMError::BadReturnArity);
        }
        if self.current_call.stack.len() > 1 {
            return Err(VMError::StackNotClean);
        }

        self.validate_actor_call_storage(registry.as_deref())?;
        let result = self.current_call.stack.pop().expect("one result checked above");
        let leftover_gas = self
            .current_call
            .gas_limit
            .saturating_sub(self.current_call.gas_used);

        let parent = self.call_stack.pop().expect("checked non-empty above");
        self.current_call = parent;
        self.clean_exit_to_parent(leftover_gas);
        self.current_call.stack.push(result);
        self.current_call.stack.push(Value::Scalar(Scalar::ONE));
        Ok(())
    }

    /// `type` — peeks the top value and pushes its type code as an
    /// `Scalar`. The original value remains on the stack underneath.
    fn op_type(&mut self) -> Result<(), VMError> {
        let code = self
            .current_call
            .stack
            .last()
            .ok_or(VMError::StackUnderflow)?
            .type_code();
        self.push_value(Value::Scalar(Scalar::from(code as u64)));
        Ok(())
    }

    fn pop_bytes_32(&mut self) -> Result<[u8; 32], VMError> {
        let source = self.pop_byte_source()?;
        array32(source.byte_payload()?).ok_or(VMError::MalformedAddress)
    }

    fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), VMError> {
        self.charge_alloc_bytes(bytes.len())?;
        self.current_call
            .charge_gas(hash_gas(bytes.len().saturating_add(2), 64)?)?;
        self.push_value(Value::Cell(Cell::new(bytes.to_vec(), vec![])?.into()));
        Ok(())
    }

    fn pop_actor_address(&mut self) -> Result<ActorAddress, VMError> {
        let reference = self.pop_value()?.into_cell_ref()?;
        let root = cells::read_cell(&mut self.resolver(), &reference)?;
        ActorAddress::from_cell(&root, &mut self.resolver()).map_err(VMError::from)
    }

    fn op_nop(&mut self) -> Result<(), VMError> {
        Ok(())
    }

    /// Returns the current call's actor identity, or
    /// `OpcodeRequiresActorContext` if the frame has none.
    fn require_actor(&self) -> Result<&ActorID, VMError> {
        self.current_call
            .kind
            .actor()
            .ok_or(VMError::OpcodeRequiresActorContext)
    }

    /// _token_ **amount** → _token qty flv_
    fn op_amount(&mut self) -> Result<(), VMError> {
        let token = self.pop_value()?;
        match token {
            Value::ClearToken(t) => {
                let qty = t.qty();
                let flv = t.flv();
                self.push_value(Value::ClearToken(t));
                self.push_value(Value::Scalar(qty));
                self.push_value(Value::Scalar(flv));
                Ok(())
            }
            Value::Token(t) => {
                self.charge_alloc_items(2)?;
                let qty_pt = Point::commitment(t.qty.clone());
                let flv_pt = Point::commitment(t.flv.clone());
                self.push_value(Value::Token(t));
                self.push_value(Value::Point(qty_pt));
                self.push_value(Value::Point(flv_pt));
                Ok(())
            }
            other => {
                self.push_value(other);
                Err(VMError::TypeNotToken)
            }
        }
    }

    /// _qty:Scalar tag_ **issuepub** → _CT_
    ///
    /// Cleartext mint under the enclosing actor's identity. Requires
    /// an internal actor frame; `ExternalRoot` and `ContractOpen` error
    /// `OpcodeRequiresActorContext` (issuance domains are disjoint —
    /// see spec.md §issuepub). Non-`Scalar` qty hard-fails
    /// `TypeNotScalar`; the confidential path lives in [`op_issuepriv`].
    fn op_issuepub(&mut self) -> Result<(), VMError> {
        let actor = self.require_actor()?.clone();
        let tag = self.pop_byte_source()?;
        let qty = match self.pop_value()? {
            Value::Scalar(i) => i,
            _ => return Err(VMError::TypeNotScalar),
        };
        let flv = flavor_from_actor(&actor, tag.byte_payload()?);
        // Cleartext qty + flv go straight into the txlog as `Scalar`s —
        // no commitment indirection. The `IssuePub` entry is publicly
        // auditable directly on the wire.
        self.txlog.push(TxEntry::IssuePub(qty, flv));
        self.push_value(Value::ClearToken(ClearToken::new(qty, flv)));
        Ok(())
    }

    /// _qty:Variable tag_ **issuepriv** → _T_
    ///
    /// Confidential mint under the enclosing predicate's identity.
    /// Requires a `CallKind::ContractOpen` frame (errors
    /// `OpcodeRequiresPredicateContext` otherwise) AND external
    /// context (errors `ExternalOnly`; the CS lane is needed for the
    /// range proof and the qty commitment registration).
    fn op_issuepriv<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        // Snapshot the predicate before any pop, so a wrong frame
        // surfaces before we mutate the stack.
        let predicate = match &self.current_call.kind {
            CallKind::ContractOpen { predicate, .. } => predicate.clone(),
            _ => return Err(VMError::OpcodeRequiresPredicateContext),
        };
        self.require_external()?;
        use spacesuit::BitRange;
        // One committed variable plus a 64-bit range gadget.
        self.current_call.charge_gas(r1cs_gas(65)?)?;

        let tag = self.pop_byte_source()?;
        let qty_commitment = self.pop_value()?.to_variable()?.into_commitment()?;
        // Register the Pedersen commitment with the CS. On the prover
        // the open assignment travels through; on the verifier only
        // the closed point. Same r1cs::Variable on both sides.
        let (_qty_point, qty_r1cs) = delegate.commit_variable(&qty_commitment)?;
        // 64-bit range proof on the qty commitment — `borrow`/`mix`
        // already enforce this for confidential token paths.
        let qty_assignment = match qty_commitment.assignment() {
            Some(i) => Some(scalar_to_signed_integer(i)?),
            None => None,
        };
        spacesuit::range_proof(
            delegate.cs(),
            qty_r1cs.into(),
            qty_assignment,
            BitRange::max(),
        )
        .map_err(VMError::R1CSError)?;

        // Flavor binds to the enclosing predicate + tag.
        let flv = flavor_from_predicate(&predicate, tag.byte_payload()?);
        let flv_commit = Commitment::unblinded(flv);

        self.txlog.push(TxEntry::IssuePriv(
            qty_commitment.to_point(),
            flv_commit.to_point(),
        ));
        self.push_value(Value::Token(Token::new(qty_commitment, flv_commit)));
        Ok(())
    }

    /// _token_ **retire** → ø
    fn op_retire(&mut self) -> Result<(), VMError> {
        let val = self.pop_value()?;
        match val {
            Value::ClearToken(t) => {
                if !t.is_portable() {
                    self.push_value(Value::ClearToken(t));
                    return Err(VMError::NegativeTokenRetirement);
                }
                let qty_commit = Commitment::unblinded(t.qty());
                let flv_commit = Commitment::unblinded(t.flv());
                self.txlog.push(TxEntry::Retire(
                    qty_commit.to_point(),
                    flv_commit.to_point(),
                ));
                Ok(())
            }
            Value::Token(t) => {
                self.txlog
                    .push(TxEntry::Retire(t.qty.to_point(), t.flv.to_point()));
                Ok(())
            }
            other => {
                self.push_value(other);
                Err(VMError::TypeNotToken)
            }
        }
    }

    /// _qty flv_ **borrow** → _widetoken token_
    ///
    /// Quantity and flavor independently accept Scalar or committed Variable.
    fn op_borrow<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        let flv_val = self.pop_value()?;
        let qty_v = self.pop_value()?;
        match (qty_v, flv_val) {
            (Value::Scalar(qty), Value::Scalar(flv)) => {
                let pos = ClearToken::new(qty, flv);
                let neg = pos.negated();
                self.push_value(Value::ClearToken(neg));
                self.push_value(Value::ClearToken(pos));
                Ok(())
            }
            (
                qty @ (Value::Scalar(_) | Value::Variable(_)),
                flv @ (Value::Scalar(_) | Value::Variable(_)),
            ) => {
                self.require_external()?;
                self.op_borrow_encrypted_inner(qty, flv, delegate)
            }
            (Value::Point(_), _) | (_, Value::Point(_)) => Err(VMError::TokenRequiresCS),
            _ => Err(VMError::TypeNotScalar),
        }
    }

    /// _a b_ **merge** → _{c 1 | a b 0}_
    fn op_merge(&mut self) -> Result<(), VMError> {
        let b = self.pop_value()?.to_clear_token()?;
        let a = self.pop_value()?.to_clear_token()?;
        match a.merge_into(b) {
            Ok(c) => {
                self.push_value(Value::ClearToken(c));
                self.push_value(Value::Scalar(Scalar::from(1u64)));
            }
            Err((a, b)) => {
                self.push_value(Value::ClearToken(a));
                self.push_value(Value::ClearToken(b));
                self.push_value(Value::Scalar(Scalar::ZERO));
            }
        }
        Ok(())
    }

    /// _a q_ **split** → _a' b_
    fn op_split(&mut self) -> Result<(), VMError> {
        let q = self.pop_value()?.to_scalar()?;
        let a = self.pop_value()?.to_clear_token()?;
        match a.split(q) {
            Some((remainder, new_token)) => {
                self.push_value(Value::ClearToken(remainder));
                self.push_value(Value::ClearToken(new_token));
                Ok(())
            }
            None => Err(VMError::TokenSplitOutOfRange),
        }
    }

    /// _cid tag_ **issuepubflv** → _int_
    ///
    /// Consumer-side helper for `issuepub`: pops `tag` (Cell/Slice/Builder) and
    /// `cid` (Cell/Slice/Builder, exactly 32 bytes — an actor id), pushes
    /// `flavor_from_actor(cid, tag)` as `Scalar`. Pure helper: no CS,
    /// no txlog effect, no actor-context requirement. Use to
    /// recompute and verify a cleartext token's flavor without
    /// running the corresponding `issuepub`.
    fn op_issuepubflv(&mut self) -> Result<(), VMError> {
        let tag = self.pop_byte_source()?;
        let cid = self.pop_byte_source()?;
        let bytes = array32(cid.byte_payload()?).ok_or(VMError::IndexOutOfRange)?;
        let flv = flavor_from_actor(&ActorID::from(bytes), tag.byte_payload()?);
        self.push_value(Value::Scalar(flv));
        Ok(())
    }

    /// _pred tag_ **issueprivflv** → _int_
    ///
    /// Consumer-side helper for `issuepriv`: pops `tag` (Cell/Slice/Builder) and
    /// `pred` (Cell/Slice/Builder, exactly 32 bytes — a compressed Ristretto
    /// predicate point), pushes `flavor_from_predicate(pred, tag)` as
    /// `Scalar`. Pure helper: no CS, no txlog effect, no
    /// predicate-context requirement. Use to recompute and verify a
    /// confidential token's flavor (the flv commitment is unblinded,
    /// so its point determines the scalar).
    fn op_issueprivflv(&mut self) -> Result<(), VMError> {
        let tag = self.pop_byte_source()?;
        let pred = self.pop_byte_source()?;
        let bytes = array32(pred.byte_payload()?).ok_or(VMError::IndexOutOfRange)?;
        let predicate = Predicate::opaque(curve25519_dalek::ristretto::CompressedRistretto(bytes));
        let flv = flavor_from_predicate(&predicate, tag.byte_payload()?);
        self.push_value(Value::Scalar(flv));
        Ok(())
    }

    /// Values may return upward in any form, but only portable values may be
    /// delegated downward into a child frame. The caller must resolve any
    /// outstanding loan before calling another actor or predicate.
    fn require_portable_call_arg(arg: &Value) -> Result<(), VMError> {
        if !arg.is_portable() {
            return Err(VMError::NonPortableInCall);
        }
        Ok(())
    }

    /// Merlin message for `signcall`: binds the signature to the
    /// factual program Cell graph. Programs add further context (anchor,
    /// actor identity) via explicit checks inside their script.
    fn signcall_message(program: &Cell) -> Vec<u8> {
        let mut t = Transcript::new(b"flamevm.signcall");
        t.append_message(b"program", &program.id());
        let mut out = vec![0u8; 32];
        t.challenge_bytes(b"msg", &mut out);
        out
    }

    /// _contract-cell_ **input** → _contract_
    ///
    /// The public body must resolve in the frozen transaction Cell hierarchy. Private
    /// witnesses may replace its typed values only after matching this identity.
    fn op_input(&mut self) -> Result<(), VMError> {
        self.require_external()?;
        let reference = self.pop_value()?.into_cell_ref()?;
        let id = reference.id();
        let cell = cells::read_cell(&mut self.resolver(), &reference)?;
        let public = Contract::from_trusted_cell(&cell, &mut self.resolver())?;
        self.charge_value_encoding(public.payload(), 3)?;
        if public.id() != id {
            return Err(VMError::MalformedContractEncoding);
        }
        if let Some(witness) = self.contract_witnesses.get(&id) {
            if witness.id() != id {
                return Err(VMError::MalformedContractEncoding);
            }
            let mut pending = vec![witness.payload()];
            while let Some(value) = pending.pop() {
                match value {
                    Value::Dict(dict) => pending.extend(dict.cached_values()),
                    other => {
                        self.value_witnesses
                            .insert(other.to_cell()?.id(), other.clone());
                    }
                }
            }
        }
        let predicate = public.predicate.clone();
        let anchor = public.anchor;
        let payload = self.restore_private_value(public.into_payload())?;
        let contract = Contract::new(predicate, anchor, payload)?;
        self.last_anchor = Some(Anchor(id));
        self.txlog.push(TxEntry::Input(id));
        self.push_value(Value::Contract(Box::new(contract)));
        Ok(())
    }

    /// Private leaf witnesses never materialize a Dict or bypass a public path.
    fn restore_private_value(&mut self, public: Value) -> Result<Value, VMError> {
        if matches!(public, Value::Dict(_)) || !public.is_portable() {
            return Ok(public);
        }
        self.charge_value_encoding(&public, 1)?;
        let id = public.to_cell()?.id();
        Ok(self.value_witnesses.get(&id).cloned().unwrap_or(public))
    }

    /// _payload pred_ **contract** → _contract_
    fn op_contract(&mut self) -> Result<(), VMError> {
        let pred = self.pop_value()?.to_point()?.to_predicate()?;
        let payload = self.pop_value()?;
        self.charge_value_encoding(&payload, 2)?;
        let anchor = self.consume_anchor()?;
        let contract = Contract::new(pred, anchor, payload)?;
        self.push_value(Value::Contract(Box::new(contract)));
        Ok(())
    }

    /// _payload pred_ **output** → ø
    fn op_output(&mut self) -> Result<(), VMError> {
        let pred = self.pop_value()?.to_point()?.to_predicate()?;
        let payload = self.pop_value()?;
        self.charge_value_encoding(&payload, 2)?;
        let anchor = self.consume_anchor()?;
        let contract = Contract::new(pred, anchor, payload)?;
        self.txlog.push(TxEntry::Output(contract));
        Ok(())
    }

    /// _contract ik program gas arg_ **open** → _result 1 | contract arg 0_
    ///
    /// Authenticates the supplied program's level-zero hash. The
    /// isolated frame receives the stored payload followed by one portable argument.
    fn op_open(&mut self) -> Result<(), VMError> {
        let arg = self.pop_value()?;
        Self::require_portable_call_arg(&arg)?;
        let gas = self.pop_gas_limit()?;
        let reference = self.pop_value()?.into_cell_ref()?;
        let internal_key = self.pop_value()?.to_point()?.to_compressed();
        let contract = self.pop_value()?.to_contract()?;
        self.current_call.charge_gas(GAS_POINT_DECOMPRESS)?;
        let root = cells::read_cell(&mut self.resolver(), &reference)?;
        contract.predicate.verify_taproot(&internal_key, &root)?;
        let id = root.id();
        let program = Program::new(root.clone())?;
        let code_bytes = program.len;
        let instructions = if let Some(instructions) = self.script_witnesses.get(&id) {
            if crate::script::instruction_cell(instructions)?.id() != id {
                return Err(VMError::TaprootProofMismatch);
            }
            Some(instructions.clone())
        } else {
            None
        };
        let child_anchor = self.split_anchor_for_call()?;
        self.enter_contract_open_frame(
            contract,
            Script::Opaque(Vec::new()),
            code_bytes,
            gas,
            arg,
            child_anchor,
        )?;
        self.current_call.code = None;
        self.current_call.entry_program = Some(root.as_ref().clone());
        self.current_call.program = Some(program);
        self.current_call.instructions = instructions;
        Ok(())
    }

    /// _cell_ **exec** is a tail transfer: only code cursors and Cell-local
    /// labels change. Frame identity, escrow, stack and gas remain intact.
    fn op_exec(&mut self) -> Result<(), VMError> {
        let reference = self.pop_value()?.into_cell_ref()?;
        let root = cells::read_cell(&mut self.resolver(), &reference)?;
        let program = Program::new(root.clone())?;
        let id = root.id();
        let instructions = self
            .current_call
            .private_scripts
            .get(&id)
            .or_else(|| self.script_witnesses.get(&id))
            .cloned();
        if let Some(instructions) = &instructions {
            if crate::script::instruction_cell(instructions)?.id() != id {
                return Err(VMError::TaprootProofMismatch);
            }
        }
        self.current_call.program = Some(program);
        self.current_call.instructions = instructions;
        self.current_call.cursor = 0;
        self.current_call.instruction_cursor = 0;
        self.current_call.labels.clear();
        Ok(())
    }

    /// _contract script sig gas arg_ **signcall** → _result 1 | contract arg 0_
    ///

    /// The signature commits to the factual Cell graph, including supplied/pruned branches.
    fn op_signcall(&mut self) -> Result<(), VMError> {
        let arg = self.pop_value()?;
        Self::require_portable_call_arg(&arg)?;
        let gas = self.pop_gas_limit()?;
        let signature_bytes = self.pop_byte_source()?;
        let program_value = match self.pop_value()? {
            Value::Cell(cell) => cell,
            _ => return Err(VMError::TypeNotCell),
        };
        let root = cells::read_cell(&mut self.resolver(), &program_value)?;
        let contract = self.pop_value()?.to_contract()?;
        let sig: [u8; 64] = signature_bytes
            .byte_payload()?
            .try_into()
            .map_err(|_| VMError::BadSignatureBytes)?;
        self.current_call.charge_gas(GAS_SIGNATURE_VERIFY)?;
        let program = Program::new(root.clone())?;
        let code_bytes = program.len;
        let msg = Self::signcall_message(&root);
        let mut private_scripts = BTreeMap::new();
        if let Some(crate::CellWitness::Script(instructions)) = program_value.witness() {
            let (witness_root, overlays) = crate::script::compile_program(instructions)?;
            if witness_root.id() != root.id() {
                return Err(VMError::TaprootProofMismatch);
            }
            private_scripts = overlays;
        }
        let instructions = private_scripts
            .get(&root.id())
            .or_else(|| self.script_witnesses.get(&root.id()))
            .cloned();
        if let Some(instructions) = &instructions {
            if crate::script::instruction_cell(instructions)?.id() != root.id() {
                return Err(VMError::TaprootProofMismatch);
            }
        }
        let verification_key = contract.predicate.verification_key();
        let external = self.is_external();
        if !external {
            let signature =
                musig::Signature::from_bytes(sig).map_err(|_| VMError::BadSignatureBytes)?;
            let key = musig::VerificationKey::from_compressed(verification_key);
            signature
                .verify(&mut signcall_verification_transcript(&msg), key)
                .map_err(|_| VMError::SignatureVerificationFailed)?;
        }
        let child_anchor = self.split_anchor_for_call()?;
        if external {
            self.deferred_sigs.push(DeferredSig::Explicit {
                verification_key,
                message: msg,
                signature: sig,
            });
        }
        self.enter_contract_open_frame(
            contract,
            Script::Cell(root.as_ref().clone()),
            code_bytes,
            gas,
            arg,
            child_anchor,
        )?;
        self.current_call.code = None;
        self.current_call.entry_program = Some(root.as_ref().clone());
        self.current_call.program = Some(program);
        self.current_call.instructions = instructions;
        self.current_call.private_scripts = private_scripts;
        Ok(())
    }

    /// Shared tail of `op_open` / `op_signcall`: build a new
    /// `ContractOpen` frame snapshotting the caller's CS context, pour
    /// `contract.payload` then `arg` onto the new stack, swap the
    /// parent out, and switch the active anchor to `child_anchor`
    /// (the `left` half of the parent's call-entry split).
    fn enter_contract_open_frame(
        &mut self,
        contract: Contract,
        code: Script,
        code_bytes: usize,
        gas: u64,
        arg: Value,
        child_anchor: Anchor,
    ) -> Result<(), VMError> {
        if self.call_stack.len() >= MAX_CALL_DEPTH {
            return Err(VMError::CallDepthExceeded);
        }
        self.charge_alloc_items(2)?;
        let clone_gas = 1u64.saturating_add(contract.clone_gas()).saturating_add(arg.clone_gas());
        self.current_call.charge_gas(clone_gas)?;
        self.current_call.snap_failure_values = vec![
            Value::Contract(Box::new(contract.clone())),
            arg.clone(),
        ];
        let external_context = self.is_external();
        let caller_id = self.current_call.kind.actor().map(ActorID::to_hash);
        let mut frame = CallFrame::from_code(
            code,
            CallKind::ContractOpen {
                predicate: contract.predicate.clone(),
                external_context,
                caller_id,
            },
            gas,
        )
        .with_anchor(child_anchor);
        frame.gas_used = alloc_byte_gas(code_bytes)?
            .saturating_add(alloc_item_gas(2)?);
        self.current_call.charge_gas(gas)?;
        frame.stack.push(contract.into_payload());
        frame.stack.push(arg);
        let parent = core::mem::replace(&mut self.current_call, frame);
        self.call_stack.push(parent);
        self.last_anchor = Some(child_anchor);
        Ok(())
    }

    /// Pops the child gas grant as a non-negative `u64`.
    fn pop_gas_limit(&mut self) -> Result<u64, VMError> {
        self.pop_value()?
            .to_scalar()?
            .to_u64()
            .ok_or(VMError::InvalidBitrange)
    }

    /// _arg refund gas addr_ **send** → ø
    ///
    /// Queues a [`Message`] for the consensus layer to instantiate as a
    /// future internal tx and emits a `TxEntry::Send`. The anchor is
    /// ratcheted from `last_anchor` before the entry is appended. There
    /// is no VM-level method operand; a selector, when used, is an
    /// ordinary payload argument (ADR 0020).
    fn op_send(&mut self) -> Result<(), VMError> {
        let mut target = self.pop_actor_address()?;
        let gas = self
            .pop_value()?
            .to_scalar()?
            .to_u64()
            .ok_or(VMError::InvalidBitrange)?;
        let refund_predicate = Predicate::opaque(curve25519_dalek::ristretto::CompressedRistretto(
            self.pop_bytes_32()?,
        ));
        let arg = self.pop_value()?;

        if !arg.is_portable() {
            return Err(VMError::NonPortableInSend);
        }
        self.charge_value_encoding(&arg, 4)?;
        self.current_call.charge_gas(arg.clone_gas())?;
        if let ActorAddress::Init { ctor: witness }
        | ActorAddress::Restore { state: witness, .. } = &mut target
        {
            let mut reader = CodeCells(self.resolver());
            match cells::read_cell(&mut reader, witness)
                .and_then(|root| root.load_graph_scoped(&mut reader))
            {
                Ok(root) => *witness = root.into(),
                // An existing Init destination can execute without its constructor.
                Err(CellError::MissingCell(_)) => {}
                Err(error) => return Err(error.into()),
            }
        }
        let anchor = self.consume_anchor()?;
        let caller = self.current_call.kind.actor().cloned();
        // Single source of truth: the full Message lives in the
        // TxLog as `TxEntry::Send(Message)` — symmetric with
        // `TxEntry::Output(Contract)`. The block builder scans these
        // entries to construct internal-tx deliveries; no separate
        // queue.
        let message = Message::new(target, caller, anchor, arg, gas, refund_predicate)?;
        // A send reserves future execution from the active frame. The grant is
        // intentionally not refunded: asynchronous execution has no live
        // caller to receive it, and descendants must divide an existing grant
        // rather than minting fresh gas.
        self.current_call.charge_gas(gas)?;
        self.txlog.push(TxEntry::Send(message));
        Ok(())
    }

    /// _arg gas addr_ **call** → _result 1 | arg 0_
    ///
    /// Canonical actor routing, initialization/restoration and execution share
    /// one checkpoint. A failed call restores its argument and all mutations.
    fn op_call(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let caller = *self.require_actor()?;
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        let address = self.pop_actor_address()?;
        let callee = address.to_actor_id();
        let gas = self.pop_gas_limit()?;
        self.current_call.charge_gas(gas)?;
        let arg = self.pop_value()?;
        Self::require_portable_call_arg(&arg)?;

        registry.push_checkpoint();
        let mut initial_gas = 0;
        let pre_frame: Result<(CallFrame, Arc<CellIndex>, Option<TxEntry>), VMError> = (|| {
            if self.call_stack.len() >= MAX_CALL_DEPTH {
                return Err(VMError::CallDepthExceeded);
            }
            let effect = prepare_actor_address(
                registry,
                &address,
                &self.cells,
                Some(&self.current_call.actor_cells),
                &mut self.resolved_cells,
                &mut initial_gas,
                gas,
            )?;
            initial_gas = initial_gas
                .saturating_add(
                    registry
                        .actor_code_bytes(&callee)?
                        .saturating_mul(GAS_PER_ALLOC_BYTE),
                )
                .saturating_add(alloc_item_gas(1)?);
            if initial_gas > gas {
                return Err(VMError::OutOfGas);
            }
            let actor_cells = registry.actor_cells(&callee)?;
            let mut reader = ExecutionCells {
                external: &self.cells,
                actor: &actor_cells,
                address_sender: None,
                resolved_cells: &mut self.resolved_cells,
                gas_used: &mut initial_gas,
                gas_limit: gas,
            };
            let script = registry.load_program_with_cells(&callee, &mut reader)?;
            let frame = CallFrame::from_cell(
                script,
                CallKind::ActorCall {
                    actor: callee,
                    caller,
                },
                gas,
            )?;
            Ok((frame, actor_cells, effect))
        })();
        let (mut frame, actor_cells, effect) = match pre_frame {
            Ok(value) => value,
            Err(error) => {
                registry.pop_checkpoint_rollback();
                if !matches!(
                    error,
                    VMError::OutOfGas | VMError::Cell(CellError::ResourceExhausted)
                ) {
                    self.current_call.gas_used = self
                        .current_call
                        .gas_used
                        .saturating_sub(gas.saturating_sub(initial_gas));
                }
                self.push_failed_values(vec![arg]);
                return Ok(());
            }
        };
        let enter = (|| {
            self.charge_alloc_items(1)?;
            self.current_call.charge_gas(arg.clone_gas())?;
            self.current_call.snap_failure_values = vec![arg.clone()];
            self.split_anchor_for_call()
        })();
        let callee_anchor = match enter {
            Ok(anchor) => anchor,
            Err(error) => {
                registry.pop_checkpoint_rollback();
                return Err(error);
            }
        };
        frame.anchor = Some(callee_anchor);
        frame.gas_used = initial_gas;
        frame.actor_cells = actor_cells;
        frame.stack.push(arg);
        let parent = core::mem::replace(&mut self.current_call, frame);
        self.call_stack.push(parent);
        self.last_anchor = Some(callee_anchor);
        if let Some(effect) = effect {
            self.txlog.push(effect);
        }
        Ok(())
    }

    /// **load** → _value_
    ///
    /// **Checks out** the current actor's state: moves its portable
    /// `Value` out of
    /// the registry (the actor goes empty) and pushes it onto the
    /// stack. While checked out, any call/load against this actor fails
    /// `ActorEmpty` — the state's presence is the re-entrancy lock (ADR
    /// 0017). A frame must `save` it back or explicitly dismantle it
    /// before returning; leftover state fails the clean-stack rule and
    /// rolls back. State shape is not VM-enforced. Re-loading an
    /// already-checked-out actor errors `ActorEmpty`.
    fn op_load(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let actor = self.require_actor()?.clone();
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        let state_bytes = registry.actor_state_bytes(&actor)?;
        self.current_call
            .charge_gas(state_bytes.saturating_mul(GAS_PER_ALLOC_BYTE))?;
        let state = registry.load_state_with_cells(&actor, &mut self.resolver())?;
        self.push_value(state);
        Ok(())
    }

    /// _value_ **save** → ø
    ///
    /// Pops the state value (any portable `Value`), validates
    /// portability, and **moves it back** into
    /// the current actor (which must be checked out by a prior `load` —
    /// else `SaveWithoutLoad`, since saving would clobber live state).
    /// Portability is the canonical storage gate — every inserted value
    /// must be portable (`Scalar`, `Cell`, `Point`, `Dict` of
    /// portable, non-negative `ClearToken`, `Token`). Non-portable
    /// values (`Contract`, `Merlin`, `Variable`, `Expression`,
    /// `Constraint`, `MultiscalarMul`, `WideToken`, negative
    /// `ClearToken`) hard-fail `NonPortableInState`.
    fn op_save(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let actor = *self.require_actor()?;
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        let state = self.pop_value()?;
        // Portability is the canonical storage gate — checked here
        // before any registry mutation so a bad state is rejected
        // cleanly. Distinct from encodability (which the encoder may
        // or may not implement for a given variant).
        if !state.is_portable() {
            return Err(VMError::NonPortableInState);
        }
        self.charge_value_encoding(&state, 2)?;
        self.current_call.charge_gas(state.clone_gas())?;
        // Rust-level deep clone for the txlog entry. The registry
        // takes ownership of one copy; the txlog gets another.
        // `clone` ignores VM stack-copyability rules so portable
        // linear values (Token) survive — those are exactly what
        // actor state is for.
        let state_for_log = state.clone();
        // Moves the state back in; errors `SaveWithoutLoad` if the
        // actor isn't checked out (no matching `load`).
        registry.save_state(&actor, state)?;
        registry.validate_actor_storage(&actor, self.block_height)?;
        // Structural effect. State-machine replay applies these
        // last-write-wins per actor; the merkle leaf hashes
        // `state_root(state)`, while the entry carries the full state
        // value for direct consumers.
        self.txlog.push(TxEntry::ActorSave {
            actor,
            state: state_for_log,
        });
        Ok(())
    }

    /// _code_ **setcode** → ø
    ///
    /// Replaces the actor's native program Cell graph
    /// and records `TxEntry::SetCode`. Upgrade *policy* (who may
    /// call this) is the author's, gated in the actor's own code via
    /// `callerid` — actors authenticate by identity, not signatures.
    /// See ADR 0018.
    fn op_setcode(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let actor = *self.require_actor()?;
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        let reference = self.pop_value()?.into_cell_ref()?;
        let code = cells::read_cell(&mut self.resolver(), &reference)?;
        if code.is_pruned() {
            return Err(CellError::PrunedCell.into());
        }
        self.current_call
            .charge_gas(code_materialization_gas(&code))?;
        // Persistence needs the same supplied graph regardless of cache residency.
        let code = code.load_graph_scoped(&mut CodeCells(self.resolver()))?;
        registry.set_code(&actor, code.clone())?;
        registry.validate_actor_storage(&actor, self.block_height)?;
        self.txlog.push(TxEntry::SetCode { actor, code });
        Ok(())
    }

    /// _q_ **addstorage** → _debt 1 | 0_. Invalid market requests are
    /// soft failures; type, context, and host invariant errors are hard.
    fn op_addstorage(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let actor = *self.require_actor()?;
        let request = self.pop_value()?.to_scalar()?;
        let Some(bytes) = request.to_u64() else {
            self.push_value(Value::Scalar(Scalar::ZERO));
            return Ok(());
        };
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        let Some(purchase) = registry.purchase_storage(&actor, bytes, self.block_height)? else {
            self.push_value(Value::Scalar(Scalar::ZERO));
            return Ok(());
        };
        if purchase.fee_sparks.is_zero() || purchase.fee_sparks.is_centered_negative() {
            return Err(VMError::StorageArithmeticOverflow);
        }
        self.txlog.push(TxEntry::StoragePurchase {
            actor: actor.clone(),
            bytes,
            expiry_height: purchase.expiry_height,
            fee_sparks: purchase.fee_sparks,
        });
        self.push_value(Value::ClearToken(ClearToken::new(
            -purchase.fee_sparks,
            FLAME_FLAVOR,
        )));
        self.push_value(Value::Scalar(Scalar::ONE));

        Ok(())
    }

    /// _q_ **quotestorage** → _fee 1 | 0_. Read-only counterpart to
    /// [`Self::op_addstorage`].
    fn op_quotestorage(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let actor = self.require_actor()?.clone();
        let request = self.pop_value()?.to_scalar()?;
        let Some(bytes) = request.to_u64() else {
            self.push_value(Value::Scalar(Scalar::ZERO));
            return Ok(());
        };
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        match registry.quote_storage(&actor, bytes, self.block_height)? {
            Some(quote)
                if !quote.fee_sparks.is_zero() && !quote.fee_sparks.is_centered_negative() =>
            {
                self.push_value(Value::Scalar(quote.fee_sparks));
                self.push_value(Value::Scalar(Scalar::ONE));
            }
            Some(_) => return Err(VMError::StorageArithmeticOverflow),
            None => self.push_value(Value::Scalar(Scalar::ZERO)),
        }
        Ok(())
    }

    /// _contract_ **signtx** → _payload_
    ///
    /// External-only. Defers a TxID-bound signature for the contract's predicate,
    /// and returns its single payload Value.
    fn op_signtx(&mut self) -> Result<(), VMError> {
        self.require_external()?;
        let contract = self.pop_value()?.to_contract()?;
        self.charge_value_encoding(contract.payload(), 1)?;
        // Each authorized contract contributes one key/message term to the final
        // aggregate-signature verification.
        self.current_call.charge_gas(GAS_SIGNATURE_VERIFY)?;
        self.deferred_sigs.push(DeferredSig::TxBound {
            verification_key: contract.predicate.verification_key(),
            contract_id: contract.id(),
        });
        self.push_value(contract.into_payload());
        Ok(())
    }

    /// **selfid** → _cell_
    fn op_selfid(&mut self) -> Result<(), VMError> {
        let actor = self.require_actor()?.clone();
        self.push_bytes(&actor.to_hash())
    }

    /// **anchor** → _cell_
    ///
    /// Pushes the tx's *current* anchor (the value the next consume
    /// site would split). Hard-fails `AnchorMissing` if no anchor
    /// has been claimed yet — same rule as `contract` / `output` /
    /// `send`. Available in either context.
    fn op_anchor(&mut self) -> Result<(), VMError> {
        let a = self.last_anchor.ok_or(VMError::AnchorMissing)?;
        self.push_bytes(&a.0)
    }

    /// **callerid** → _cell_
    ///
    /// Pushes the direct caller actor id in a zero-ref Cell, or zero bytes when no
    /// authenticated actor directly invoked this frame. Available in actor
    /// and ContractOpen frames, but not ExternalRoot.
    fn op_callerid(&mut self) -> Result<(), VMError> {
        if matches!(self.current_call.kind, CallKind::ExternalRoot) {
            return Err(VMError::OpcodeRequiresActorContext);
        }
        let bytes = self.current_call.kind.caller_id().unwrap_or([0u8; 32]);
        self.push_bytes(&bytes)
    }

    /// **timelock** → _n {0|1}_
    ///
    /// Pushes the transaction's `locktime` and a unit flag
    /// (0 = block height, 1 = Unix timestamp). Bitcoin BIP-65
    /// convention: `flag = 1` iff `locktime >= LOCKTIME_TIMESTAMP_THRESHOLD`
    /// (i.e. ≥ 500_000_000, approximately 1985-11-05 Unix time).
    fn op_timelock(&mut self) -> Result<(), VMError> {
        let lt = self.header.locktime as u64;
        let flag: u64 = if lt >= LOCKTIME_TIMESTAMP_THRESHOLD as u64 {
            1
        } else {
            0
        };
        self.push_value(Value::Scalar(Scalar::from(lt)));
        self.push_value(Value::Scalar(Scalar::from(flag)));
        Ok(())
    }

    /// **gas** → _n_  Pushes remaining gas for the current call.
    fn op_gas(&mut self) -> Result<(), VMError> {
        let remaining = self
            .current_call
            .gas_limit
            .saturating_sub(self.current_call.gas_used);
        self.push_value(Value::Scalar(Scalar::from(remaining)));
        Ok(())
    }

    /// **usage** → _n_.
    fn op_usage(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let actor = self.require_actor()?;
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        let usage = registry.actor_usage(actor)?;
        self.push_value(Value::Scalar(Scalar::from(usage)));
        Ok(())
    }

    /// _height_ **capacity** → _bytes_.
    fn op_capacity(&mut self, registry: Option<&mut dyn ActorRegistry>) -> Result<(), VMError> {
        let actor = self.require_actor()?.clone();
        let height = self
            .pop_value()?
            .to_scalar()?
            .to_u64()
            .ok_or(VMError::InvalidBitrange)?;
        if height < self.block_height {
            return Err(VMError::StorageHeightInPast);
        }
        let registry = registry.ok_or(VMError::RegistryUnavailable)?;
        let capacity = registry.actor_capacity(&actor, height)?;
        self.push_value(Value::Scalar(Scalar::from(capacity)));
        Ok(())
    }

    /// **alloc** → _var_: allocate once; `expr` consumes copies of its handle.
    ///
    /// Allocates a low-level R1CS variable. Witness `Some(i)` on the
    /// prover; `None` on the verifier.
    fn op_alloc<D: Delegate>(
        &mut self,
        witness: Option<Scalar>,
        delegate: &mut D,
    ) -> Result<(), VMError> {
        self.require_external()?;
        self.charge_alloc_items(1)?;
        self.current_call.charge_gas(r1cs_gas(1)?)?;
        use bulletproofs::r1cs::ConstraintSystem;
        let witness_scalar = witness.map(|i| i.to_dalek());
        let r1cs_var = delegate
            .cs()
            .allocate(witness_scalar)
            .map_err(VMError::R1CSError)?;
        self.push_value(Value::Variable(Variable::Allocated {
            variable: r1cs_var,
            assignment: witness,
        }));
        Ok(())
    }

    /// Consume a Scalar or Variable to produce a constant or one-term Expression.
    fn op_expr<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        self.require_external()?;
        use curve25519_dalek::scalar::Scalar as DalekScalar;
        let (r1cs_var, witness) = match self.pop_value()? {
            Value::Scalar(scalar) => {
                self.push_value(Value::Expression(Expression::constant(scalar)));
                return Ok(());
            }
            Value::Variable(variable) => {
                self.charge_alloc_items(1)?;
                match variable {
                    Variable::Committed(commitment) => {
                        self.current_call.charge_gas(r1cs_gas(1)?)?;
                        let (_, variable) = delegate.commit_variable(&commitment)?;
                        (variable, commitment.assignment())
                    }
                    Variable::Allocated {
                        variable,
                        assignment,
                    } => (variable, assignment),
                }
            }
            _ => return Err(VMError::TypeNotVariable),
        };
        let expr = Expression::LinearCombination(vec![(r1cs_var, DalekScalar::ONE)], witness);
        self.push_value(Value::Expression(expr));
        Ok(())
    }

    /// _x n_ **range** → _x_
    ///
    /// Checks a raw scalar in any context. In external execution only,
    /// an Expression is checked or constrained to the same `[0, 2^n)` range.
    /// The checked value is returned with its original type.
    fn op_range<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        use bulletproofs::r1cs::LinearCombination as LC;
        use spacesuit::BitRange;

        // Pop n (bit-count) — must be a scalar in [1, 64].
        let n_int = self.pop_value()?.to_scalar()?;
        let n_u64 = n_int.to_u64().ok_or(VMError::BitCountOutOfRange)?;
        let n_usize = usize::try_from(n_u64).map_err(|_| VMError::BitCountOutOfRange)?;
        if n_usize == 0 {
            return Err(VMError::BitCountOutOfRange);
        }
        let bit_range = BitRange::new(n_usize).ok_or(VMError::BitCountOutOfRange)?;

        let value = self.pop_value()?;
        if let Value::Scalar(scalar) = value {
            if !scalar_fits_in_n_bits(scalar, n_usize) {
                return Err(VMError::InvalidBitrange);
            }
            self.push_value(Value::Scalar(scalar));
            return Ok(());
        }

        self.require_external()?;
        self.charge_alloc_items(n_usize)?;

        let expr = value.to_expression()?;

        match &expr {
            Expression::Constant(value) => {
                // Public constants are checked without touching the CS.
                if !scalar_fits_in_n_bits(*value, n_usize) {
                    return Err(VMError::InvalidBitrange);
                }
                self.push_value(Value::Expression(expr));
                Ok(())
            }
            Expression::LinearCombination(terms, assignment) => {
                self.current_call.charge_gas(r1cs_gas(n_usize)?)?;
                let lc: LC = terms.iter().cloned().collect();
                // Convert the witness (if present) to spacesuit's
                // SignedInteger. Scalars in the u64 range map cleanly;
                // other assignments fail the prover at
                // gadget time via `to_u64() → None` inside
                // spacesuit::range_proof.
                let assignment_si = match assignment {
                    Some(i) => Some(scalar_to_signed_integer(*i)?),
                    None => None,
                };
                spacesuit::range_proof(delegate.cs(), lc, assignment_si, bit_range)
                    .map_err(VMError::R1CSError)?;
                self.push_value(Value::Expression(expr));
                Ok(())
            }
        }
    }

    fn op_commit(&mut self) -> Result<(), VMError> {
        self.require_external()?;
        let commitment = self.pop_value()?.to_point()?.to_commitment()?;
        self.push_value(Value::Variable(Variable::Committed(commitment)));
        Ok(())
    }

    /// Bind public or committed components, range-prove quantity, and create debt.
    fn op_borrow_encrypted_inner<D: Delegate>(
        &mut self,
        qty: Value,
        flv: Value,
        delegate: &mut D,
    ) -> Result<(), VMError> {
        use bulletproofs::r1cs::ConstraintSystem;
        use spacesuit::BitRange;
        // Public quantities are checked directly; only secret quantities need a range gadget.
        let public_quantity = matches!(&qty, Value::Scalar(_));
        self.current_call
            .charge_gas(r1cs_gas(if public_quantity { 3 } else { 67 })?)?;
        // Commit both to the CS. Prover uses the open witness; verifier
        // sees only the closed point. Either way, the returned r1cs vars
        // are bound to the same commitment point on both sides.
        let bind = |value: Value,
                    delegate: &mut D|
         -> Result<(Commitment, bulletproofs::r1cs::Variable), VMError> {
            match value {
                Value::Variable(variable) => {
                    let commitment = variable.into_commitment()?;
                    let (_, variable) = delegate.commit_variable(&commitment)?;
                    Ok((commitment, variable))
                }
                Value::Scalar(scalar) => {
                    let variable = delegate
                        .cs()
                        .allocate(Some(scalar.to_dalek()))
                        .map_err(VMError::R1CSError)?;
                    delegate.cs().constrain(variable - scalar.to_dalek());
                    Ok((Commitment::unblinded(scalar), variable))
                }
                _ => Err(VMError::TypeNotScalar),
            }
        };
        if let Value::Scalar(scalar) = &qty {
            if scalar.to_u64().is_none() {
                return Err(VMError::InvalidBitrange);
            }
        }
        let (flv_commitment, flv_var) = bind(flv, delegate)?;
        let (qty_commitment, qty_var) = bind(qty, delegate)?;
        // Witness assignments (prover only). Negative qty is a
        // protocol error here — borrow's +T is range-proven non-negative.
        let qty_assignment = match qty_commitment.assignment() {
            Some(i) => Some(scalar_to_signed_integer(i)?),
            None => None,
        };
        let flv_assignment = flv_commitment.assignment().map(|i| i.to_dalek());
        // 64-bit range proof on the positive qty.
        if !public_quantity {
            spacesuit::range_proof(
                delegate.cs(),
                qty_var.into(),
                qty_assignment,
                BitRange::max(),
            )
            .map_err(VMError::R1CSError)?;
        }
        // Allocate -qty in the CS, witness = -qty_assignment.
        let neg_qty_assignment = qty_assignment.map(|q| -q);
        let neg_qty_var = delegate
            .cs()
            .allocate(neg_qty_assignment.map(|q| q.to_scalar()))
            .map_err(VMError::R1CSError)?;
        // Constrain qty + (-qty) = 0.
        delegate.cs().constrain(qty_var + neg_qty_var);
        // Build the WideToken (negative half) and the Token (positive
        // half). The Token carries the prover's open commitments
        // unchanged so downstream `mix` / `cloak` can re-commit them.
        let wide = WideToken(spacesuit::AllocatedValue {
            q: neg_qty_var,
            f: flv_var,
            assignment: match (neg_qty_assignment, flv_assignment) {
                (Some(q), Some(f)) => Some(Box::new(spacesuit::Value { q, f })),
                _ => None,
            },
        });
        let token = Token::new(qty_commitment, flv_commitment);
        self.push_value(Value::WideToken(wide));
        self.push_value(Value::Token(token));
        Ok(())
    }

    /// _qty_ **fee** → _widetoken_
    ///
    /// External-only. Records `TxEntry::Fee(qty)`, allocates a WideToken
    /// debt with `q = -qty` and the native Flame flavor, then pushes it.
    fn op_fee<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        self.require_external()?;
        use bulletproofs::r1cs::ConstraintSystem;
        self.current_call.charge_gas(r1cs_gas(2)?)?;
        let qty = self.pop_value()?.to_scalar()?;
        if qty.is_centered_negative() {
            return Err(VMError::FeeQtyNegative);
        }
        let qty_u64 = qty.to_u64().ok_or(VMError::FeeTooHigh)?;
        self.total_fee.add(qty_u64)?;
        let qty_scalar: curve25519_dalek::scalar::Scalar = qty.into();
        let flv_scalar: curve25519_dalek::scalar::Scalar = FLAME_FLAVOR.into();
        let q_var = delegate
            .cs()
            .allocate(Some(-qty_scalar))
            .map_err(VMError::R1CSError)?;
        delegate.cs().constrain(q_var + qty_scalar);
        let f_var = delegate
            .cs()
            .allocate(Some(flv_scalar))
            .map_err(VMError::R1CSError)?;
        delegate.cs().constrain(f_var - flv_scalar);
        let assignment = Some(Box::new(spacesuit::Value {
            q: -spacesuit::SignedInteger::from(qty_u64),
            f: flv_scalar,
        }));
        let wide = WideToken(spacesuit::AllocatedValue {
            q: q_var,
            f: f_var,
            assignment,
        });
        self.push_value(Value::WideToken(wide));
        self.txlog.push(TxEntry::Fee(qty_u64));
        Ok(())
    }

    /// Converts a stack value into a `spacesuit::AllocatedValue` for
    /// the cloak gadget. Token/ClearToken commit to the CS; WideToken
    /// unwraps in place.
    fn value_to_allocated<D: Delegate>(
        value: Value,
        delegate: &mut D,
    ) -> Result<spacesuit::AllocatedValue, VMError> {
        let (qty, flv) = match value {
            Value::WideToken(w) => return Ok(w.0),
            Value::Token(t) => (t.qty, t.flv),
            Value::ClearToken(c) => (
                Commitment::unblinded(c.qty()),
                Commitment::unblinded(c.flv()),
            ),
            _ => return Err(VMError::TypeNotToken),
        };
        let (_, qty_var) = delegate.commit_variable(&qty)?;
        let (_, flv_var) = delegate.commit_variable(&flv)?;
        let qty_assg = match qty.assignment() {
            Some(i) => Some(scalar_to_signed_integer(i)?),
            None => None,
        };
        let flv_assg = flv.assignment().map(|i| i.to_dalek());
        Ok(spacesuit::AllocatedValue {
            q: qty_var,
            f: flv_var,
            assignment: match (qty_assg, flv_assg) {
                (Some(q), Some(f)) => Some(Box::new(spacesuit::Value { q, f })),
                _ => None,
            },
        })
    }

    /// _anytokens… commitments… m n_ **mix** → _tokens_
    ///
    /// Invokes the spacesuit cloak gadget to balance `m` input tokens
    /// against `n` range-proven output tokens per flavor.
    fn op_mix<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        self.require_external()?;
        // Pop n (output count) and m (input count).
        let n = self.pop_byte_count(usize::MAX)?;
        let m = self.pop_byte_count(usize::MAX)?;
        // Degenerate shapes (m=0 or n=0) make `spacesuit::cloak`'s
        // `k_mix` underflow (`0..k-1` with usize `k=0`). Reject at
        // the opcode boundary so the error is deterministic.
        if m == 0 || n == 0 {
            return Err(VMError::MixDegenerate);
        }
        let mix_items = m.checked_add(n).ok_or(VMError::OutOfGas)?;
        let quadratic_work = mix_items.checked_mul(mix_items).ok_or(VMError::OutOfGas)?;
        self.charge_alloc_items(quadratic_work)?;
        self.current_call.charge_gas(r1cs_gas(quadratic_work)?)?;
        // Stack depth check: 2n commitment byte sources plus m bearer values.
        let needed = m.saturating_add(n.saturating_mul(2));
        if needed > self.current_call.stack.len() {
            return Err(VMError::StackUnderflow);
        }
        // Output commitments retain private annotations only after public-byte validation.
        let mut output_tokens: Vec<Token> = Vec::with_capacity(n);
        let mut cloak_outs: Vec<spacesuit::AllocatedValue> = Vec::with_capacity(n);
        for _ in 0..n {
            let flv_commit = self.pop_commitment()?;
            let qty_commit = self.pop_commitment()?;
            let token = Token::new(qty_commit, flv_commit);
            // Build the AllocatedValue against the CS.
            let allocated = Self::value_to_allocated(Value::Token(token.clone()), delegate)?;
            // Insert at front so the deepest output ends up at cloak_outs[0].
            output_tokens.insert(0, token);
            cloak_outs.insert(0, allocated);
        }
        // Build inputs.
        let mut cloak_ins: Vec<spacesuit::AllocatedValue> = Vec::with_capacity(m);
        for _ in 0..m {
            let item = self.pop_value()?;
            let allocated = Self::value_to_allocated(item, delegate)?;
            cloak_ins.insert(0, allocated);
        }
        // Run the cloak gadget. On constraint-system error, surface
        // as R1CSError; the verifier will reject the proof.
        spacesuit::cloak(delegate.cs(), cloak_ins, cloak_outs).map_err(VMError::R1CSError)?;
        // Push the output Tokens in the same order (deepest first).
        for token in output_tokens {
            self.push_value(Value::Token(token));
        }
        Ok(())
    }

    /// `decrypt` — `token f f' q q' → cleartoken`. Reveals a
    /// cleartext quantity / flavor pair for an encrypted Token by
    /// supplying their cleartext values (`f`, `q`) and Pedersen
    /// blinding factors (`f'`, `q'`).
    ///
    /// External execution appends both opening equations to the delegate's
    /// batch and checks them at transaction finalization. Internal execution
    /// checks both equations immediately because it has no deferred batch.
    ///
    /// All four scalar operands (`f`, `f'`, `q`, `q'`) are popped as
    /// `Scalar`. The Token is popped last (deepest on stack). The
    /// `ClearToken(q, f)` is pushed only after the applicable check is queued
    /// or completed.
    fn op_decrypt<D: Delegate>(&mut self, delegate: &mut D) -> Result<(), VMError> {
        use bulletproofs::PedersenGens;
        let q_blind = self.pop_value()?.to_scalar()?;
        let q_value = self.pop_value()?.to_scalar()?;
        let f_blind = self.pop_value()?.to_scalar()?;
        let f_value = self.pop_value()?.to_scalar()?;
        let token = match self.pop_value()? {
            Value::Token(t) => t,
            _ => return Err(VMError::TypeNotToken),
        };
        // Two point decompressions and two two-term opening equations. The
        // charge is identical whether they are checked immediately or batched.
        let opening_gas = linear_gas(GAS_MSM_VERIFY_BASE, GAS_MSM_VERIFY_TERM, 2)?;
        self.current_call
            .charge_gas(opening_gas.checked_mul(2).ok_or(VMError::OutOfGas)?)?;
        let gens = PedersenGens::default();
        let qty_point = token.qty.to_point().decompress();
        let flv_point = token.flv.to_point().decompress();
        if self.is_external() {
            // Each equation gets its own random batch factor, so it cannot
            // cancel another opening or an unrelated signature statement.
            let neg_one = -DalekScalar::ONE;
            musig::BatchVerification::append(
                delegate.batch_verifier(),
                q_value.to_dalek(),
                [q_blind.to_dalek(), neg_one],
                [Some(gens.B_blinding), qty_point],
            );
            musig::BatchVerification::append(
                delegate.batch_verifier(),
                f_value.to_dalek(),
                [f_blind.to_dalek(), neg_one],
                [Some(gens.B_blinding), flv_point],
            );
        } else {
            let qty_point = qty_point.ok_or(VMError::InvalidPoint)?;
            let flv_point = flv_point.ok_or(VMError::InvalidPoint)?;
            let expected_qty = gens.B * q_value.to_dalek() + gens.B_blinding * q_blind.to_dalek();
            let expected_flv = gens.B * f_value.to_dalek() + gens.B_blinding * f_blind.to_dalek();
            if expected_qty != qty_point || expected_flv != flv_point {
                return Err(VMError::CommitmentOpeningMismatch);
            }
        }
        self.push_value(Value::ClearToken(ClearToken::new(q_value, f_value)));
        Ok(())
    }
}

/// Checks a canonical residue against `[0, 2^n)` for `n` in `[1, 64]`.
/// Used for raw scalars and public Expression constants, without R1CS.
fn scalar_fits_in_n_bits(value: Scalar, n: usize) -> bool {
    if n == 64 {
        return value.to_u64().is_some();
    }
    match value.to_u64() {
        Some(v) => v < (1u64 << n),
        None => false,
    }
}

/// Converts a scalar's centered value to a `spacesuit::SignedInteger`
/// witness when its magnitude fits u64. Other values error `InvalidBitrange`.
///
/// **Prover/verifier asymmetry (fail-closed, liveness-only).** Callers
/// invoke this only on the *prover* side (`commitment.assignment()` is
/// `Some` for the prover, `None` for the verifier). An out-of-`u64`
/// witness makes this error on the prover while the verifier — lacking
/// the assignment — does not, so a caught sub-call could push a `0`
/// marker on the prover and `1` on the verifier. That divergence is
/// **fail-closed**: the proof binds the whole CS via Fiat–Shamir, so
/// any prover/verifier control-flow divergence makes the proof fail to
/// verify (verifier rejects) — it can never make the verifier *accept*
/// an invalid tx. The effect is a self-inflicted liveness edge (a
/// prover that commits to an out-of-range qty produces an unverifiable
/// tx), not a soundness break. Hardening (making such witness-gated
/// failures tx-level/uncatchable so they never reach a marker) is a
/// deliberate ZK-review item, not a drive-by change.
fn scalar_to_signed_integer(value: Scalar) -> Result<spacesuit::SignedInteger, VMError> {
    if value.is_centered_negative() {
        let mag = value.centered_abs();
        let mag_u64 = mag.to_u64().ok_or(VMError::InvalidBitrange)?;
        Ok(-spacesuit::SignedInteger::from(mag_u64))
    } else {
        let v = value.to_u64().ok_or(VMError::InvalidBitrange)?;
        Ok(spacesuit::SignedInteger::from(v))
    }
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;

#[cfg(test)]
mod cell_execution_tests {
    use super::*;
    use crate::{PredicateTree, Prover, Verifier};
    use bulletproofs::PedersenGens;

    #[test]
    fn temporary_dictionary_wrapping_is_cheap_until_materialization() {
        let frame = CallFrame::new(Vec::new(), CallKind::ExternalRoot, 15_000);
        let mut vm = VM::new(
            TxHeader {
                version: 1,
                locktime: 0,
            },
            frame,
        );
        vm.push_value(Value::Cell(
            crate::CellValue::from_bytes(vec![7; 1000]).unwrap(),
        ));
        let mut costs = Vec::new();
        for _ in 0..20 {
            vm.push_value(Value::Scalar(Scalar::ZERO));
            vm.push_value(Value::Scalar(Scalar::ONE));
            let before = vm.current_call.gas_used;
            vm.op_dict().unwrap();
            costs.push(vm.current_call.gas_used - before);
        }
        assert!(costs.iter().all(|cost| *cost == costs[0]));
        let nested = vm.pop_value().unwrap();
        let gas = nested.encoding_gas().unwrap();
        assert!(gas > costs.iter().sum());
        vm.current_call.gas_limit = vm.current_call.gas_used + gas - 1;
        assert!(matches!(vm.charge_value_encoding(&nested, 1), Err(VMError::OutOfGas)));
    }

    #[test]
    fn native_dictionary_nesting_is_bounded_before_clone_or_serialization() {
        let frame = CallFrame::new(Vec::new(), CallKind::ExternalRoot, 10_000);
        let mut vm = VM::new(TxHeader { version: 1, locktime: 0 }, frame);
        vm.push_value(Value::Scalar(Scalar::ONE));
        for _ in 0..256 {
            vm.push_value(Value::Scalar(Scalar::ZERO));
            vm.push_value(Value::Scalar(Scalar::ONE));
            vm.op_dict().unwrap();
        }
        vm.push_value(Value::Scalar(Scalar::ZERO));
        vm.push_value(Value::Scalar(Scalar::ONE));
        assert!(matches!(vm.op_dict(), Err(VMError::Cell(CellError::LimitExceeded))));
        assert!(vm.current_call.gas_used < vm.current_call.gas_limit);
    }

    #[test]
    fn dictionary_insertion_charges_for_a_cell_reference_not_its_body() {
        let mut costs = Vec::new();
        for length in [0, cells::MAX_CELL_PAYLOAD] {
            let cell = crate::CellValue::from_bytes(vec![7; length]).unwrap();
            let id = cell.id();
            let frame = CallFrame::new(Vec::new(), CallKind::ExternalRoot, 100);
            let mut vm = VM::new(
                TxHeader {
                    version: 1,
                    locktime: 0,
                },
                frame,
            );
            vm.push_value(Value::Dict(Dict::new()));
            vm.push_value(Value::Scalar(Scalar::ZERO));
            vm.push_value(Value::Cell(cell));
            vm.op_put().unwrap();
            costs.push(vm.current_call.gas_used);
            let dict = vm.pop_value().unwrap().to_dict().unwrap();
            assert!(
                matches!(dict.get(&Scalar::ZERO), Some(Value::Cell(value)) if value.id() == id)
            );
        }
        assert_eq!(costs[0], costs[1]);
        assert!(costs[0] > 0);

        let frame = CallFrame::new(Vec::new(), CallKind::ExternalRoot, costs[0] - 1);
        let mut vm = VM::new(
            TxHeader {
                version: 1,
                locktime: 0,
            },
            frame,
        );
        vm.push_value(Value::Dict(Dict::new()));
        vm.push_value(Value::Scalar(Scalar::ZERO));
        vm.push_value(Value::Cell(crate::CellValue::from_bytes(vec![]).unwrap()));
        assert!(matches!(vm.op_put(), Err(VMError::OutOfGas)));
    }

    #[test]
    fn native_record_access_is_cheap_and_serialization_does_not_change_its_gas() {
        let native = Dict::from_values(vec![
            Value::Scalar(Scalar::from(7u64)),
            Value::Scalar(Scalar::from(9u64)),
        ]);
        let serialized = native.to_cell().unwrap();
        let backed = Dict::from_trusted_cell(&serialized, &mut ()).unwrap();
        let mut costs = Vec::new();
        for dict in [native.clone(), native, backed] {
            let frame = CallFrame::new(Vec::new(), CallKind::ExternalRoot, 100_000);
            let mut vm = VM::new(TxHeader { version: 1, locktime: 0 }, frame);
            vm.push_value(Value::Dict(dict));
            for _ in 0..2 {
                vm.push_value(Value::Scalar(Scalar::ZERO));
                let before = vm.current_call.gas_used;
                vm.op_getdup().unwrap();
                costs.push(vm.current_call.gas_used - before);
                assert!(matches!(vm.pop_value().unwrap(), Value::Scalar(n) if n == Scalar::ONE));
                assert!(matches!(vm.pop_value().unwrap(), Value::Scalar(n) if n == Scalar::from(7u64)));
            }
        }
        assert_eq!(&costs[..4], &[3; 4]);
        assert!(costs[4] > costs[5]);
        assert!(costs[4] > 10 * costs[0]);
    }

    #[test]
    fn input_dict_loads_only_the_requested_path_even_with_private_witnesses() {
        struct Recording {
            source: CellIndex,
            used: CellIndex,
        }
        impl CellReader for Recording {
            fn read(
                &mut self,
                id: CellID,
                resident: Option<&Arc<Cell>>,
            ) -> Result<Arc<Cell>, CellError> {
                let cell = self.source.read(id, resident)?;
                self.used.insert(Arc::clone(&cell))?;
                Ok(cell)
            }
        }
        let mut dict = Dict::new();
        dict.insert(Scalar::ONE, Value::Scalar(Scalar::from(11u64)));
        dict.insert(Scalar::from(2u64), Value::Scalar(Scalar::from(22u64)));
        let contract = Contract::new(
            Predicate::opaque(Predicate::unspendable_key()),
            Anchor([3; 32]),
            Value::Dict(dict),
        )
        .unwrap();
        let id = contract.id();
        let mut recording = Recording {
            source: CellIndex::collect(Arc::new(contract.to_cell().unwrap())).unwrap(),
            used: CellIndex::new(),
        };
        let root = cells::resolve_cell(&mut recording, id).unwrap();
        let loaded = Contract::from_trusted_cell(&root, &mut recording).unwrap();
        let mut dict = loaded.into_payload().to_dict().unwrap();
        dict.get_resolved(&Scalar::ONE, &mut recording)
            .unwrap()
            .unwrap();
        assert!(recording.used.len() < recording.source.len());
        let cells = Arc::new(recording.used);

        let mut costs = Vec::new();
        for private_overlay in [false, true] {
            let frame = CallFrame::new(Vec::new(), CallKind::ExternalRoot, 1_000_000);
            let mut vm = VM::new(
                TxHeader {
                    version: 1,
                    locktime: 0,
                },
                frame,
            )
            .with_cells(Arc::clone(&cells))
            .unwrap();
            if private_overlay {
                vm.contract_witnesses.insert(id, contract.clone());
            }
            vm.push_value(Value::Cell(
                CellRef::resident(root.clone()).to_unloaded().into(),
            ));
            vm.op_input()
                .expect("input does not eagerly hydrate hidden branches");
            vm.op_signtx().unwrap();
            let Value::Dict(dict) = &vm.current_call.stack[0] else {
                panic!("expected Dict")
            };
            assert_eq!(dict.cached_values().count(), 0);
            for _ in 0..2 {
                vm.push_value(Value::Scalar(Scalar::ONE));
                let before = vm.current_call.gas_used;
                vm.op_getdup().unwrap();
                costs.push(vm.current_call.gas_used - before);
                assert!(matches!(vm.pop_value().unwrap(), Value::Scalar(n) if n == Scalar::ONE));
                assert!(
                    matches!(vm.pop_value().unwrap(), Value::Scalar(n) if n == Scalar::from(11u64))
                );
            }
            vm.push_value(Value::Scalar(Scalar::from(2u64)));
            assert!(matches!(
                vm.op_getdup(),
                Err(VMError::Cell(CellError::MissingCell(_)))
            ));
        }
        assert_eq!(&costs[..2], &costs[2..]);
        assert!(costs[0] > costs[1]);
    }

    #[test]
    fn contract_and_branch_bodies_use_the_committed_bag_and_private_overlays() {
        let branch = ScriptBuilder::new()
            .drop_() // unit argument
            .drop_()
            .alloc(Some(Scalar::from(9u64)))
            .drop_()
            .push_int(0u64)
            .return_();
        let tree = PredicateTree::scripts_only(vec![branch.to_bytecode()], [7; 32]).unwrap();
        let proof = tree.opening_for(0).unwrap();
        let contract = Contract::new(
            Predicate::tree(tree),
            Anchor([1; 32]),
            Value::Scalar(Scalar::ONE),
        )
        .unwrap();
        let program = ScriptBuilder::new()
            .with_script_witness(branch)
            .unwrap()
            .push_literal(crate::CellValue::contract(contract).unwrap())
            .input()
            .push_point(*proof.internal_key.as_bytes())
            .push_cell(proof.root)
            .push_int(100_000u64)
            .push_int(0u64)
            .open()
            .verify()
            .drop_();
        let header = TxHeader {
            version: 1,
            locktime: 0,
        };
        let pc = PedersenGens::default();
        let result = Prover::prove(&pc, program, header, 1_000_000).unwrap();
        let proof = result.proof.as_ref().unwrap();
        let verified = Verifier::verify_cell_with_cells(
            &pc,
            result.program.clone(),
            proof,
            header,
            1_000_000,
            None,
            &result.cells,
        )
        .unwrap();
        assert_eq!(result.txid, verified.txid);
        assert_eq!(result.gas_used, verified.gas_used);
        assert!(matches!(result.txlog[1], TxEntry::Input(_)));
        assert!(Verifier::verify_cell_with_cells(
            &pc,
            result.program.clone(),
            proof,
            header,
            1_000_000,
            None,
            &CellIndex::new()
        )
        .is_err());
        let mut extended = result.cells.as_ref().clone();
        extended
            .insert(Arc::new(Cell::new(vec![99], vec![]).unwrap()))
            .unwrap();
        assert!(Verifier::verify_cell_with_cells(
            &pc,
            result.program,
            proof,
            header,
            1_000_000,
            None,
            &extended
        )
        .is_err());
    }

    #[test]
    fn cell_resolution_has_identical_gas_for_ram_actor_storage_and_witnesses() {
        let cell = Arc::new(Cell::new(vec![7; 128], vec![]).unwrap());
        let bag = CellIndex::collect(cell.clone()).unwrap();
        let empty = CellIndex::new();
        let mut costs = Vec::new();
        for (reference, actor, external, address_sender) in [
            (Some(CellRef::Resident(cell.clone())), &empty, &empty, None),
            (
                Some(CellRef::resident(cell.clone()).to_unloaded()),
                &bag,
                &empty,
                None,
            ),
            (
                Some(CellRef::resident(cell.clone()).to_unloaded()),
                &empty,
                &bag,
                None,
            ),
            (None, &bag, &empty, None),
            (None, &empty, &bag, None),
            (None, &empty, &empty, Some(&bag)),
        ] {
            let mut gas = 0;
            let mut resolved_cells = BTreeSet::new();
            let mut resolver = ExecutionCells {
                external,
                actor,
                address_sender,
                resolved_cells: &mut resolved_cells,
                gas_used: &mut gas,
                gas_limit: 1000,
            };
            let read = match reference {
                Some(reference) => read_cell(&mut resolver, &reference),
                None => cells::resolve_cell(&mut resolver, cell.id()),
            };
            assert_eq!(read.unwrap().id(), cell.id());
            costs.push(gas);
        }
        assert_eq!(costs, vec![131; 6]);
    }

    #[test]
    fn cell_read_warmth_uses_physical_ids_and_resets_for_each_execution() {
        let cell = Arc::new(Cell::new(vec![7; 128], vec![]).unwrap());
        let same = Arc::new(Cell::new(vec![7; 128], vec![]).unwrap());
        assert!(!Arc::ptr_eq(&cell, &same));
        let pruned = Arc::new(cell.prune(1).unwrap());
        assert_eq!(cell.hash(0).unwrap(), pruned.hash(0).unwrap());
        assert_ne!(cell.id(), pruned.id());
        for _ in 0..2 {
            let frame = CallFrame::new(Vec::new(), CallKind::ExternalRoot, 10_000);
            let mut vm = VM::new(TxHeader { version: 1, locktime: 0 }, frame);
            let mut costs = Vec::new();
            for body in [&cell, &same, &pruned, &pruned] {
                let before = vm.current_call.gas_used;
                read_cell(&mut vm.resolver(), &CellRef::resident(body.clone())).unwrap();
                costs.push(vm.current_call.gas_used - before);
            }
            assert_eq!(costs, vec![131, 34, 37, 10]);
            assert_eq!(vm.resolved_cells.len(), 2);
        }
    }

    #[test]
    fn warm_cells_never_supply_bodies_outside_the_current_scope() {
        let body = Arc::new(Cell::new(vec![7; 128], vec![]).unwrap());
        let bag = CellIndex::collect(body.clone()).unwrap();
        let empty = CellIndex::new();
        let mut resolved_cells = BTreeSet::new();
        let mut gas = 0;
        {
            let mut reader = ExecutionCells {
                external: &empty,
                actor: &empty,
                address_sender: Some(&bag),
                resolved_cells: &mut resolved_cells,
                gas_used: &mut gas,
                gas_limit: 10_000,
            };
            cells::resolve_cell(&mut reader, body.id()).unwrap();
        }
        assert!(resolved_cells.contains(&body.id()));
        let before = gas;
        let mut reader = ExecutionCells {
            external: &empty,
            actor: &empty,
            address_sender: None,
            resolved_cells: &mut resolved_cells,
            gas_used: &mut gas,
            gas_limit: 10_000,
        };
        assert_eq!(
            read_cell(&mut reader, &CellRef::resident(body.clone()).to_unloaded()).unwrap_err(),
            CellError::MissingCell(body.id())
        );
        assert_eq!(gas - before, GAS_CELL_READ);
    }

    #[test]
    fn unpaid_or_mismatched_body_reads_do_not_warm_cells() {
        let cell = Arc::new(Cell::new(vec![7; 128], vec![]).unwrap());
        let empty = CellIndex::new();
        for (id, limit, expected) in [
            (cell.id(), 130, CellError::ResourceExhausted),
            ([9; 32], 1000, CellError::CellHashMismatch { expected: [9; 32], actual: cell.id() }),
        ] {
            let mut gas = 0;
            let mut resolved_cells = BTreeSet::new();
            let mut reader = ExecutionCells {
                external: &empty,
                actor: &empty,
                address_sender: None,
                resolved_cells: &mut resolved_cells,
                gas_used: &mut gas,
                gas_limit: limit,
            };
            assert_eq!(reader.read(id, Some(&cell)).unwrap_err(), expected);
            assert!(resolved_cells.is_empty());
        }
    }

    #[test]
    fn cell_read_misses_are_scoped_and_charge_attempts_before_lookup() {
        let empty = CellIndex::new();
        let id = [9; 32];
        let mut gas = 0;
        let mut resolved_cells = BTreeSet::new();
        let mut reader = ExecutionCells {
            external: &empty,
            actor: &empty,
            address_sender: None,
            resolved_cells: &mut resolved_cells,
            gas_used: &mut gas,
            gas_limit: 1000,
        };
        assert_eq!(
            cells::resolve_cell(&mut reader, id).unwrap_err(),
            CellError::MissingCell(id)
        );
        assert_eq!(gas, 1);
        assert!(resolved_cells.is_empty());

        let cell = Arc::new(Cell::new(vec![7], vec![]).unwrap());
        for reference in [Some(CellRef::resident(cell.clone())), None] {
            let mut gas = 0;
            let mut resolved_cells = BTreeSet::new();
            let mut reader = ExecutionCells {
                external: &empty,
                actor: &empty,
                address_sender: None,
                resolved_cells: &mut resolved_cells,
                gas_used: &mut gas,
                gas_limit: 0,
            };
            let result = match reference {
                Some(r) => read_cell(&mut reader, &r),
                None => cells::resolve_cell(&mut reader, cell.id()),
            };
            assert_eq!(result.unwrap_err(), CellError::ResourceExhausted);
            assert_eq!(gas, 1);
        }
    }
}
