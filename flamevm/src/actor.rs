//! Actor data model: identity, state, lifecycle counters, registry.

use cells::{
    Cell, CellBuilder, CellDecode, CellEncode, CellError, CellIndex, CellReader, CellRef, CellSlice,
};
use std::sync::Arc;

use crate::dict::Dict;
use crate::errors::VMError;
use crate::value::Value;

// ── Actor identity and routing ──────────────────────────────────

mod wire {
    use std::convert::TryInto;
    include!(concat!(env!("OUT_DIR"), "/actor_wire.rs"));
}

/// Immutable actor identity: the factual CellID of its original constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActorID(pub [u8; 32]);

impl From<[u8; 32]> for ActorID {
    fn from(id: [u8; 32]) -> Self {
        Self(id)
    }
}

impl ActorID {
    /// Returns the raw identity bytes, without an address-style tag.
    pub fn to_hash(&self) -> [u8; 32] {
        self.0
    }

    /// Actor identities have only one representation.
    pub fn to_canonical(&self) -> Self {
        *self
    }
}

impl CellEncode for ActorID {
    fn encode(&self, builder: &mut CellBuilder) -> Result<(), CellError> {
        wire::ActorID(self.0).encode(builder)
    }
}

impl CellDecode for ActorID {
    fn decode<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        cells: &mut R,
    ) -> Result<Self, CellError> {
        Ok(Self(wire::ActorID::decode(slice, cells)?.0))
    }
}

/// Canonical actor routing operand, defined by `flamevm/actor.ctl`.
#[derive(Clone, Debug)]
pub enum ActorAddress {
    /// Initialize an absent actor using this native constructor graph.
    Init { ctor: CellRef },
    /// Invoke an existing initialized actor.
    Address { addr: ActorID },
    /// Restore authenticated state under an existing actor identity.
    Restore { addr: ActorID, state: CellRef },
}

impl ActorAddress {
    /// Resolves identity without reading the constructor or supplied state.
    pub fn to_actor_id(&self) -> ActorID {
        match self {
            Self::Init { ctor } => ActorID(ctor.id()),
            Self::Address { addr } | Self::Restore { addr, .. } => *addr,
        }
    }

    /// Returns the routing identity bytes, independent of the address style.
    pub fn to_hash(&self) -> [u8; 32] {
        self.to_actor_id().to_hash()
    }
}

impl PartialEq for ActorAddress {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Init { ctor: left }, Self::Init { ctor: right }) => {
                left.commitment() == right.commitment()
            }
            (Self::Address { addr: left }, Self::Address { addr: right }) => left == right,
            (
                Self::Restore {
                    addr: left,
                    state: left_state,
                },
                Self::Restore {
                    addr: right,
                    state: right_state,
                },
            ) => left == right && left_state.commitment() == right_state.commitment(),
            _ => false,
        }
    }
}

impl Eq for ActorAddress {}

impl From<ActorID> for ActorAddress {
    fn from(addr: ActorID) -> Self {
        Self::Address { addr }
    }
}

impl CellEncode for ActorAddress {
    fn encode(&self, builder: &mut CellBuilder) -> Result<(), CellError> {
        let address = match self {
            Self::Init { ctor } => wire::ActorAddress::Init { ctor: ctor.clone() },
            Self::Address { addr } => wire::ActorAddress::Address {
                addr: wire::ActorID(addr.0),
            },
            Self::Restore { addr, state } => wire::ActorAddress::Restore {
                addr: wire::ActorID(addr.0),
                state: state.clone(),
            },
        };
        address.encode(builder)
    }
}

impl CellDecode for ActorAddress {
    fn decode<R: CellReader + ?Sized>(
        slice: &mut CellSlice<'_>,
        cells: &mut R,
    ) -> Result<Self, CellError> {
        Ok(match wire::ActorAddress::decode(slice, cells)? {
            wire::ActorAddress::Init { ctor } => Self::Init { ctor },
            wire::ActorAddress::Address { addr } => Self::Address {
                addr: ActorID(addr.0),
            },
            wire::ActorAddress::Restore { addr, state } => Self::Restore {
                addr: ActorID(addr.0),
                state,
            },
        })
    }
}

#[cfg(test)]
mod address_tests {
    use super::*;

