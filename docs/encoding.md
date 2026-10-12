# Data encoding

FlameVM values, actor storage, and transaction/block transport use the
byte-granular [Cells format](cells.md). Utreexo remains an isolated exception:
its accumulator, Merkle paths, and legacy codecs are unchanged.
Expected types carry no schema tag or version. Sum types have one discriminant;
existing transaction/block protocol versions remain in their headers.

Ordinary Cells carry 0..4095 payload bytes and 0..4 ordered references.
The physical [Cell record and level-dependent hash preimages](cells.md#canonical-cell-record)
are distinct: references retain mask/hash/depth summaries, and `CellID` is the
highest factual hash. Explicit pruning preserves selected lower hashes, not
the factual ID; unloading a body preserves every commitment. Standalone
transport is a root-last DAG packet: `V128` count followed by ordered Cell
records with backward reference indexes. Every referenced physical body is
included; intentional cuts are explicit pruned Cells. Typed `CellEnvelope`
snapshots are ordinary cap-plus-proof-root Cells, with no root-ID prefix.

## Transactions and execution witnesses

The authoritative [CTL schema](../flamevm/tx.ctl) generates the wire codecs at
build time. The layout is:

| Cell | Payload | Ordered child references |
| --- | --- | --- |
| Tx | signature:64 bytes, r1cs_length:u16 LE, r1cs_proof:r1cs_length bytes | TxBody |
| TxBody | version:u32 LE, locktime:u32 LE | program, pruned TxLog |
| program graph container | empty | executable code Cell, execution-witness snapshot |
| pruned TxLog | retained hash(0):32 bytes, depth(0):u16 LE | none |

The executable code Cell contains raw bytecode and up to four literal refs;
there is no program length prefix or Snake decoder. `pushcell` consumes the
next reference, and `exec` replaces the current code Cell in the same frame.
The container keeps lookup availability outside that consuming ref stream.
There is no separate witness field or witness hash in Tx or TxBody.
The execution read context is the union of that snapshot and all inline supplied
program bodies; `ExternalTx::execution_cells()` shares it with induced actor work.
Resident/unloaded normalization during transport cannot change failure behavior.
Transaction decoding restores the complete supplied program graph, even through
an outer block CellEnvelope. Explicit pruned bodies remain pruned. Missing
ordinary bodies are not accepted as an alternative transaction witness set.
`ExternalTx::script()` and the script-byte admission limit refer to the root
code payload. Total transport bytes and execution gas bound the rest of the graph.

An all-zero 64-byte signature means absent authorization; it is decoded as
`None`, never verified as a signature. Any other field must parse as a Schnorr
signature. The verifier rejects absence when `signtx` is used and presence
when no `signtx` authorization was recorded. Explicit `signcall` signatures
remain in the program.

The R1CS proof is inline, not a Snake or child Cell. Its length must fit both
U16 and the remaining 4095-byte payload: at most 4029 bytes. The pinned proof
decoder accepts at most 2497 bytes, so current proofs fit. Caller proof-size
limits are checked before copying proof bytes.

The log child must be a resident, explicit mask-one pruning record. Its full
record is 36 bytes: `01 80`, then its retained hash and LE depth. It is not
decoded as a TxLog. Verification computes the effects and reconstructs this
canonical pruning record from `hash(0)` and `depth(0)`, comparing the complete
commitment. A computed log containing pruned outputs may have higher hash
levels; those do not replace this specified level-zero comparison.

`UnsignedTx::to_cell()` encodes TxBody. `ExternalTx::to_bytes()` transports the
complete Tx root directly, without an additional CellEnvelope wrapper.

Transport and execution have separate scopes:

- The **transport DAG** reconstructs the body, program, signature,
  proof, and witness hierarchy. Its bodies are not implicitly VM witnesses.
- The **execution index** is the immutable set available to the external VM and
  every synchronous call and asynchronous descendant it initiates. It contains
  Contract bodies and optional actor/Dict bodies; inline policy Cells remain
  in the executable graph rather than being duplicated here.

External execution emits `Header` followed by effects. There is no `CellWitness`
entry: the factual program graph is already committed by the body ID.
Removing even an unused execution body changes the signed statement. The index
is fixed before proving; it never includes its own transaction envelope,
avoiding a circular commitment. Private commitment openings and prover-only
instructions do not belong in the public transport or witness snapshot.

### Prover-to-verifier API

Keep witnesses in `ScriptBuilder` until `build_tx`; raw `to_bytecode()` is not
a standalone transaction. `PredicateTree::from_scripts` retains typed branch
programs, including their allocation assignments and nested witnesses.
`push_taproot_proof(&tree, logical_index)` emits the inner Point and partial
policy Cell, and attaches that program's private overlay and nested lookup
witnesses. Unselected programs' witnesses are not collected. Inline policy code
is not duplicated as a separate code Cell in the execution index.

For a Contract protected by `tree`, the public workflow is:

```rust
let unsigned = ScriptBuilder::new()
    .push_literal(CellValue::contract(contract))
    .input()
    .push_taproot_proof(&tree, 0)?
    .push_int(child_gas)
    .push_int(0u64) // no explicit arguments
    .open()
    .verify()
    .drop_() // discard the zero return count from this branch
    .build_tx(header, limits)?;

// When signtx is used, sign unsigned.signing_instructions() instead.
let tx = unsigned.without_signature()?;
let bytes = tx.to_bytes()?;
let decoded = ExternalTx::from_bytes_bounded(
    &bytes, 1, max_script_bytes, max_proof_bytes,
)?;
let verified_log = decoded.verify(limits)?;
```

This example expects the selected branch to consume its payload and return no
values. `UnsignedTx::witnesses()` exposes the frozen public index for inspection;
both `sign` and `without_signature` preserve it. `ExternalTx` carries the index
through transport and passes it to the verifier automatically. There is no
separate witness sidecar for the caller to assemble. The index contains all
explicitly attached paths, including those in conditional code, and freezes
before proving; it is not trimmed according to later execution outcomes.

### Transaction identity

`TxID = TxBody.id()`, `WitnessID = Tx.id()`, and
`EffectID = computed_TxLog.hash(0)`. Signature and R1CS transcripts bind TxID.
Changing top-level signature/proof changes WitnessID, not TxID; changing the
program or supplied execution data changes TxID even when effects are equal.
The log must not contain the body ID, which would create a circular commitment.
Internal execution IDs retain the factual computed-log root, bound to Receive.
Verification re-executes and checks the claimed log hash and depth. Bounded transport
decoders reject trailing bytes and unused outer bodies; unused bodies inside
the committed execution index are allowed.

## TxLog

The root payload is entry_count:u64 LE. A nonempty log has one reference to
a Trie keyed by consecutive u64 big-endian indices. Each leaf references a
separate TxEntry Cell. The sum tag is one byte:

| Tag | Entry | Remaining payload | References |
| ---: | --- | --- | --- |
| 0 | Header | version:u32, locktime:u32 | — |
| 1 | Data | — | snake bytes |
| 2 | Input | ContractID:32 | — |
| 3 | Receive | MessageID:32 | — |
| 4 | Output | — | Contract |
| 5 | IssuePub | qty:Scalar, flavor:Scalar | — |
| 6 | IssuePriv | qty point:32, flavor point:32 | — |
| 7 | Retire | qty point:32, flavor point:32 | — |
| 8 | Fee | sparks:u64 | — |
| 9 | ActorSave | actor ID:32 | state Value |
| 10 | SetCode | actor ID:32 | executable code Cell |
| 11 | Send | — | Message |
| 12 | StoragePurchase | actor ID:32, bytes:u64, expiry:u64, fee:Scalar | — |
| 13 | ActorDestroy | actor ID:32 | — |
| 14 | ActorDeploy | actor ID:32 | constructor code Cell |

Integers in payloads are little-endian unless explicitly stated otherwise.

## Values

| Expected type | Payload | References |
| --- | --- | --- |
| Scalar (formerly Int253) | canonical residue, 32 LE bytes, strictly below the group order | — |
| Point / Predicate | 32-byte compressed Ristretto representation | — |
| Token | qty commitment:32, flavor commitment:32 | — |
| ClearToken | qty Scalar:32, flavor Scalar:32 | — |
| Raw Cell | ordinary payload: 0..4095 bytes; descriptor supplies its length | 0..4 child refs; explicit pruned Cells retain their hash/depth summaries |
| Script | raw instruction bytes, 0..4095; no length prefix | literal refs, including explicit continuation targets |
| Arbitrary blob field | snake: u32 LE total length followed by bytes | next continuation when needed |
| Dict | count:u64 LE, flags:u8 | Trie root unless empty |

Only `Value` adds a tag: Scalar=0, Dict=2, Point=3,
Token=4, ClearToken=6, Cell=13. The removed String tag 1 is reserved and rejected;
no existing tags are renumbered. Fixed-width contents follow inline; Dict
and raw Cell contents are in one child reference. A raw Cell is serialized data,
not authority to instantiate bearer assets. Other tags are rejected. WideToken, the
Contract stack handle, Merlin, Variable, Expression, Constraint, and MSM
have no Value encoding.

`CellValue` is the sole immutable byte/graph container in the VM, holding its
`CellRef` and optional `Arc<CellWitness>` directly. Private Point, Scalar,
Script and Contract annotations are not encoded or included in Cell hashes.
Consumers validate applicable annotations against the authenticated public
bytes before using them. A Contract annotation's public payload remains its
32-byte ContractID; its public body is collected separately into execution
witnesses. A Script annotation retains the existing byte-only `signcall`
source format, not a graph-aware signature-message redesign.

Byte operations accept only zero-ref Cell/Slice/Builder payloads. Literal
construction and Builder growth enforce the ordinary 4095-byte capacity.
Native byte literals use `pushcell` and a child Cell; no inline string opcode
remains. Host `push_literal(CellValue)` retains the byte-only source literal
marker for ordinary zero-ref payloads, lowered before execution. Graph/pruned
literals fall back to a bare `pushcell` in byte-source output; use `to_cell`
or `build_tx` to retain their bodies and references. `push_cell` supports native
graph references, including unloaded ones.
Programs span Cells only by executing `pushcell exec`; instructions do not
straddle Cells. Proofs and other schema-defined blob fields may still use Snakes.
The external Tx's R1CS proof is the explicitly inline exception above.

Dict has a single implementation over `cells::Trie`, including small and
sequential dictionaries. Keys are always present in the trie path: reverse
the canonical 32-byte scalar encoding to obtain big-endian paths, preserving
unsigned numeric order. There is no separate list-mode Dict format.
Flags are sticky portable (bit 0) and droppable (bit 1); other bits are invalid.
An empty Dict has no root and is droppable even if its sticky droppable bit
is false. Pruning does not reset either flag.

Ordinary typed imports validate counts, keys, flags, and reachable values.
Authenticated Contract/Actor reads may trust previously admitted summaries
and leave branches pruned. Access resolves only the required path. Private
prover values can supply commitment openings, but cannot manufacture public
availability. Nested typed imports have an explicit depth bound.

`Contract::from_trusted_cell` and `Value::decode_trusted` skip the complete
Dict walk performed by ordinary `CellDecode`. Their precondition is prior
admission of the owning state and its summaries, not merely knowledge of a
Cell hash. Hash equality proves content identity, not correct capability flags
or ownership. Input membership/spend-once checks remain the chain's job.
`Trie::from_cell(root, key_bytes)` accepts a resident or pruned `CellRef`, or a
`Cell`/`Arc<Cell>` converted into a resident reference. It needs no trusted
count and does not inspect the root; all visited nodes are resolved and
validated on access. Key width is fixed by the owning schema, not encoded in
the Trie. Dict owns its committed entry count;
`Trie::entries_exact(count, resolver)` checks that count on full import. The
constructor establishes neither VM ownership nor admission.

Encoding does not impose portability: a negative ClearToken is encodable.
Domain admission into outputs, actor state, downward calls, or sends enforces
the appropriate rules. Compressed points are validated at cryptographic use.

## Contracts

A UTXO Contract is a Cell whose ID is its ContractID:

```text
payload: predicate:32 | anchor:32 | one tagged Value
refs:    references used by that Value (e.g. one Dict root)
```

A Contract contains **one Value**, not an implicit list of stack arguments.
Use a Dict to hold several values. `contract` and `output` take
`payload predicate`; `signtx` returns that one payload, without an arity.

`input` consumes a 32-byte ContractID, resolves its body from the initiating
execution index, and creates the linear Contract handle. The chain verifies
the spend-once Utreexo proof separately. ScriptBuilder can accept a private
Contract witness, emit its short ID in bytecode, and collect public Cell bodies
without revealing private token openings.

## Predicate branches

A Predicate remains a single 32-byte tweaked key:
`P = X + h(X, program_cell.hash(0))·B`. The root is executable code, not a
native index, a tagged program leaf, or a separate Merkle proof. A one-Cell
policy needs no tree at all. A branching policy decides which child to execute
using ordinary bytecode: `runslice`, indexed `loadref`, and `exec`.

The supplied root may have pruned descendants. Its level-zero hash
authenticates the underlying policy, while its factual highest-level hash
commits the exact supplied graph in TxID. Execution does not virtualize that
graph or hydrate explicitly pruned branches. A pruned root cannot execute;
accessing a pruned child fails with normal child-call recovery.

The optional host `PredicateTree` builds up to four-way executable dispatch
Cells and salted `pushcell drop pushcell exec` leaf trampolines. The first
leaf ref is a secret-derived 32-byte salt Cell, the second the actual program.
Dispatch bytecode selects the first unpruned child using `cellinfo`; it fails
if none is supplied. `opening_for(logical_index)` returns the inner Point and
a root Cell carrying one selected path with all other choices explicitly
pruned. No root/index selector or separate sibling-hash vector is serialized.

`ScriptBuilder::push_taproot_proof` pushes the Point and opening root directly,
retaining only the selected program's private assignments and nested lookup
witnesses. The inline code graph is not duplicated in the execution index.
Private overlays must compile to the exact factual program Cell. Further
bodies are read only when instructions request them.

VM stack form:

```text
contract internal_key program_cell gas args… k  open
  → results… k' 1
  | contract args… k 0       (entered child failed)
```

The child starts with the single payload followed by the explicit arguments.
Invalid commitments, missing/pruned executable roots, or malformed operands
hard-fail the current frame before entry. Failures after entry restore the
original Contract and arguments. Unused synchronous-call gas returns on both
success and failure, but consumed work remains charged. Merlin binds the
Taproot tweak and cryptographic proofs, not raw Cell content addresses.

Replacing the former native program Trie changes helper-built predicate roots,
tweaked points, and corresponding Contract IDs. This is a format change with no
legacy decoder or state migration.

## Actors and messages

ActorID is a sum: `0 | hash:32`, or `1` with a reference to constructor
code as a native program Cell. Both normalize to the constructor code's CellID.

A Message contains anchor:32, ActorID, caller-presence:u8 and optional raw
caller ID:32, refund Predicate:32, gas:u64, and a reference to a Dict of
arguments at consecutive keys. A constructor target uses the preceding
reference for its code. MessageID is this root CellID. The anchor is ratcheted
at every send, so identities are unique by construction.

The actor registry is a 32-byte-key Trie ordered by canonical actor ID.
Its envelope has count:u64 LE and an optional Trie reference. Each entry
contains resident snapshot CellID:32 and one unloaded reference to this actor layout:

| Actor slot | Payload | Ordered references |
| --- | --- | --- |
| Live or frozen | 1:u8, code byte length:u64, state encoded byte size:u64 | executable code Cell, state Value, lease envelope |
| Explicitly destroyed, leases remain | 0:u8 | lease envelope |

The lease envelope contains count:u32 LE and an optional Trie reference.
Keys are expiry height:u64 BE; leaves contain units:u64 LE. Same-expiry leases
are coalesced. `Blockchain::actor_storage` exposes a `StoredActor { root,
cells }` snapshot for witness construction.

The committed residency snapshot indexes actor/lease metadata and the explicitly
retained code/state bodies. Its normal Cell hierarchy represents missing
descendants with fresh pruning records and preserves the original content IDs
through its stored projection cap. Rent counts unique resident code/state Cell record
bytes plus the existing per-lease charge; registry metadata is not charged
again. Frozen actors retain metadata, code/state IDs, lengths, and leases,
but discard code/state bodies. They are not literally represented by only
32 bytes. No tokens are retired by freezing.

Resolution uses attached resident Cells, then the **current actor's** retained
code/state bodies from `ActorRegistry::actor_cells`, then the initiating
execution index. This VM fallback excludes actor-layout and lease metadata;
the registry's `StoredActor` export includes that metadata for storage/witness
construction. Resolution never consults another actor's store or a node-global
cache. Every logical Cell access is charged, including
resident/cache hits. Witness-only reads do not persist bodies; explicit
`save`/`setcode` retain reachable owned changes. General partial-pruning
opcodes and disk adapters are not introduced by this migration.

Existing block-boundary timing is retained: actors losing capacity are
unavailable during that block and have their bodies frozen at its end.
Subsequent execution can use supplied witnesses; persistent restoration must
still satisfy storage capacity.

## Blocks and Utreexo

Block transport is a CellEnvelope. The Block root has empty payload and
references [BlockHeader, transaction sequence]. A BlockTx root contains
gas:u64 LE and references [ExternalTx, Utreexo proof sequence].

Block sequences use count:u32 LE and an optional Trie root, keyed by consecutive
u32 BE indices. Their leaf Cells hold the expected type. The witness root is
the transaction-sequence CellID. An execution record is
`kind:u8 | TxID:32` (0 external, 1 internal, 2 failed/bounced internal);
the effects root is the execution-record-sequence CellID.

The BlockHeader is 212 payload bytes, with no references:

```text
version:u32 | height:u64 | core_block_hash:32 | parent:32
| witness_root:32 | effects_root:32 | Utreexo_root:32 | actor_root:32
| available_storage_units:u64
```

BlockHash is its CellID. Each external transaction immediately drains its
FIFO actor-message closure before the next external transaction, reusing
only its own execution index. A block-wide transport deduplication cannot add
witnesses to any transaction.

Utreexo's existing `Forest`, `Path`, and `Proof` algorithms, hashing, and byte
encodings are unchanged. The `merkle` and `readerwriter` crates remain for
that isolated subsystem. Only outer block transport wraps each unchanged
`Proof` byte encoding in a snake Cell, as a leaf of the block's proof sequence.
FlameVM no longer depends on those crates; its duplicate Chunk/Trie/Dict2
implementations are removed in favor of `cells`. Compact **instruction bytecode** is still
parsed directly from raw program Cell payloads; it is not a second
general-purpose serialization framework.

These are consensus-breaking changes to the migrated encodings and identities.
Old FlameVM/transport bytes and IDs must not be mixed with the new formats;
signatures and proofs bound to changed IDs must be regenerated. This does not
change Utreexo's own proof format.