    struct NoReads;

    impl CellReader for NoReads {
        fn read(
            &mut self,
            _: cells::CellID,
            _: Option<&Arc<Cell>>,
        ) -> Result<Arc<Cell>, CellError> {
            panic!("decoding an actor address must not load witness bodies")
        }
    }

    #[test]
    fn actor_id_is_raw32_and_address_tags_follow_ctl() {
        let id = ActorID::from([0x42; 32]);
        let cell = id.to_cell().unwrap();
        assert_eq!(cell.payload(), &[0x42; 32]);
        assert!(cell.refs().is_empty());
        assert_eq!(ActorID::from_cell(&cell, &mut NoReads).unwrap(), id);

        let ctor = Cell::new(
            vec![0xa6, 0xa7],
            vec![Cell::new(vec![0], vec![]).unwrap().into()],
        )
        .unwrap();
        let init = ActorAddress::Init {
            ctor: ctor.clone().into(),
        };
        let encoded = init.to_cell().unwrap();
        assert_eq!(encoded.payload(), &[0]);
        assert_eq!(encoded.refs().len(), 1);
        assert_eq!(encoded.refs()[0].id(), ctor.id());
        assert_eq!(init.to_actor_id(), ActorID::from(ctor.id()));
        let decoded = ActorAddress::from_cell(&encoded, &mut NoReads).unwrap();
        assert_eq!(decoded, init);
        assert_ne!(init, ActorAddress::from(init.to_actor_id()));
        match decoded {
            ActorAddress::Init { ctor: reference } => {
                assert_eq!(
                    reference.as_resident().unwrap().refs()[0].id(),
                    ctor.refs()[0].id()
                );
            }
            _ => panic!("wrong address style"),
        }

        let address = ActorAddress::from(id).to_cell().unwrap();
        assert_eq!(address.payload(), [&[1][..], &id.0[..]].concat());
        assert!(address.refs().is_empty());
        assert_eq!(
            ActorAddress::from_cell(&address, &mut NoReads)
                .unwrap()
                .to_actor_id(),
            id
        );

        let state = Cell::new(vec![], vec![ctor.into(), cell.into()]).unwrap();
        let restore = ActorAddress::Restore {
            addr: id,
            state: state.clone().into(),
        };
        let encoded = restore.to_cell().unwrap();
        assert_eq!(encoded.payload(), [&[2][..], &id.0[..]].concat());
        assert_eq!(encoded.refs().len(), 1);
        match ActorAddress::from_cell(&encoded, &mut NoReads).unwrap() {
            ActorAddress::Restore {
                addr,
                state: reference,
            } => {
                assert_eq!(addr, id);
                assert_eq!(reference.id(), state.id());
                assert_eq!(reference.as_resident().unwrap().refs().len(), 2);
            }
            _ => panic!("wrong address style"),
        }
    }

    #[test]
    fn actor_addresses_keep_pruned_and_unloaded_references_lazy() {
        let cell = Cell::new(vec![0], vec![]).unwrap();
        let pruned = cell.prune(1).unwrap();
        let init = ActorAddress::Init {
            ctor: pruned.clone().into(),
        };
        assert_eq!(init.to_actor_id(), ActorID::from(pruned.id()));
        assert_ne!(init.to_actor_id().to_hash(), cell.id());
        let decoded = ActorAddress::from_cell(&init.to_cell().unwrap(), &mut NoReads).unwrap();
        match decoded {
            ActorAddress::Init { ctor } => assert!(ctor.as_resident().unwrap().is_pruned()),
            _ => panic!("wrong address style"),
        }

        let restore = ActorAddress::Restore {
            addr: ActorID::from([7; 32]),
            state: CellRef::from(cell).to_unloaded(),
        };
        let decoded = ActorAddress::from_cell(&restore.to_cell().unwrap(), &mut NoReads).unwrap();
        match decoded {
            ActorAddress::Restore { state, .. } => assert!(state.is_unloaded()),
            _ => panic!("wrong address style"),
        }
    }

    #[test]
    fn actor_addresses_reject_unknown_tags_and_wrong_field_shapes() {
        for tag in 3..=u8::MAX {
            let cell = Cell::new(vec![tag], vec![]).unwrap();
            assert!(matches!(
                ActorAddress::from_cell(&cell, &mut NoReads),
                Err(CellError::InvalidFormat)
            ));
        }
        for payload in [
            vec![0],
            vec![1],
            vec![2; 33],
            [&[1][..], &[0; 33][..]].concat(),
        ] {
            let cell = Cell::new(payload, vec![]).unwrap();
            assert!(ActorAddress::from_cell(&cell, &mut NoReads).is_err());
        }
        let cell = Cell::new(
            [&[1][..], &[0; 32][..]].concat(),
            vec![Cell::new(vec![], vec![]).unwrap().into()],
        )
        .unwrap();
        assert!(matches!(
            ActorAddress::from_cell(&cell, &mut NoReads),
            Err(CellError::TrailingReferences)
        ));
    }
}

// ── Actor state + code helpers ───────────────────────────────────
//
// An actor is `(code, state)`: an executable Cell graph (set by `setcode`,
// dispatched on the `method` opcode) plus an opaque state `Value` the
// author structures however they like. `op_load` / `op_save` move the
// state; `state_root` / `code_root` are the canonical commitments for
// `TxEntry::ActorSave` / `TxEntry::SetCode`. See ADR 0018.

/// A neutral empty state — an empty Dict. State may be **any** portable
/// `Value`; this is just a convenient default for deploy / tests.
pub fn empty_state() -> Value {
    Value::Dict(Dict::new())
}

/// Canonical state Cell ID, referenced by `TxEntry::ActorSave`.
/// Infallible in valid registry
/// context: op_save gates stored states on portability, and all
/// portable values are wire-encodable.
pub fn state_root(state: &Value) -> [u8; 32] {
    state
        .to_cell()
        .expect("admitted actor state is encodable")
        .id()
}

/// Compiles legacy flat code to native program Cells with explicit continuations.
pub fn code_cell(code: &[u8]) -> Result<Cell, CellError> {
    crate::script::script_cell(code)
}

/// Compatibility decoding for bytecode-only actor APIs, not execution. Only
/// canonical compiler-produced continuation chains can be flattened safely.
pub fn code_from_cell<R: CellReader + ?Sized>(
    root: &Cell,
    reader: &mut R,
    limit: usize,
) -> Result<Vec<u8>, CellError> {
    let mut cell = root.clone();
    let mut bytes = Vec::new();
    loop {
        if cell.is_pruned() {
            return Err(CellError::PrunedCell);
        }
        let mut payload = cell.payload();
        let mut refs = cell.refs().iter();
        let mut next = None;
        while !payload.is_empty() {
            let instruction =
                crate::Instruction::parse(&mut payload).map_err(|_| CellError::InvalidFormat)?;
            let mut encoded = Vec::new();
            if matches!(instruction, crate::Instruction::PushCell(_)) {
                let reference = refs.next().ok_or(CellError::InsufficientReferences)?;
                if payload == [0xa7] && refs.len() == 0 {
                    next = Some(cells::read_cell(reader, reference)?.as_ref().clone());
                    break;
                }
                let literal = cells::read_cell(reader, reference)?;
                if literal.is_pruned() || !literal.refs().is_empty() {
                    return Err(CellError::InvalidFormat);
                }
                crate::Instruction::CellLiteral(std::sync::Arc::new(crate::CellValue::from(
                    literal,
                )))
                .encode_source(&mut encoded);
            } else {
                instruction.encode_source(&mut encoded);
            }
            if encoded.len() > limit.saturating_sub(bytes.len()) {
                return Err(CellError::LimitExceeded);
            }
            bytes.extend_from_slice(&encoded);
        }
        if refs.len() != 0 {
            return Err(CellError::TrailingReferences);
        }
        match next {
            Some(child) => {
                if cell.payload().len() <= 2 {
                    return Err(CellError::InvalidFormat);
                }
                cell = child;
            }
            None => break,
        }
    }
    if code_cell(&bytes)?.id() != root.id() {
        return Err(CellError::InvalidFormat);
    }
    Ok(bytes)
}

/// Legacy source-byte constructor address; native program identities use `Cell::id`.
pub fn code_root(code: &[u8]) -> [u8; 32] {
    code_cell(code)
        .expect("actor code is representable as native program Cells")
        .id()
}

// ── Actor data sizing ─────────────────────────────────────────────

/// Canonical charged size of an actor's code and state. Lease-record overhead
/// belongs to the blockchain host because FlameVM does not own lease records.
///
/// Returns an error if the state can't be encoded. Note:
/// encodability ≠ portability — op_save's `is_portable` check is the
/// authoritative storage gate; this reports the rare case of a portable
/// value that lacks an encoder.
pub fn code_state_bytes(code: &Cell, state: &Value) -> Result<u64, VMError> {
    let mut graph = CellIndex::collect(Arc::new(code.clone()))?;
    graph.extend(&CellIndex::collect(Arc::new(state.to_cell()?))?)?;
    let size = graph.iter().try_fold(0u64, |size, (_, cell)| {
        size.checked_add(cell.record_size() as u64)
            .ok_or(VMError::StorageArithmeticOverflow)
    });
    size
}

/// Result of a successful persistent-storage purchase. The host owns
/// pricing and lease bookkeeping; the VM only turns this deterministic
/// result into a debt token and transaction-log effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoragePurchase {
    pub fee_sparks: crate::Scalar,
    pub expiry_height: u64,
}

// ── ActorRegistry trait ──────────────────────────────────────────

/// Mutable handle into actor state owned by the blockchain state machine.
/// FlameVM never owns the global lease pool or block lifecycle; it only asks
/// this host for deterministic actor reads and mutations while executing one
/// transaction.
///
pub trait ActorRegistry {
    /// True when the actor's combined code/data root is explicitly pruned.
    fn is_frozen(&self, _id: &ActorID) -> Result<bool, VMError> {
        Ok(false)
    }

    /// Authenticates and merges supplied bodies into the actor's current image.
    /// The caller must checkpoint before this mutation: execution failure rolls
    /// hydration and all associated storage changes back together.
    fn restore_with_cells(
        &mut self,
        _id: &ActorID,
        _state: &CellRef,
        _cells: &mut dyn CellReader,
    ) -> Result<(), VMError> {
        Err(VMError::ActorRestoreUnsupported)
    }

    /// Native execution entry; references remain part of the executable graph.
    fn load_program_with_cells(
        &self,
        id: &ActorID,
        cells: &mut dyn CellReader,
    ) -> Result<Cell, VMError> {
        self.load_code_with_cells(id, cells)
    }
    /// Bodies committed as resident for this actor, never a node-global cache.
    fn actor_cells(&self, _id: &ActorID) -> Result<Arc<CellIndex>, VMError> {
        Ok(Arc::new(CellIndex::new()))
    }

    /// Execution-only recovery from the initiating transaction's immutable Cell hierarchy.
    /// Implementations must not implicitly persist externally supplied bodies.
    fn load_code_with_cells(
        &self,
        id: &ActorID,
        _cells: &mut dyn CellReader,
    ) -> Result<Cell, VMError> {
        self.load_code(id)
    }

    fn load_state_with_cells(
        &mut self,
        id: &ActorID,
        _cells: &mut dyn CellReader,
    ) -> Result<Value, VMError> {
        self.load_state(id)
    }

    // ── lookup ─────────────────────────────────────────────────

    /// **Checks out** the actor's state — moves it out of the registry
    /// (leaving the actor `None`/empty) and returns it for `op_load` to
    /// push onto the stack. Errors `ActorEmpty` if it's already checked
    /// out (the re-entrancy lock), or `ActorNotFound`. The matching
    /// `save_state` moves it back.
    fn load_state(&mut self, id: &ActorID) -> Result<Value, VMError>;

    /// Moves `state` back into a **checked-out** actor (`op_save`).
    /// Errors `SaveWithoutLoad` if the actor isn't checked out (saving
    /// would clobber live, possibly token-bearing, state). The caller
    /// ensures `state` is portable (op_save checks first).
    fn save_state(&mut self, id: &ActorID, state: Value) -> Result<(), VMError>;

    /// Returns the actor's native program Cell, method-agnostic — the code itself
    /// dispatches on the `method` opcode. Errors `ActorNotFound` or
    /// `ActorEmpty` (checked out → re-entrancy block).
    fn load_code(&self, actor: &ActorID) -> Result<Cell, VMError>;

    /// Code bytes that [`Self::load_code`] would activate. Used to debit gas
    /// before the host clones or decodes the code.
    fn actor_code_bytes(&self, actor: &ActorID) -> Result<u64, VMError>;

    /// Canonical state bytes that [`Self::load_state`] would activate. Used to
    /// debit gas before checkout or deserialization.
    fn actor_state_bytes(&self, actor: &ActorID) -> Result<u64, VMError>;

    /// Replaces the actor's program graph (`setcode`). Errors `ActorNotFound`.
    fn set_code(&mut self, actor: &ActorID, code: Cell) -> Result<(), VMError>;

    /// Charged bytes currently occupied by code, committed state, and lease
    /// records. While state is checked out this remains the committed usage.
    fn actor_usage(&self, actor: &ActorID) -> Result<u64, VMError>;

    /// Leased bytes available at `height`. A past height and an actor pending
    /// block-boundary destruction are hard errors.
    fn actor_capacity(&self, actor: &ActorID, height: u64) -> Result<u64, VMError>;

    /// Quotes `bytes` without changing state. `None` is the storage opcodes'
    /// in-band unavailable result; arithmetic or invariant failures are hard
    /// errors.
    fn quote_storage(
        &self,
        actor: &ActorID,
        bytes: u64,
        current_height: u64,
    ) -> Result<Option<StoragePurchase>, VMError>;

    /// Atomically buys a one-year lease for `actor`. The implementation must
    /// use the same checks and quote as [`Self::quote_storage`].
    fn purchase_storage(
        &mut self,
        actor: &ActorID,
        bytes: u64,
        current_height: u64,
    ) -> Result<Option<StoragePurchase>, VMError>;

    /// Checks the post-mutation `usage <= capacity(current_height)` invariant.
    /// This is also the final gate for a provisionally deployed actor.
    fn validate_actor_storage(&self, actor: &ActorID, current_height: u64) -> Result<(), VMError>;

    /// True iff a row exists in the registry for `actor`. Used by
    /// `op_call` / message delivery to distinguish "actor doesn't exist"
    /// (hard fail for direct calls; deploy path for constructor messages)
    /// from a temporarily checked-out state.
    fn exists(&self, actor: &ActorID) -> bool;

    // ── checkpoint / rollback (call-frame atomicity) ───────────

    /// Open an undo-log frame onto an internal stack. Called by the
    /// VM at every call-frame entry (and once per tx) so a failed
    /// sub-call can roll back any registry mutations the failed callee
    /// made — `op_save`/`op_load` moves (a checked-out state is just a
    /// `None` the undo restores to `Some`) and deploys. Pairs LIFO with
    /// [`Self::pop_checkpoint_commit`] / [`Self::pop_checkpoint_rollback`].
    fn push_checkpoint(&mut self);

    /// Pop the topmost undo frame and discard it (keep current state),
    /// merging its entries into the parent. Clean call-frame / tx exit.
    fn pop_checkpoint_commit(&mut self);

    /// Pop the topmost undo frame and replay it, restoring registry
    /// state. Called when a call frame fails (`VM::fail_current_call`)
    /// or the outermost tx fails — so a failed `load` restores the
    /// checked-out state (the actor isn't spuriously self-destructed).
    /// Registry-owned pool, lease, and expiry-index mutations are part of
    /// the same checkpoint and must be restored as well.
    fn pop_checkpoint_rollback(&mut self);

    // ── self-destruct (Q6 / ADR 0017) ──────────────────────────

    /// End-of-tx hook for explicit self-destruction: removes actors whose
    /// checked-out state was fully dismantled and returns their ids in
    /// canonical order. Unexpired leases remain owned by the global expiry
    /// index and are not refunded.
    fn commit_tx_destructions(&mut self) -> Vec<ActorID>;

    // ── deployment (Q4 — transparent on first delivery) ────────

    /// Installs a provisional actor with no lease. Its constructor may buy
    /// storage; the containing transaction commits only after
    /// [`Self::validate_actor_storage`] succeeds.
    fn deploy(&mut self, id: ActorID, code: Cell, state: Value) -> Result<(), VMError>;
}
