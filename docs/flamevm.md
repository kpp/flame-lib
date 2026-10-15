# FlameVM

This is the canonical design and specification of FlameVM. Explanatory sections
give rationale; exact encodings, stack diagrams, and statements using **must** or
**must not** define consensus behavior.

## Introduction

FlameVM implements transaction verifications rules in the Flame blockchain in a form of a Forth-like stack machine.

FlameVM is used in two different contexts: external transactions and internal transactions. External transactions have user-defined results, zero-knowledge proof for confidential transfers and compute, and have access to Utreexo storage: scalable compressed set of unspent transaction outputs. Internal transactions operate on uncompressed storage and invoke multiplayer smart contracts that react to user-defined messages without pre-determined results.

FlameVM operates on multiple data types, including linear types for tokens and zero-knowledge expressions. Types can be “portable” and “non-portable”, “copyable” and “non-copyable”. Portable types can exist in the long-term blockchain state outside of VM execution. Copyable types can be duplicated during VM execution.

Successful VM execution equals to successful transaction verification. Therefore, VM execution encodes both built-in network rules, as well as enables authors to create custom rules within their applications.


## Design overview

Flame supports two forms of smart contract. **Contracts** are immutable,
single-use values held in Utreexo; each locks a payload under a **Predicate**.
**Actors** are persistent identities with state and directly executable code,
entered through asynchronous `send`, synchronous `call`, or read-only `query`. One stack machine
verifies both: external transactions consume and create Contracts and may emit
messages; internal transactions deliver messages and synchronously call Actors.

Values have explicit copy and drop capabilities. Bearer values—including tokens
and Contracts—are linear. Portable values may cross Contract, Message, and
actor-state boundaries; ephemeral verification values such as transcripts,
expressions, constraints, and MSMs may not.

The VM emits an ordered log of state effects. Inputs, outputs, issuance,
retirement, fees, sends, actor saves, storage purchases, actor destruction, and
explicit data are effects. Calls, branches, signatures, constraints, and batch
verification are execution machinery and do not appear in the log. External
TxID commits the body: header, supplied program graph, and a pruned commitment
to the ordered effects. Internal IDs retain their computed effect root.

Every `open`, `signcall`, and actor `call` runs in an isolated frame with its own
stack, control flow, and gas budget. Each invocation passes one portable
application Value, and successful calls return one application Value followed
by status `1`. Use a Dict to group fields, or Scalar `0` as unit when there is
no application input/result. Failed calls restore state effects, constraints, deferred
signatures, fees, and batch-verification work to their entry checkpoints. They
also return entry-owned values: actor calls return their argument, while Contract
predicate calls return the original locked Contract followed by their explicit
argument. Failure status `0` follows those restored values; there is no count.
All downward call arguments must be portable, just like asynchronous `send`
payloads. Read/write return values are unrestricted: non-portable liabilities and
VM-local values may travel upward so the caller can resolve them, but they may
not be delegated to another callee. Read-only actor calls restrict both their
argument and result to recursively data-only portable values; see [Actor queries](#actor-queries).

Actor state is its re-entrancy lock. `load` moves state out of the registry;
while checked out, another frame cannot enter, observe, or mutate that actor.
The loaded value remains exclusively owned by the current frame across nested
interactions and may be saved afterward; it cannot become stale through
re-entrancy. The VM does not interpret actor policy, so the actor must still
account for a callee's status and effects when deciding what state to save.

Persistent actor storage is purchased in one-year leases by the actor itself.
Purchases burn Flame at a deterministic reserve price; storage is neither a
token nor transferable between actors. See [Actor storage](storage.md).

Execution memory is independent of persistent actor storage. There is no
separate memory-limit operand or storage-derived RAM allowance. Variable-size
allocation work is charged to the active frame's gas before allocation; these
charges are monotonic, so the gas cap bounds hostile allocation even after
values are dropped. Calls are bounded to 64 nested frames. Instruction fetch,
EOF checks, and forward label scans consume gas. Synchronous calls return
unused gas on success or failure; consumed work is never refunded.

External transaction effects are atomic and independently verifiable. Internal
transactions execute serially because they share actor state. Expired leases
return to the byte pool; an actor is destroyed when an expiry leaves it with
less capacity than its occupied storage.

All Flame-defined multi-byte integers are little-endian. Decoders reject
alternate-width integers, unordered or duplicate dictionary keys, excessive
nesting, unknown tags, and trailing data where a complete value is required.


## External transactions

External transactions are composed directly by users and have pre-determined outcome. They consume and produce transaction outputs, modifying the set of *unspent transaction outputs.* Each output can be unlocked by one or more signature and the entire transaction contains a zero-knowledge proof of confidential transfers and other constraints on encrypted data imposed by smart contracts.

External transactions are best used for private transactions between few parties.

External transactions produce the following effects:

1. Inputs — consumption of entries from Utreexo that produce *contracts* on VM stack.
2. Outputs — creation of new entries in the Utreexo.
3. Sends — messages sent to actors that produce *internal transactions*.
4. Fee — payment of the transaction fees.
5. Issuance and retirement — creation and removal of tokens to/from circulation.
6. Data entry — for data logging that does not occupy permanent storage.

External TxID is the factual CellID of TxBody: header, program, and a mask-one pruned TxLog reference. Program subcells carry all supplied execution witnesses. The VM recomputes the log and checks its level-zero hash and depth. The Tx root carries the 64-byte signature and U16-length inline R1CS proof; its factual ID is WitnessID. Transaction signatures and ZK proofs bind TxID, covering the code, data availability, and declared effects. An all-zero signature field denotes absent authorization. See [encoding](encoding.md#transactions-and-execution-witnesses).

## Internal transactions

Internal transaction is caused by a “message” sent to an “actor” by an external transaction. Such message does not control the outcome: internal transaction may produce undetermined result or even fail. As such, race conditions (when multiple users send messages to the same actor) can be resolved by the actor itself, accepting all the messages.

Internal transactions are best used for public multiplayer apps, where an actor can be accessed by anyone in any order.

Internal transactions do not have a pre-determined effect and therefore do not support signatures and ZK proofs bound to transaction ID.

Like external, internal transactions produce effects:

1. `Receive(MessageID)` (txlog variant `TxEntry::Receive`) — the consumed Send's id. Emitted automatically after `Header`, committing the whole Message (anchor, target, caller, payload, gas, refund predicate) into the internal effect root. The **single payload Value** is the initial stack item. A selector may be the entire Value or a field of an application-defined Dict; the VM imposes no method layout.
2. Actor activation — first use of `Init` emits `ActorDeploy { actor, code }` with empty initial data; authenticated `Restore` emits `ActorRestore { actor, state }` with the supplied partial code/data proof. Both effects commit only with successful execution and sufficient storage.
3. Outputs — creation of new entries in the Utreexo.
4. Sends — messages sent to actors that produce other internal transactions.
5. Issuance and retirement — creation and removal of tokens to/from circulation.
6. Actor mutations — `ActorSave` carries the full replacement state and `SetCode` carries the full replacement code.
7. Data entry — for data logging that does not occupy permanent storage.
8. Storage purchases — `addstorage` records the actor, purchased bytes, expiry
   height, and burned sparks.

Lease expiry freezes the entire actor code/data root instead of destroying its
linear contents. Actor identity and lease metadata survive. A later explicit
`Restore` may recover part of the committed tree; ordinary witness reads do not
implicitly persist it. Explicit dismantling can still emit `ActorDestroy`. See
[Actor storage](storage.md#global-state-and-block-order).

Each external transaction immediately drains its FIFO message descendants
before the next external transaction, with the same immutable execution witness hierarchy.
Witness bodies from another external transaction are never added to that bag.

Calls themselves are intra-transaction control flow, not effects. Anything a callee does that the outer world cares about appears through one of the effects above.

Canonical internal log shapes are enforced by the state machine. Success is `Header, Receive, effects...`; `ActorDeploy` and `ActorRestore` appear when address resolution performs them, including synchronous callees. Any `ActorDestroy` entries form a suffix. Failed delivery is exactly `Header, Receive, Output(refund)`. Lease expiry freezes the actor image without emitting retirement/destruction effects. External logs start with `Header` and cannot contain internal-only actor effects.

**TxLog transport.** Effects are **re-derived by re-executing** the bytecode under the proof/signature binding; a supplied log does not authorize effects. External TxBody carries only its canonical 36-byte pruning record. `TxEntry` and `TxLog` implement `CellEncode` and `CellDecode` for archival and light-client transport. The log envelope holds a u64-LE count and a Trie of consecutive u64 big-endian indices, whose leaves reference tagged TxEntry Cells (tags 0–15). Blob fields use dedicated zero-ref Cells with at most 4095 raw payload bytes and no length prefix; typed fields use their expected Cell layouts. `TxLog::effect_id()` returns its level-zero hash, not the external TxID. See [Data encoding](encoding.md#txlog).

Internal transactions do not pay transaction-prioritization fees. Actors may
nevertheless burn Flame for storage through `addstorage`. Internal transactions
do not support inputs, as those can be consumed only by external transactions
with a Utreexo proof and, most of the time, a transaction signature.

## Limits

**Transaction gas limit:** the declared `Limits.gas` is the execution budget for
one external transaction. It includes gas permanently reserved by direct
`send` effects. The VM hard-fails before actual use can exceed the declaration,
and the network envelope caps the declaration itself.

**Gas credit:** for one verified external transaction:

```text
direct_send_gas = sum(gas grant of each Send in the derived TxLog)
gas_credit      = gas_used - direct_send_gas
```

`gas_credit` is the actual external execution work. It has a per-transaction
limit and contributes to the block's external-gas total. Direct send grants
contribute instead to the block's internal-gas total. Nested sends are not
counted again: they partition gas already granted to their ancestor message.

**Script size limit:** currently counts the external root code Cell's payload, both
per transaction and across a block. Dynamically loaded actor code is persistent
state and its execution/allocation work is charged in gas; it is not counted a
second time as external witness script. Additional code/literal Cells are bounded
by total transport bytes and execution gas; graph-aware script-size policy remains
an explicit follow-up in [the plan](plan.md).

**Multiplications limit:** `TxMetrics.multiplications` is the exact number of
Bulletproofs multiplication gates in the final constraint system, including
randomized constraints and excluding failed-child work rolled back at a call
boundary. It is limited per transaction and in aggregate per block.

**Issued storage:** `StorageParams.issued_units_per_block` adds exactly that
many units to the global pool at each core block. Storage purchases do not
issue bytes; they move already-issued units from the pool into actor leases.
The storage supply invariant checks initial plus per-block issuance against the
pool and all live leases.

## Fees

Transaction fees are paid by external transactions and are necessary to prioritize common resources on the open network and mitigate denial-of-service attacks. As blockchain imposes limits on storage and computation costs, transactions paying higher fees (per resource used) are prioritised over transactions paying lower fees.
BFT consensus implies that the block candidate is progressively built and already included transactions are not pruned by higher-paying ones.

Transaction fees are paid in Flame and cover computation for the external
transaction and subsequent calls within internal transactions. Actor storage is paid separately by burning
Flame through `addstorage`. 

An external transaction pays for its own execution and prepays every gas grant
attached to its direct `send` effects. An internal transaction executes inside
that grant and prepays any descendant sends from the same budget, so a message
tree can divide gas but cannot amplify it.

Unused gas in a message send is discarded: transaction commits to full amount of gas before doing a message send. Unused gas in a call (within internal transaction) remains with the caller and therefore not lost. 

Every `call`, `open`, and `signcall` has an explicit gas operand. There is no
implicit default grant; a caller that wants to delegate its remaining budget
must read `gas` and pass the desired amount explicitly.

One Flame equals `100_000_000` sparks. Native token quantities and fees are
integral sparks. 

Storage-purchase pricing and issuance are specified in
[Actor storage](storage.md).

## Types

Ownership: every type on the stack is always owned. FlameVM does not allow reference-counting, borrowed stack values or implicit copies. Read-only actor queries retain the same ownership and checkout rules; they restrict effects rather than borrowing actor state.

Plain data types: scalars, immutable Cells, Ristretto points. These can be copied and ported.

Structured data types: dicts that are used as lists, dictionaries and enum variants. Dicts are **never copyable** and cache sticky portability and droppability capabilities (see §Dict).

Bearer types: Contract, WideToken, Token and ClearToken.

Constraint-system types: Variable, Expression and Constraint.

Cryptography types: Merlin transcript. (Batched scalar-point
checks are not user-visible — see the note on `MultiscalarMul`
below.)

Portable types can cross a Contract, Message, actor-call, or actor-state boundary.

Copyable types can be duplicated by the VM. Every copyable value is also
droppable, but non-copyable pure-computation values may be droppable too.

| Type | Copyable | Droppable | Portable |
| --- | --- | --- | --- |
| Scalar, Point | yes | yes | yes |
| Cell (raw serialized data) | yes | yes | yes |
| Slice | yes | yes | no |
| Builder | no | yes | no |
| Dict | no | empty or sticky flag | sticky flag |
| Token | no | no | yes |
| ClearToken | no | quantity is zero | centered quantity is non-negative |
| WideToken, Contract | no | no | no |
| Variable | yes | yes | no |
| Merlin, Expression, Constraint, MultiscalarMul | no | yes | no |

Droppability tracks asset ownership, not whether a value is mutable or useful.
Dropping a transcript, expression, constraint, variable, or unscheduled MSM
abandons computation but cannot destroy bearer value. A Dict starts droppable;
inserting any non-droppable value clears its sticky flag. A fully drained Dict
is nevertheless droppable, allowing the empty container shell to be discarded.

Portability is a business-logic capability, not a serialization property.
Generic value codecs describe representation only. For example, a negative
`ClearToken`, or a Dict containing one, may be encoded and decoded for
diagnostics even though Contract, Message, call, and actor-state admission reject
it. A type may also be non-portable and have no implemented value encoding;
these are separate questions.

Tuples are not distinct types, but a name for passing multiple items on the stack and through calls. During VM execution any tuple is simply a number of items on stack.

Optionals are not distinct types, but a convention to return tuple (values…, 1) or (0). Instruction `verify` can be used to "unwrap optional" and fail immediately if the result is missing.

**Implementation layout (not consensus).** Heap-indirection keeps the Rust
`Value` enum compact without changing VM semantics: `Merlin` boxes its
`Transcript`, `Value::Contract` boxes the otherwise unchanged `Contract`, secret
equality constraints box both `Expression`s, `CellValue` keeps its optional
private `CellWitness` behind a shared `Arc`, and
`spacesuit::AllocatedValue` boxes its optional cleartext assignment. On the
current 64-bit build this makes `Value` 80 bytes, equal to its largest remaining
inline variant (`Token`), while `WideToken` is 40 bytes. Rust layout and byte
counts are implementation details; the canonical wire representation is
unchanged.

Implemented generic `Value` Cell encodings:

| Type | Tag | Representation after the tag |
| --- | ---: | --- |
| Scalar | 0 | canonical 32-byte LE residue |
| Dict | 2 | one reference to count/flags/Trie envelope |
| Point | 3 | compressed Ristretto:32 |
| Token | 4 | qty commitment:32, flavor commitment:32 |
| ClearToken | 6 | qty Scalar:32, flavor Scalar:32 |
| Cell | 13 | one raw serialized Cell reference |

Other Value tags are rejected, including reserved tag 1 (the removed String
variant). Existing tags are not renumbered. An expected concrete type does not repeat a tag.
See [Data encoding](encoding.md) for the complete Cell layouts.

Stack-only types (non-portable and without an implemented `Value` encoding):

| Type | Description |
| --- | --- |
| Slice | Owned shared Cell, byte cursor and ordered remaining refs; copyable and droppable. |
| Builder | Mutable payload/ref buffers; non-copyable, droppable; finalized into a Cell. |
| Contract | Linear contract handle. `Contract` has its own top-level encoding, but `Value::Contract` has no generic value tag. |
| WideToken | Possibly-negative encrypted token tied to the current constraint system. |
| Merlin | Mutable transcript state tied to the current execution. |
| Variable | Copyable handle to a Pedersen commitment or an allocated R1CS variable; not portable. |
| Expression | Linear combination of variables. |
| Constraint | Logical combination of boolean conditions. |
| MultiscalarMul | Lazy `sum(s_i · P_i)` accumulator; consumed by `verify` which appends it to the same batch as Schnorr/Musig sigs (assertion: `sum == identity`). |

### Encoding

FlameVM values use `CellEncode` / `CellDecode`, not the flat Reader/Writer
layer. Cell IDs commit payload and ordered child IDs, independently of
residency. Expected types have no schema/version prefixes. Byte data occupies
ordinary Cell payloads; byte-only operands require zero references. Dict uses one fixed-width Trie format for all
key patterns, with big-endian Scalar paths preserving numeric order.

The encoder does not impose portability. Negative ClearTokens and Dicts with
encodable non-portable members may be encoded. Business-logic admission into
Contracts, actor state, messages, and downward calls checks portability.
Unsupported computation-only variants have no Value encoding.

Typed imports reject malformed tags, noncanonical Scalar residues, invalid
counts/flags/paths, trailing payload, and trailing references. Generic imports
validate the whole supplied value graph, with a depth bound. Authenticated
Contract/Actor reads can retain pruned Dict branches and trust previously
admitted count/capability summaries; a requested missing branch is an error,
not an absent key.

The VM resolves resident references first, then the current actor's explicitly
retained code/state bodies, then the initiating transaction's immutable Cell hierarchy.
Actor-layout and lease metadata are included in registry storage exports, not
in the `actor_cells` execution fallback. No other actor or global cache supplies
implicit witnesses.

Group-element validation remains lazy: decoding a Point or Token preserves its
32-byte compressed representation without decompression. Cryptographic use
performs that validation. This permits byte-identical public/private encoding;
it does not make invalid points usable.

The compact **instruction** grammar is separate from data encoding.
`pushint` still chooses the smallest operand width, and label numbers retain
their offset sub-varint encoding. These instruction bytes are carried inside
raw executable Cell payloads, without a length prefix. Each frame consumes
bytes and literal references independently; `exec` changes the current Cell.

Utreexo is outside this migration: its accumulator, paths, hashing, and legacy
`merkle`/`readerwriter` codecs remain unchanged. Outer block transport merely
stores the existing Utreexo `Proof` bytes as raw payloads in dedicated
zero-ref Cells, without an additional length prefix.

Canonical `pushint` opcode width boundaries:

| Value | Bytecode hex |
| ---: | --- |
| 0 | `00` |
| 15 | `0f` |
| 16 | `1010` |
| 255 | `10ff` |
| 256 | `120001` |
| 65,535 | `12ffff` |
| 65,536 | `140000010000000000` |
| `2^64 - 1` | `14ffffffffffffffff` |
| `2^64` | `1600000000000000000100000000000000` |
| `2^128 - 1` | `16ffffffffffffffffffffffffffffffff` |
| `2^128` | `180000000000000000000000000000000001000000000000000000000000000000` |
| -1 | `1101` |
| -255 | `11ff` |
| -256 | `130001` |
| -65,535 | `13ffff` |
| -65,536 | `150000010000000000` |
| `-(2^64 - 1)` | `15ffffffffffffffff` |
| `-2^64` | `1700000000000000000100000000000000` |
| `-(2^128 - 1)` | `17ffffffffffffffffffffffffffffffff` |
| `-2^128` | `18edd3f55c1a631258d69cf7a2def9de14ffffffffffffffffffffffffffffff0f` |

### Scalar values

Flame scalars are elements of the field modulo the Ristretto255 group order
`ℓ = 2^252 + 27742317777372353535851937790883648493`. Their canonical
representation is the unsigned residue in `[0, ℓ)`, stored as 32 little-endian
bytes. There is no stored sign. Addition, multiplication, and negation are
ordinary field operations modulo ℓ; equality compares canonical residues.

`abs` and `divmod` interpret a residue `r` through its centered lift:
`r` when `r ≤ H`, otherwise `r-ℓ`, where `H = (ℓ-1)/2`. This gives the
integer interval `[-H, H]`. For example, the scalar `ℓ-1` has centered value
`-1`. This interpretation does not change the stored value or impose checked
integer semantics on field arithmetic. Financial code must establish its
own bounds to prevent unintended modular wraparound.

Scalars are used in logical operations: non-zero is “true”, zero is “false”.
Logical results are always 0 or 1. Dict keys are ordered by unsigned canonical
residue, so `ℓ-1` follows all smaller residues rather than preceding zero.

Canonical full-width scalar bytes and Dict key ordering are consensus-visible.
The fixed-width Scalar Cell encoding is not wire-compatible with the former
compact value-tag encoding. Existing state and
bytecode must be regenerated or explicitly migrated before using this format.

### Cell

An immutable Cell handle, represented by `CellValue`. Its `CellRef` identifies
the public graph and may be resident or unloaded; the handle can also point to
an explicit pruning record. Ordinary Cells contain 0..4095 payload bytes and
0..4 ordered child references. The descriptor supplies the payload length,
without a separate byte-string prefix. Byte-only operations require zero refs;
they do not flatten graphs. Reads use Slice and writes use Builder.

There is no separate VM String type. Bounded data, signatures and hashes use
Cell payloads. Blob fields use one zero-ref Cell, at most 4095 bytes, with
no length prefix. Applications define layouts for larger pure-data transfers;
programs use explicit `pushcell exec` continuation. `CellValue::from_bytes` and
`CellValue::script` are fallible, enforcing the single-payload capacity; fixed
32-byte Point, Scalar and Contract-ID constructors are bounded by construction.

**Prover-side witness variants.** `CellValue` holds
`CellRef + Option<Arc<CellWitness>>` directly, without another byte-container
wrapper. The optional annotation preserves typed private data through copies
and value transfers; it is never encoded and never affects Cell identity:

- `CellWitness::Point(Point)` — point-shaped private annotation for byte-oriented commitment consumers such as `mix`; it is not recovered from serialized bytes by `readpoint`. Point-only `commit` uses typed Points directly, or the witness-preserving Points returned by `amount`.
- `CellWitness::Scalar(scalar)` — canonical scalar payload; parse with `readscalar` before `expr`.
- `CellWitness::Script(instructions)` — native executable program graph, retaining private instruction assignments for `signcall` or matching execution overlays.
- `CellWitness::Contract(c)` — used before `input`, carrying open commitments on Token payloads.

The verifier sees the literal's public Cell bytes, without annotations. A
Contract witness emits its actual Contract root and public descendants, while
keeping private openings separately. `input` resolves the authenticated
public path before restoring private atomic witnesses. Commitment/script
consumers validate per-value annotations against public bytes; unrelated hints
do not change parsing semantics. Predicate overlays likewise require matching
public code. Ordinary Cell equality uses factual identity and ignores annotations.
Host witness collection includes program Cells in Contract payloads and cached
nested Dict values, so `exec`/`open` can retain their private assignments. These
overlays never supply an absent public body or hydrate an omitted Dict branch.

**`pushpoint` also carries witnesses.** Because the `pushpoint` instruction's in-memory operand is a `Point` (not a raw `[u8; 32]`), the prover can attach a `Point::Commitment` / `Point::Predicate` witness to a literal point pushed via `pushpoint` — not only via a literal `CellValue::point`. The wire encoding stays the canonical 32 bytes regardless.

### Point

Ristretto255 group element. Stored as compressed 32-byte encoding. Used to represent public keys, Pedersen commitments, and Taproot predicates.

**Prover-side variants.** On the prover side a Point may carry typed witness data while still serializing to the same canonical 32 bytes:

- `Point::Opaque(CompressedRistretto)` — verifier's view; no witness.
- `Point::Commitment(Commitment::Open(value, blinding))` — Pedersen commitment with cleartext opening (used by [`commit`](#commit) / [`expr`](#expr) to skip re-opening).
- `Point::Predicate(PredicateTree)` — Taproot predicate with an executable Cell graph (used by [`contract`](#contract) / [`output`](#output) to attach an unlock witness, and by [`signtx`](#signtx) / [`signcall`](#signcall) for the verification key).

A Point on the value stack downcasts via `to_commitment` / `to_predicate` to extract its witness (preserved through `Point::Commitment` / `Point::Predicate`) or to wrap an `Opaque` as the verifier's `Closed` / `Opaque` form.

### MultiscalarMul

Lazy multi-scalar-multiplication: a vector of `(scalar_i, point_i)` pairs that the VM defers as the assertion `sum(s_i · P_i) == identity`. Non-copyable but droppable, stack-only, and **not** wire-encodable — exactly like [Expression](#types) and [Constraint](#types). Dropping it abandons an assertion that has not been scheduled; only `verify` commits it to the batch.

`evalpoint` is the alternative consumer: compute the public sum immediately and
return a copyable, portable opaque Point, without asserting identity or adding
anything to the verification batch. It is available in external and actor
execution; secret R1CS Expressions are not its operands.

**Purpose: custom Sigma-protocols.** Together with [Merlin](#cryptography-instructions) transcripts (Fiat–Shamir challenges), `MultiscalarMul` lets contract authors express any Schnorr-style relation over Pedersen-committed data — proof of knowledge of discrete log, equality of two encryptions, proof of correct re-encryption, etc. The verification equation always reduces to "this weighted sum of group elements is the identity point".

**Batched verification.** `verify` on an MSM attempts to decompress every point while scheduling the statement and appends each result as `Some(point)` or `None` to the same `BatchVerifier` that holds the transaction's Schnorr / Musig signatures (with `basepoint_scalar = 0` so the MSM contributes only its dynamic terms). Scheduling does not accept or reject the assertion. At finalize, `optional_multiscalar_mul` rejects the entire batch if any point failed decompression or if the weighted sum is not the identity, while still amortising batch work across every Sigma-protocol assertion and signature in the transaction.

**Witness-gated failures are fail-closed.** A few opcodes can fail on the *prover* over data the verifier lacks (e.g. an out-of-`u64` range-proof assignment, a `commit`-then-`expr` over an opaque commitment with no witness). Because the proof binds the **entire** constraint system through Fiat–Shamir, any prover/verifier control-flow or CS divergence — including one caused by such a prover-only failure being caught as a sub-call `0` marker — makes the proof **fail to verify**: the verifier rejects, it can never *accept* an invalid transaction. The effect is a self-inflicted liveness edge (a prover that commits to a malformed witness produces an unverifiable tx), not a soundness break. Making such failures tx-level (uncatchable) so they never reach a marker is a deliberate liveness-hardening item, tracked for ZK review.

**Batch rollback under call failure.** The batch is a verifier-side optimization, not part of the consensus semantics — the on-chain `TxLog` and proof shapes are unchanged. The delegate owns a single `BatchVerifier` for the whole tx, with its RNG. Every opcode that appends to it (today: `op_verify` on a `MultiscalarMul`) calls `BatchVerification::append`, which multiplies the appended statement by a fresh random scalar drawn from the delegate's RNG — Schwartz–Zippel safety against canceling failures. For per-frame rollback the VM snapshots the batch state (basepoint scalar + dyn-arrays length) on the *parent* frame at every call entry (`call` / `open` / `signcall`), via `BatchCheckpoint::snapshot`. On call failure, `fail_current_call` restores the batch via `BatchCheckpoint::restore` — truncating the dyn-arrays and resetting the basepoint scalar — so any MSM appended by the failed callee is dropped. On clean return the snapshot is discarded; the child's appends stay. The RNG state is *not* rewound; random factors sampled between snapshot and restore are simply lost, which is harmless — the remaining batch terms still carry the random factors that were sampled for them.

**Construction.** MSM has no dedicated constructor opcode. Instead, the arithmetic opcodes lift Point/MSM operands implicitly:

| Operation | Result |
|---|---|
| `Point + Point` | MSM with two unit-scalar terms |
| `Point + MSM` / `MSM + Point` | MSM with the point appended (coefficient 1) |
| `MSM + MSM` | concatenated term lists |
| `Scalar * Point` / `Point * Scalar` | MSM with one term `(scalar, point)` |
| `Scalar * MSM` / `MSM * Scalar` | MSM with all coefficients scaled |
| `-Point` | MSM with one term `(-1, point)` |
| `-MSM` | MSM with all coefficients negated |

Quadratic group-element products (`Point * Point`, `MSM * Point`, `MSM * MSM`) hard-fail `TypeNotScalar` — no Sigma-protocol semantics.

**Random factor.** The `BatchVerifier` multiplies each appended statement (MSM, single-sig, multi-sig) by a fresh random scalar before summing, so a failing MSM cannot be cancelled out by other batch members (probability `< 2^-252` per statement). See `starsig::BatchVerification` for the exact construction.

### Dict

Dict is a versatile data structure for representing lists, dictionaries and even sum-type (aka “enum”) values. One-key struct is used to encode a single variant of a sum-type.

Keys are `Scalar` values. Canonical iteration follows unsigned numeric order
of residues in `[0, ℓ)`: a near-order key such as `ℓ-1` sorts after small keys.
All serialized dictionaries use the same `cells::Trie`, with fixed 32-byte
big-endian paths. Sequential keys `0..n-1` are a list convention, not another encoding.

**Runtime representations.** `dict` and host construction create a native ordered
map whose entries are complete. Native insertion, lookup, extraction and ordered
navigation do not encode Values or build/hash Cells. Decoding creates a cell-backed
Dict: its authenticated Trie is authoritative and its typed-value cache may be
incomplete. Cell-backed operations retain resolver-aware path validation and
eager path rebuilding; a missing cache entry is never evidence of an absent key.
Deleting a branch may require resolving its surviving sibling for compression.
There is no deferred-write/tombstone overlay over stored data.

Native serialization builds the canonical Trie from sorted keys without changing
the runtime representation or discarding typed witnesses. Cell-backed serialization
preserves untouched references, including explicit pruning. No private or host
cache can hydrate a publicly missing branch. Serialization never makes subsequent
native access more expensive or changes its availability semantics.

**Dict gas.** Each native lookup, insertion, replacement, removal or ordered
navigation charges `1 + ceil(log2(n+1))`, where `n` is the entry count before the
operation, in addition to the instruction and applicable allocation/copy charges.
`dict` charges this for each insertion. Native operations do not prepay Value
encoding; commitment/admission boundaries do. They create no synthetic Cell
reads. Cell-backed operations pay the [cold/warm Cell-read tariff](#cell-read-gas)
for each visited Cell, including when a typed value is already cached. The first
successful body read warms that physical CellID for this transaction; repeated
reads cost less. Host cache residency alone never makes a read warm. This prices
the logical work of each runtime representation, not an imaginary Trie traversal
for a native map.

Native materialization adds a conservative path-work bound, without constructing
Cells to estimate it: `n * 2*(h+1) * (38 + r*(3 + 34*(1+m)))`, or zero if empty.
Here `h = min(n, 1+d)`, `d` counts radix-4 key positions that differ from the first
key, `r = min(n,4)`, and `m` is the number of bits in the union of public referenced
Cell masks (including nested cached Dict values and backed roots). Recursive Value
encoding is charged separately. All arithmetic is checked. Runtime-only values
remain usable until a boundary requires encoding; encoding depth remains bounded.

**Owned runtime depth.** Cheap construction must not permit unbounded recursive
cloning or disposal. Each Dict tracks a conservative owned-depth watermark:
an empty/lazy Dict starts at 1; an inserted or hydrated child Dict contributes
its watermark plus 1, and a Contract contributes its payload's owned depth plus
one Contract wrapper plus the enclosing Dict. Other Values contribute no owned
Dict/Contract nesting. Insertion/hydration above 256 fails `CellError::LimitExceeded`
without transferring the rejected value. While nonempty the watermark never
decreases; fully draining the Dict resets it to 1. It is not serialized. This
runtime bound is distinct from the unchanged typed encoding-depth bound of 128.

**Dicts are never copyable**: `dup`/`getdup` of a Dict value always fails `TypeNotCopyable`. Each Dict carries independent sticky `portable` and `droppable` flags. A newly constructed empty Dict starts with both flags true. Every successful insertion applies `dict.portable &= value.is_portable()` and `dict.droppable &= value.is_droppable()`. Removal and replacement never restore a cleared flag; a rejected strict insertion does not change either flag. Emptiness overrides only droppability: after every member is extracted, the empty Dict can be dropped while the extracted bearer values remain owned by the script. Portability remains sticky because it governs domain admission rather than disposal.

The `dict` / `put` / `replace` opcodes may insert any Value. A non-portable Dict remains usable on the stack, but `contract`, `output`, `send`, `call`, `open`, `signcall`, and actor-state storage reject it at their portability boundary. Checking a Dict is O(1), including when it is nested: inserting a nested Dict reads that Dict's already-cached flag. Dict values are owned and cannot be mutated through an alias, so the cached parent flag cannot become stale.

The Cell envelope serializes count:u64 LE and flags:u8 (portable bit 0,
droppable bit 1), followed by a Trie reference unless empty. Sticky flags
survive encode/decode, including on an empty Dict; emptiness still permits
dropping it. Generic imports validate the count, paths, and all members against
claimed capabilities. Previously authenticated storage can preserve pruned
branches and use the committed summaries without loading every member.

Drilling down the nested dict preserving ownership with `get` and `put` instructions: 

```rust
// Given a struct like this want to drill down "a",
// then "b" and update balance.
{ 1: {2: balance } }

// Program:
1 get 2 get ... put put
```

### Tokens

All token types are non-copyable. `Token`, `WideToken`, and a nonzero
`ClearToken` are non-droppable; a zero-quantity `ClearToken` is droppable
because it carries only a flavor nameplate and no balance.

The native Flame flavor is `FLAME_FLAVOR = 1`. One Flame is exactly
`100_000_000` sparks; all native-token quantities on the stack and wire are
integral sparks.

For clear token quantities, “negative” and “non-negative” refer to the scalar's
centered interpretation. This is a token-domain rule: a centered-negative
`ClearToken` represents debt and remains non-portable until balanced. Flavor
scalars use their full canonical residue range and have no sign restriction.

Clear token merging preserves integer quantities: the sum of the centered
quantities must remain in `[-H, H]`. A merge that exceeds this interval
soft-fails without consuming either token. This token-domain check does not
change ordinary `Scalar` addition, which remains modular.

WideToken: encrypted token without a range proof on quantity (could be negative). Non-portable: cannot be stored.

Token: encrypted token with a proven non-negative quantity. `Token` is portable
by construction: all construction paths must establish the range/provenance
invariant before creating the type. `Value::is_portable` therefore does not
inspect a commitment opening. In particular, portability cannot depend on
whether the prover holds `Commitment::Open` while the verifier holds the same
point as `Commitment::Closed`.

## Actors

An actor's content is one empty-payload root Cell with two ordered references:
**code** and **data**. Freezing always prunes this whole root.
The total actor record references that content root and a separate protocol
Cell holding size metadata and leases; freezing retains the protocol Cell.

- **code** — a native executable Cell graph, set at deploy and replaced by [`setcode`](#setcode). Dispatch is selector-agnostic: the registry returns its root (`load_code`); by convention code dispatches on its **top-of-stack argument**. The VM imposes **no method layout** and reserves nothing. Root payload bytes are accounted on frame entry; nested bodies are independently metered when read. Persistent storage accounts for the retained graph.

- **data** — **any portable `Value`** (Scalar, Cell, Point, Token, Dict, …), encoded in the second child Cell. [`load`](#load) checks it out, [`save`](#save) moves it back; the author structures it however they like.

The immutable ActorID is the constructor's factual CellID. The canonical
destination throughout the VM is an ActorAddress Cell. Its schema, ActorID
encoding and ActorState layout are defined in
[Cell-based encoding](encoding.md#actor-addresses-state-and-messages).

The Restore field is the combined ActorState image, not just its data Value.
Human-readable ActorIDs and other address forms are in [address.md](address.md).
Bare 32-byte payloads and byte-encoded CellEnvelopes are not VM destinations.

| Lifecycle | Init | Address | Restore |
| --- | --- | --- | --- |
| Uninitialized: no actor record | Provision code and empty Dict data, then execute | ActorNotFound | ActorNotFound |
| Initialized: ordinary ActorState root, possibly partial | Execute current code; never reinstall/reset | Execute current code | Authenticate against current image, merge bodies, execute current code |
| Frozen: entire ActorState root pruned | ActorFrozen | ActorFrozen | Authenticate against frozen image, restore selected bodies, execute available code |

The same resolution rules apply to asynchronous delivery and read/write actor-to-actor
`call`. Read-only calls accept only `Address` and never initialize or restore actors.
Init creation and Restore are provisional within the execution
checkpoint: the actor must buy enough leased capacity for retained bodies.
Successful restoration records `ActorRestore` before subsequent effects;
failed resolution or code rolls back restored bodies, purchases and effects.

Explicit Init/Restore witness graphs can be materialized from the caller's
storage for `call`, or the sender's storage when queued by `send`. These reads
are metered and do not grant the callee access to unrelated sender storage.
`send` leaves an unavailable witness reference for delivery to resolve; an
existing Init destination does not need its original constructor body.

Restoration checks the original factual snapshot level and all corresponding
hash/depth commitments, preserving older intentional cuts. Additional proofs
merge monotonically: omitting a previously retained body never erases it.
Unknown, stale or checked-out actors cannot be restored. An actor may accept a
partial proof and buy storage without reading its still-missing data; later
transactions can reveal further branches. Reading an unavailable branch fails
that execution. Restoring witness bodies alone creates no new asset ownership.

An exclusive checkout lock is independent of availability: missing/pruned data
does not mean the actor's state has been checked out. The complete image
reserves one of Cells' fifteen pruning levels for whole-root freezing.

## Actor queries

Every call frame has a `readwrite` permission, **false by default**. External and
internal transaction entry points explicitly grant `true`. Actor `call` requests
write permission; `query` requests none. Child permission is always
`parent.readwrite && requested_readwrite`, so neither a nested `call` nor a tail
`exec` can escape a read-only frame. This permission is independent of the
external/internal arithmetic context and actor identity.

Read-only execution has these consensus rules:

- Entry accepts only an existing `ActorAddress::Address`; `Init` and `Restore`
  are rejected even when their destination already exists. Missing, frozen and
  checked-out actors retain their ordinary failure behavior.
- Both argument and result must be query data: `Scalar`, `Point`, `Cell`, or a
  portable `Dict` whose entries recursively satisfy the same rule. Empty and
  nested Dicts are accepted, including DNS records with multiple fields. Tokens,
  including zero-quantity tokens, and runtime values are rejected at any depth
  with `NonDataInQuery`. This admission rule is independent of VM copyability:
  Dicts still cannot be duplicated by `dup` or `getdup`. Raw token bytes inside
  a Cell remain data and grant no token ownership.
- Cross-boundary Dicts are hydrated and recursively checked, and Cell graphs
  are materialized using the current scoped resolver and gas budget, preserving
  explicit pruning and failing on missing ordinary bodies. Private annotations
  are discarded; returned data does not require another actor's storage cache
  after the query exits.
- `load` checks out state normally and records the factual CellID of its public
  Value encoding. Each matching `save` must put back that exact commitment, or
  fail `QueryStateChanged`. This is the state at that checkout in the current
  execution view, including writes completed before query entry.
- A successful frame must restore every checkout; explicit `return` and implicit
  fall-through fail `QueryStateNotRestored` otherwise. State may contain bearer
  tokens: they remain linear inside the getter and cannot cross its boundaries.
- `input`, `contract`, `output`, `open`, `signcall`, `signtx`, `send`, `log`,
  `issuepriv`, `issuepub`, `retire`, `fee`, `setcode`, and `addstorage` fail
  `ReadOnlyViolation`. `anchor`, `timelock`, and `version` also fail: a getter
  has no transaction authority or transaction-specific input. Pure flavor
  calculations and token inspection remain permitted.
- Read-only saves emit no `ActorSave`. On success as well as failure, the frame's
  registry checkpoint is rolled back, including storage hydration and caches.
  Consumed gas is retained. The caller's anchor is preserved without splitting.

Queries use internal computation rules. Scalar assertions and supported
cryptographic checks complete before a result is accepted; external proof and
batch-verification operations are unavailable. Registry rollback must not erase
verification work needed to justify a successful result. The normal checkout
lock remains authoritative: a getter cannot read an older snapshot of an actor
whose state is currently held by another frame.

**Onchain.** `arg gas addr query` enters a read-only actor frame and produces
`result 1` or restores `arg 0`. It is callable from actor execution, including
other getters, but not from external transactions or Contract predicate frames.
It reads the current transaction's actor-state view and charges the caller's
gas grant. Ordinary `call` from a writable actor retains read/write behavior;
ordinary `call` from a getter inherits these query restrictions.

**Offchain.** The host API is
`flamevm::query_actor(actor, argument, gas_limit, registry, block, cells)` and
returns `QueryResult { value, gas_used }`. The caller supplies one consistent
registry/block snapshot and its explicit Cell lookup scope. The root has the
queried actor's `selfid`, no authenticated caller (`callerid` is zero), no anchor,
and the supplied block height. It returns one data Value through `return`, or
Scalar `0` on clean empty-stack fall-through. It does not create a transaction,
consume a Message, or emit `Receive` or any other committed effect.

The node JSON-RPC method is
`query_actor(id, argument, gas_limit, expected_block?)`, returning
`{ block, height, value, gas_used }`. Argument and result use VM Value
CellEnvelopes encoded as base64. It runs against one current committed tip;
an optional expected block hash must match or the request fails `STALE_BLOCK`.
Historical queries are not implemented. Node limits are 1,000,000 execution gas
and 64 KiB per decoded argument/result envelope, with bounded typed Dict decoding.
This is an offchain resource
budget, not a paid transaction. Responses identify their snapshot but do not
include state proofs; remote results require trust in the responding node.

## Messages

Messages execute asynchronously with one portable payload Value. Each contains a
predicate for bouncing that exact payload in case of actor failure. A message-root
frame must finish with an empty stack; it cannot return to a caller.

Messages are unique by construction, not by a duplicate-message table. Each
accepted `send` embeds the left child of the current anchor ratchet and leaves
the right child for subsequent execution. Calls split disjoint callee and
caller-continuation subtrees for read/write calls; read-only calls preserve the
caller's anchor. A failed delivery rolls back its actor effects and
produces one Contract containing the original payload under `refund_predicate`; it
does not enqueue another Message. Failure to apply that refund rejects the
whole candidate, so the originating Send does not commit.

A Message's payload is immutable and is created through a checked constructor.
Construction rejects a non-portable payload and validates encodability;
checking a Dict is O(1) through its sticky `portable` flag. Message
encoding is representation-only and relies on this prior asynchronous-domain
admission.

Before delivery, the admitted payload is encoded and trusted-decoded from its
public representation, retaining lazy authenticated branches but clearing private
openings and typed caches. This normalization happens before delivery gas is
computed: a locally queued native Dict and the same Message decoded from transport
must have identical execution, gas and pruning failures. Encoding is prepaid at
Message admission; normalization must not read unrelated hidden branches.

Not only users, but also actors can send async messages to each other. This allows an actor to commit intermediate results between transactions.

## Addresses

Human-readable wallet/actor address formatting is specified in
[address.md](address.md), using [Bech32f](bech32f.md). Key derivation is separate
in [FlameKD](flamekd.md). VM routing uses the
[ActorAddress schema](encoding.md#actor-addresses-state-and-messages);
payment formatting does not add another instruction-level destination format.

## Single-Value ABI compatibility

The single-Value call/return convention replaces counted stack arguments and
results; the opcode numbers do not change. Existing programs must be regenerated
for this ABI, including their predicate/code commitments. Message and routing
Address payloads now reference one tagged Value directly, changing their wire
format and IDs; RPC Send payloads are one Value object rather than an array.
No legacy counted-ABI or list-wrapper decoder is provided. Native Dict gas is a
consensus change, although equivalent Dict contents/flags retain their Cell format.

## Storage

Persistent-storage allocation, pricing, leases, expiry, destruction, and
introspection are specified in [Actor storage](storage.md).

**Execution memory.** Persistent storage capacity does not grant execution RAM.
The frame's gas cap is the only execution-resource budget: byte-sized and
collection-sized growth is charged before allocation, and freeing a value does
not refund allocation gas. This deliberately accounts logical work rather than
Rust allocator layout, while still proving a gas-derived upper bound on hostile
growth.


## Anchors

Every new contract and message is **anchored** by a unique 32-byte value embedded in its wire form, so two contracts with the same predicate + payload but different anchors hash to different ids. Uniqueness is the core safety property — without it, contract ids collide across txs and authors can be tricked into operating on the wrong entity.

**Uniqueness source.** Anchors are unique only when they descend from a *spend-once source*: a UTXO consumed via [`input`](#input), or the `anchor` of a Message that triggered an internal tx (which is itself a split-child of an external tx's `op_send`). Contract ids are themselves anchored (`Contract::id = H(predicate, anchor, payload)`), so each input contract's id is unique on the network. Any anchor derived deterministically from such a source remains unique.

**Per-tx slot.** The VM carries a single `last_anchor: Option<Anchor>` for the entire transaction. It is:
- `None` at the start of an external tx — [`input`](#input) is the only way to seed it.
- `Some(M)` at the start of an internal tx, where `M` is the delivering Message's anchor (a split-child from the originating external tx's `op_send`).

Calls split the anchor at entry. `op_call`, `op_open`, and `op_signcall` each split `last_anchor` into `(left, right)` at frame entry. The child's frame starts with `last_anchor = left`; the parent frame stashes `right` in its `post_call_anchor` slot, and `last_anchor` is restored to `right` when control returns (success or failure). This keeps the caller's anchor chain independent of whatever the callee does with its own half.

**Splitting.** Each opcode that produces a new unique-anchored *cross-tx* entity (contract-on-wire, message-to-actor) consumes `last_anchor` and replaces it with a fresh derived value. The split is a single Merlin transcript:

```
t = Transcript::new(b"flamevm.anchor.split");
t.append_message(b"parent", &last_anchor.0);
left  = t.challenge_bytes(b"left",  32);
right = t.challenge_bytes(b"right", 32);
```

The `left` half is embedded in the new entity (contract anchor / MessageID); the `right` half replaces `last_anchor`. Authors never see `left` and `right` separately — the VM picks them automatically at the consume site.

**Sites that consume + split**: [`contract`](#contract), [`output`](#output), [`send`](#send). Each hard-fails `AnchorMissing` if no prior input claim has seeded the tx's anchor.

**Site that seeds without consuming**: [`input`](#input) sets `last_anchor = Anchor(contract.id())` directly (the spent UTXO's id is already unique on the wire — no split needed). Replacing any prior value is intentional: it lets a partial transaction depend only on its own input claim, not on what other parts of the tx contributed before it.

**Sites that split at call entry**: read/write [`call`](#call), [`open`](#open), [`signcall`](#signcall). All three create new call frames and each splits `last_anchor` at entry. The `left` half seeds the child frame's `last_anchor`; the `right` half is held in the parent frame's `post_call_anchor` slot and replaces `last_anchor` when control returns. This makes anchor flow deterministic across call success/failure boundaries — the caller's anchor chain is independent of whatever the callee did with its left half. A [`query`](#query), or `call` inherited from a read-only parent, neither splits nor consumes an anchor and cannot read it.

**Locality across parties.** Each `op_input` *replaces* the anchor unconditionally rather than mixing into a chain. So a multi-party tx where party A claims input A_in and produces outputs, then party B claims input B_in and produces outputs, has each party's output anchors rooted only in their own input id. B's claim wipes A's residue; that's fine because B's outputs derive from B_in's split-children, not from anything A did. A party signing their portion can predict their own output anchors locally from their own input contract ids.

FlameVM splits anchors at the source: every consume site produces two cryptographically independent children, so neither the contract's stored anchor nor the residual anchor can be reused without the matching half of the original split.

## Instruction set

Each instruction is a one-byte **opcode** optionally followed by **immediate data** encoded inline in the bytecode. Stack effects are written in left-to-right bottom-to-top order: in `a b → c`, `b` is the top of the stack on entry, `c` is the top on exit.

**Context column.** The **Ctx** column in the instruction table marks opcodes that fail outside their supported context:
- **ext.** — external execution, including an external `ContractOpen`. Hard-fails
  `ExternalOnly` from internal context. Covers `input`, the constraint-system
  opcodes (`scalar`, `commit`, `alloc`, `expr`), and the CS-consuming
  opcodes (`mix`, `fee`). Branch-polymorphic opcodes (`borrow`, `eq`, `add`,
  `and`, `or`, `range`) keep a blank marker; their CS-branch restriction is in the
  per-opcode prose. `decrypt` also has a blank marker: it batches externally and
  checks immediately internally.
- **actor** — requires a current actor identity. Available only in
  `InternalRoot`, `ActorCall`, and `QueryRoot`, never `ExternalRoot` or `ContractOpen`.
- **pred.ext.** — requires an external `ContractOpen` predicate context.
- **caller** — requires a called frame (`InternalRoot`, `ActorCall`, `QueryRoot`, or
  `ContractOpen`) with caller attribution; excludes `ExternalRoot`.
- **int.** — internal chain context. Currently used only for planned chain-info
  operations.
- *(blank)* — works in either context. Most opcodes, including `send`.

These context markers do not grant write permission. The restrictions in
[Actor queries](#actor-queries) additionally apply whenever `readwrite` is false.

| Hex | Name | Ctx | Stack | Description |
| --- | --- | --- | --- | --- |
|     | **Stack**  | | | |
| 0k | [push:k](#pushk-and-friends) | | ø → scalar | Push a small literal scalar 0–15 inline. |
| 10–18 | [pushint8/16/64/128 \[s\], pushint](#pushk-and-friends) | | ø → scalar | Push a scalar with a canonical width-class payload. |
| 19 | Reserved | | | Former pushstr; native execution rejects it. |
| 1a | [pushpoint](#pushpoint) | | ø → point | Push a literal 32-byte Ristretto point. |
| 1b | [pushtoken](#pushtoken) | | flv → token | Mint a zero-qty `ClearToken` of the given flavor (placeholder). |
| 1c | [drop](#drop) | | x → ø | Discard a droppable value off the top of the stack. |
| 1d | [nop](#nop) | | ø → ø | Do nothing — useful as a padding / alignment hook. |
| 1e | [dup](#dup) | | x\_k … x\_0 k → x\_k … x\_0 x\_k | Copy the value at depth `k` onto the top (`k` popped as scalar). |
| 1f | [roll](#roll) | | x\_k … x\_0 k → x\_{k-1} … x\_0 x\_k | Move the value at depth `k` to the top (`k` popped as scalar). |
| 2k | [dup:k](#dupk) | | x\_k … x\_0 → x\_k … x\_0 x\_k | One-byte `dup` with `k` ∈ 0..=15 baked into the opcode. |
| 3k | [roll:k](#rollk) | | x\_k … x\_0 → x\_{k-1} … x\_0 x\_k | One-byte `roll` with `k` ∈ 0..=15 baked into the opcode. |
|    |  **Cell bytes**  | | | |
| 40 | [readuint](#readuint) | | s n → s' x 1 \| s 0 | Read 0..32 bytes from a Slice as a canonical LE Scalar. |
| 41 | [readscalar](#readscalar) | | s → s' x 1 \| s 0 | Read a full canonical 32-byte LE Scalar from a Slice. |
| 42 | [readbytes](#readbytes) | | s b n → s' b' 1 \| s b 0 | Copy N Slice bytes into a Builder; overflow hard-fails. |
| 43 | [readpoint](#readpoint) | | s → s' p 1 \| s 0 | Read 32 opaque Point bytes from a Slice; preserve refs. |
| 44 | [writeuint](#writeuint) | | b x n → b' | Write the low 0..32 bytes of a Scalar into a Builder. |
| 45 | [writescalar](#writescalar) | | b x → b' | Write the full canonical 32-byte LE Scalar into a Builder. |
| 46 | [appendbytes](#appendbytes) | | s b → s' b' | Append all unread bytes; preserve unread refs. |
| 47 | [writezeros](#writezeros) | | b n → b' | Append `n` zero bytes to a Builder; preserve refs. |
| 48 | [bitnot](#bitnot) | | b → b' | Invert Builder payload bytes in place; preserve refs. |
| 49 | [bitor](#bitor) | | b s → b' | OR Builder bytes with Cell/Slice payload bytes of equal length. |
| 4a | [bitand](#bitand) | | b s → b' | AND Builder bytes with Cell/Slice payload bytes of equal length. |
| 4b | [bitxor](#bitxor) | | b s → b' | XOR Builder bytes with Cell/Slice payload bytes of equal length. |
| 4c | [shiftleft](#shiftleft) | | b n → b' | Shift Builder payload bits left in place, discarding displaced bits. |
| 4d | [shiftright](#shiftright) | | b n → b' | Shift Builder payload bits right in place, discarding displaced bits. |
| 4e | [appendrefs](#appendrefs) | | s b → s' b' | Append all unread refs without loading children; preserve unread bytes. |
| 4f | [builder](#builder) | | ø → b | Create an empty Builder. |
|    | **Math & logic**  | | | |
| 50 | [abs](#abs) | | x → \|x\| s | Push the centered magnitude and derived sign (`s` ∈ {0,1}). |
| 51 | [eq](#eq) | | a b → a b {0\|1} or constraint | Equality test — cleartext peek, or lifted Constraint in the CS. |
| 52 | [neg](#neg) | | x → −x | Modular negation of Scalar / Expression / MSM (Point lifts to MSM). |
| 53 | [add](#add) | | x y → z | Add scalars (or Expressions); Point/MSM operands lift to MSM. |
| 54 | [mul](#mul) | | x y → z | Multiply scalars (or CS gate); Scalar·Point or Scalar·MSM lift to MSM. |
| 55 | [divmod](#divmod) | | x z → d r | Centered integer division toward zero — push quotient and remainder. |
| 56 | [mod252](#mod252) | | bytes → scalar | Reduce a ≤64-byte zero-ref payload modulo ℓ. |
| 57 | [not](#not) | | x → y | Logical NOT for scalars; structural negation for Constraints. |
| 58 | [and](#and) | | a b → c | Logical AND for scalars; lifts to Constraint conjunction in CS. |
| 59 | [or](#or) | | a b → c | Logical OR for scalars; lifts to Constraint disjunction in CS. |
| 5a | [size](#size) | | x → x n | Cell payload bytes, Slice unread bytes, Builder used bytes, or Dict entries. |
|    | **Constraints**  | | | |
| 60 | reserved | | | Former `scalar` opcode; rejected during execution. Use `readscalar`, then `expr`. |
| 61 | [commit](#commit) | ext. | point → var | Wrap a commitment Point, retaining its private opening when present. |
| 62 | [alloc](#alloc) | ext. | ø → var | Allocate a fresh, duplicable R1CS variable handle. |
| 63 | [expr](#expr) | ext. | scalar/var → expr | Create a constant or consume a variable handle into a one-term Expression. |
| 64 | [range](#range) | | x n → x | Check a Scalar in either context; range-prove an Expression externally. `n` ∈ 1..=64. |
|    | **Dict**  | | | |
| 70 | [dict](#dict) | | …kv… n → dict | Build a Dict from `n` `(value, key)` pairs already on the stack. |
| 71 | [put](#put) | | dict k v → dict' | Insert `v` at fresh key `k`. |
| 72 | [replace](#replace) | | dict k v → dict' {prev 1 \| 0} | Set `v` at key `k`; push prior value if any. |
| 73 | [get](#get) | | dict k → dict' k v | Remove and return the value at `k` (fails if absent). |
| 74 | [getopt](#getopt) | | dict k → dict' {v 1 \| 0} | Optional `get` — soft-fail if key absent. |
| 75 | [getdup](#getdup) | | dict k → dict {v 1 \| 0} | Peek-copy the value at `k` without removing it. |
| 76 | [first](#first--last--next) | | dict → dict {k 1 \| 0} | Push the smallest key, or `0` for an empty dict. |
| 77 | [last](#first--last--next) | | dict → dict {k 1 \| 0} | Push the largest key, or `0` for an empty dict. |
| 78 | [next](#first--last--next) | | dict k → dict {k' 1 \| 0} | Push the smallest key strictly greater than `k`. |
|    | **Cryptography**  | | | |
| 80 | [transcript](#transcript) | | label → merlin | Open a new Merlin transcript seeded with `label`. |
| 81 | [twrite](#twrite) | | m label bytes → m | Absorb zero-ref byte payloads into a transcript. |
| 82 | [tread](#tread) | | m label b n → m b' | Append `n` challenge bytes directly into the Builder. |
| 83 | [sha256](#sha256) | | s b → b' | Append a 32-byte SHA-256 payload digest into the provided Builder. |
| 84 | [sha512](#sha512) | | s b → b' | Append a 64-byte SHA-512 payload digest into the provided Builder. |
| 85 | [sha3](#sha3) | | s b → b' | Append a 32-byte SHA3-256 payload digest into the provided Builder. |
| 86 | [keccak256](#keccak256) | | s b → b' | Append a 32-byte Keccak-256 payload digest into the provided Builder. |
| 87 | [log](#log) | | bytes → ø | Emit a zero-ref Cell/Slice/Builder payload as a transaction-log data entry. |
| 88 | [cellhash](#cellhash) | | c level b → b' | Append the cached Cell hash at an explicit level; no body read. |
| 89 | [evalpoint](#evalpoint) | | msm → point | Compute public group arithmetic immediately, at 4× its batched-verification gas price. |
|    | **Tokens** | | | |
| 90 | [amount](#amount) | | t → t qty flv | Peek the quantity and flavor of a token without consuming it. |
| 91 | [issuepriv](#issuepriv) | pred.ext. | qty tag → T | Mint a confidential token under the current predicate's identity + `tag`. |
| 92 | [issueprivflv](#issueprivflv) | | pred tag → scalar | Consumer-side helper: recompute `flavor_from_predicate(pred, tag)`. |
| 93 | [issuepub](#issuepub) | actor | qty tag → CT | Mint a cleartext token under the current actor's identity + `tag`. |
| 94 | [issuepubflv](#issuepubflv) | | cid tag → scalar | Consumer-side helper: recompute `flavor_from_actor(cid, tag)`. |
| 95 | [retire](#retire) | | t → ø | Burn a token (emits a retire entry to the txlog). |
| 96 | [borrow](#borrow) | | qty flv → −T +T | Borrow balanced ±token pair; debt must be balanced before tx end. |
| 97 | [merge](#merge) | | a b → {c 1 \| a b 0} | Combine two same-flavor cleartokens; soft-fail on flavor mismatch or centered quantity overflow. |
| 98 | [split](#split) | | a q → a' b | Split quantity `q` off a cleartoken. |
| 99 | [mix](#mix) | ext. | tokens… cmts… m n → tokens | Cloak: prove `m` input tokens balance `n` output commitments per flavor. |
| 9a | [decrypt](#decrypt) | | T f f' q q' → CT | Open an encrypted Token to a ClearToken using cleartext openings. |
| 9b | [fee](#fee) | ext. | qty → −WT | Pay tx fee in the native Flame flavor; push the balancing WideToken debt to net out via `mix`. |
|    | **Control flow** | | | |
| a0 | [verify](#verify) | | x → ø | Assert: hard-fail if scalar is zero, enforce a Constraint, or batch an MSM. |
| a1 | [label](#label) | | ø → ø | Mark a jump target (operand: label number); labels number 0,1,2… in order. |
| a2 | [jump](#jump) | | ø → ø | Unconditional jump to a label (operand: label number). |
| a3 | [jumpif](#jumpif) | | x → ø | Pop a scalar; jump to a label iff non-zero (operand: label number). |
| a4 | [return](#return) | | result → ø | Exit current call frame, returning one Value to the parent or offchain query host. |
| a5 | [type](#type) | | x → x code | Push the type code of the top value (peek). |
| a6 | [pushcell](#pushcell) | | ø → cell | Consume the next reference from the current program Cell. |
| a7 | [exec](#exec) | | cell → ø | Replace the current program Cell, preserving this frame. |
| a8 | [slice](#slice) | | c → s | Resolve an ordinary Cell and create an owned Slice. |
| a9 | [endcell](#endcell) | | b → c | Finalize a Builder, computing its Cell hashes. |
| aa | [loadref](#loadref) | | s index → s' c | Remove any indexed remaining reference; do not load the child body. |
| ab | [cellinfo](#cellinfo) | | x → x bytes refs depth level pruned | Inspect a Cell, Slice tail or Builder; preserve the operand. |
| ac | [runslice](#runslice) | | ø → s | Copy the current program's unread byte/ref cursors after this instruction. |
| ad | [writeref](#writeref) | | b c → b' | Append one Cell reference without loading its body. |
| ae | [writepoint](#writepoint) | | b p → b' | Append a Point's 32 public compressed bytes. |
|    | **Contracts & predicates** | | | |
| c0 | [input](#input) | ext. | cell → contract | Decode an admitted Contract root through the execution witness hierarchy; its CellID is the input ID. |
| c1 | [contract](#contract) | | payload pred → contract | Lock one portable Value under the predicate. |
| c2 | [output](#output) | | payload pred → ø | Like `contract`, but emits an Output. |
| c3 | [open](#open) | | contract ik program gas arg → {result 1 \| contract arg 0} | Authenticate the program Cell's hash(0) and run an isolated frame. |
| c4 | [signtx](#signtx) | ext. | contract → payload | Authorize TxID and unwrap the single payload Value. |
| c5 | [signcall](#signcall) | | contract script sig gas arg → {result 1 \| contract arg 0} | Run a script signed by the contract predicate in an isolated frame. |
|    | **Actors** | | | |
| d0 | [send](#send) | | arg refund gas addr → ø | Queue an asynchronous actor message. |
| d1 | [call](#call) | actor | arg gas addr → {result 1 \| arg 0} | Synchronously call an actor. |
| d2 | [load](#load) | actor | ø → value | Check out the actor's state (any portable Value; moves it out, locks re-entry). |
| d3 | [save](#save) | actor | value → ø | Move the state value back in (requires checkout; unlocks). |
| d4 | [setcode](#setcode) | actor | cell → ø | Replace the actor's native code graph (author-gated upgrade). |
| d5 | [addstorage](#addstorage) | actor | q → {debt 1 \| 0} | Buy `q` bytes for one storage year and return a negative Flame token. |
| d6 | [quotestorage](#quotestorage) | actor | q → {fee 1 \| 0} | Quote the positive integral storage fee in sparks without reserving bytes. |
| d7 | [query](#query) | actor | arg gas addr → {result 1 \| arg 0} | Synchronously query an existing actor without write permission. |
|    | **Frame introspection** | | | |
| e0 | [selfid](#selfid) | actor | ø → c | Push the current actor's id in a 32-byte zero-ref Cell. |
| e1 | [anchor](#anchor) | | ø → c | Push the current frame's anchor in a 32-byte zero-ref Cell. |
| e2 | [callerid](#callerid) | caller | ø → c | Push the direct caller actor's id in a Cell, or zero bytes when none. |
| e4 | [gas](#gas) | | ø → n | Push remaining gas budget for the current call. |
| e5 | [gaslimit](#gaslimit) | | ø → n | Push the call's total gas budget cap. |
| e6 | [usage](#usage) | actor | ø → n | Push the actor's currently occupied storage bytes. |
| e8 | [capacity](#capacity) | actor | h → n | Push actor storage capacity available at current or future core-block height `h`. |
|    | **Tx & chain info** | | | |
| f0 | [timelock](#timelock) | | ø → n {0\|1} | Push tx locktime and a flag for height (`0`) vs. timestamp (`1`). |
| f1 | [version](#version) | | ø → n | Push tx version. |
| f2 | [height](#height) | | ø → n | Push the current core-block height, or zero in an external transaction. |
| f3 | [blockhash](#blockhash) | int. | h → s | Push the block hash at height `h`. *planned* |
| f4 | [blockburn](#blockburn) | int. | h → n | Push satoshis burned at height `h` (Bitcoin-coupled). *planned; maturity 100* |
| f5 | [blockweight](#blockweight) | int. | h → n | Push block weight at height `h`. *planned; maturity 100* |
| f6 | [blockrate](#blockrate) | int. | h → n | Push sparks-per-satoshi mint rate at height `h`. *planned; maturity 100* |
| f7 | [chainstate](#chainstate) | int. | n → dict | Push a dict of block stats at height `n`. *planned; maturity 100* |

Opcodes marked *planned* are reserved in the target byte map but are not yet
implemented with the specified behavior. The storage opcodes and `height` are
implemented; bytes `f3` through `f7` remain reserved and currently fail
`UnknownOpcode`.

### Failure modes

A **hard fail** aborts the current call.

If that call has a parent, the VM rolls the child back and converts the failure
to the call opcode's documented in-band failure shape. At the outermost frame,
the error aborts the transaction or offchain query. Synchronous failure never exposes the failed
child's mutated stack: it exposes only the entry escrow after rollback.

A **soft fail** is an in-band signal: the opcode pushes an optional shape
`{value 1 | 0}` so the script can branch. Its stack diagram is authoritative
about whether failed operands are retained; storage request opcodes consume `q`
on either branch. The two kinds are noted per opcode.

Canonical failure-shape vectors (stack order is bottom-to-top):

| Opcode and absent/failing input | Output stack | Consumed on zero branch |
| --- | --- | --- |
| `readuint`: `Slice(aa), 2` | `Slice(aa), 0` | count |
| `readscalar`: `Slice(aa×31)` | `Slice(aa×31), 0` | nothing else |
| `readbytes`: `Slice(01), Builder(), 5` | `Slice(01), Builder(), 0` | count |
| `readpoint`: `Slice(00×31)` | `Slice(00×31), 0` | neither cursor advances |
| `replace`: `{ }, 5, 7` (no prior value) | `{5: 7}, 0` | key and new value enter the Dict |
| `getopt`: `{ }, 5` | `{ }, 0` | key |
| `getdup`: `{ }, 5` | `{ }, 0` | key |
| `first` / `last`: `{ }` | `{ }, 0` | nothing else |
| `next`: `{1: 7}, 1` | `{1: 7}, 0` | search key |
| `merge`: `CT(1,7), CT(2,8)` | `CT(1,7), CT(2,8), 0` | neither token |
| `merge`: `CT(H,7), CT(1,7)` | `CT(H,7), CT(1,7), 0` | neither token (`H = (ℓ-1)/2`) |
| unavailable `addstorage` / `quotestorage`: `q` | `0` | request `q` |
| failed entered `call`: `arg` | `arg, 0` | address and gas; original argument restored |
| failed entered `query`: `arg` | `arg, 0` | address and gas; original data argument restored |
| failed entered `open`: `Contract, arg` | `Contract, arg, 0` | proof/script/gas operands; locked Contract and original argument restored |
| failed entered `signcall`: `Contract, arg` | `Contract, arg, 0` | script/signature/gas operands; locked Contract and original argument restored |

Thus count and lookup-key operands do not need restitution: they are copyable
control data, not linear values. The source Slice, Dict, tokens, Contract, and call
arguments follow the exact shapes above.

### Stack instructions

### push:k and friends

ø → _scalar_

Pushes an [`Scalar`](#scalar-values). The encoder picks the narrowest of these forms:

- `0x00..=0x0f` — immediate `push:k`, value `k ∈ 0..=15`, no payload.
- `0x10`/`0x11` — `pushint8`, 1-byte LE payload `m`, producing `m` / `-m mod ℓ`.
- `0x12`/`0x13` — `pushint16`, 2-byte LE payload `m`, producing `m` / `-m mod ℓ`.
- `0x14`/`0x15` — `pushint64`, 8-byte LE payload `m`, producing `m` / `-m mod ℓ`.
- `0x16`/`0x17` — `pushint128`, 16-byte LE payload `m`, producing `m` / `-m mod ℓ`.
- `0x18` — `pushint` full form, the 32-byte canonical residue.

The decoder rejects values that fit a narrower class, including full-width
values with a compact modular-negation form. Zero has only the immediate
encoding; the negating compact forms do not admit a second encoding for it.

### pushcell

ø → _cell_

Consumes the next reference from the current program Cell and pushes a raw
Cell handle. It does not load the referenced body. Missing refs hard-fail
`InsufficientReferences`. Handles are copyable through `dup` and `getdup`,
portable and droppable: encoded data is not authority to create bearer assets.
Backing immutable allocations are shared, not copied. Neither the current
program nor its consuming cursors are exposed on the stack.

The byte and reference cursors are independent. Jump scans do not execute
`pushcell`, and backward jumps do not rewind the reference cursor. Code that
executes `pushcell` again consumes a further ref or fails; it cannot duplicate
an already consumed reference by looping.

### exec

_cell_ → ø

Resolves the supplied Cell through the current read context, checks its full
commitment, and replaces this frame's program. Starts at byte/ref offset zero
with an empty Cell-local label table. Abandons unread bytes and refs in the old
program; those are serialized data, not live stack assets.

The stack, caller/actor identity, external/internal permissions, anchor, gas
budget, call depth and failure escrow are unchanged. `return` still returns
from the original call. Missing/pruned targets and gas exhaustion use that
call's ordinary failure rollback and argument recovery; root failures reject
the transaction. Resident and unloaded targets use the same cold/warm read
tariff; their logical read history, not handle residency, determines the charge.

Thus `pushcell exec` is explicit continuation. No program length prefix,
implicit child traversal or new call frame is involved. The builder
packs straight-line programs at instruction boundaries and emits this pair.
It rejects automatic splitting of labeled programs: labels cannot span Cells.
The transaction commits its original program graph, not the last Cell executed.

### pushpoint

ø → _point_

Reads 32 more bytes and pushes them as a [Point](#point). Bytes are not decompressed eagerly — invalid Ristretto encodings only fail when consumed by a downstream opcode.

### pushtoken

_flv_ → _token_

Pops a `Scalar` flavor; pushes a zero-quantity `ClearToken { qty: 0, flv }`. Convenience for downstream `merge`/`mix` shapes that need a typed placeholder.

### drop

_x_ → ø

Drops a [droppable](#types) value. Hard-fails `TypeNotDroppable` for
asset-bearing values and Dicts whose sticky droppable flag is false. A
non-empty Dict of droppable members and pure-computation values may be dropped.

### nop

ø → ø

No effect.

### dup

_x\_k … x\_0 k_ → _x\_k … x\_0 x\_k_

Pops `k` as `Scalar`, copies the value at depth `k` (zero-indexed from the top) onto the stack. Source must be a [copyable](#types) type — hard-fails `TypeNotCopyable` otherwise. Hard-fails `IndexOutOfRange` when the canonical residue is outside the stack's index range.

Prefer the immediate [`dup:k`](#dupk) form for `k ∈ 0..=15` (one byte instead of two).

### roll

_x\_k … x\_0 k_ → _x\_{k-1} … x\_0 x\_k_

Pops `k` as `Scalar`, moves the value at depth `k` to the top of the stack. Any type works (no copy required). Hard-fails `IndexOutOfRange` when the canonical residue is outside the stack's index range.

Prefer the immediate [`roll:k`](#rollk) form for `k ∈ 0..=15`.

### dup:k

_x\_k … x\_0_ → _x\_k … x\_0 x\_k_

Immediate-encoded `dup` with `k ∈ 0..=15` taken from the low nibble of the opcode byte (`0x2k`). One-byte equivalent of `pushint8 k; dup` — saves a byte over [`dup`](#dup) for shallow depths. Same copyability and bounds rules as `dup`.

### roll:k

_x\_k … x\_0_ → _x\_{k-1} … x\_0 x\_k_

Immediate-encoded `roll` with `k ∈ 0..=15` taken from the low nibble of the opcode byte (`0x3k`). One-byte equivalent of `pushint8 k; roll`. Same bounds rules as `roll`.

## Cell byte/reference instructions

A Slice owns shared immutable data, a byte cursor and an ordered remaining-reference view; a
Builder owns mutable payload/reference buffers. Neither is portable. Cells and
Slices are copyable with ordinary `dup`; Builders are not. All three are
droppable plain serialization data, not authority to instantiate bearer assets.

### builder

ø → _b_

Creates an empty Builder. Payload capacity is 4095 bytes and reference capacity
is four. Ordinary writes never spill or implicitly descend into another Cell.

### slice

_c_ → _s_

Resolves the Cell body in the current metered context and creates an owned Slice
at byte offset zero with all references available. Missing bodies or explicit pruning hard-fail.

### loadref

_s index_ → _s' c_

Removes the reference at the zero-based index in the Slice's **remaining**
references and pushes its Cell handle above the updated Slice. Later references
shift down, preserving their relative order. For `[A, B, C]`, `1 loadref` yields
`[A, C]` and `B`; a subsequent `1 loadref` yields `[A]` and `C`.
Unread bytes and other copies of the Slice are unchanged. Invalid indexes
(including negative indexes or indexes past the remaining refs) hard-fail with
`IndexOutOfRange`; there is no success flag.

The child is not resolved or parsed. Unloaded and explicitly pruned references
are valid results. Use the separate `slice` instruction to read a child's body;
it hard-fails for an unavailable or explicitly pruned Cell, even if a full
version of the underlying data exists elsewhere. Removal performs bounded
inline bookkeeping and charges only the base instruction gas, with no payload
copy, child-body read or heap allocation. Equality and `appendrefs` use only the
references still present in the updated Slice.

### endcell

_b_ → _c_

Consumes the Builder and computes its ordinary Cell and all significant hashes.
Finalization precharges a conservative bound for all sixteen possible hashes.

### cellinfo

_x_ → _x bytes refs depth level pruned_

Peeks a **Cell, Slice or Builder** and pushes five Scalars in this order (`pruned` is on top):

| Result | Meaning |
| --- | --- |
| `bytes` | Cell payload length, Slice unread bytes, or Builder used bytes; excludes descendants. |
| `refs` | Cell immediate refs, Slice remaining refs, or Builder stored refs. |
| `depth` | Factual depth at the highest significant level of these contents. |
| `level` | Highest significant hash level, not the number of set mask bits. |
| `pruned` | `1` for an explicit pruned Cell; `0` for ordinary Cells, Slices and Builders. |

Resolves the Cell's own record through the metered resolver, using the same
cold/warm tariff for resident and unloaded handles. Child bodies are not loaded. The original handle
and its private annotations stay unchanged. Slices use only their unread byte/ref
streams, not the original backing Cell's metadata; Builders use their current
contents. Their level is the highest set bit's level in the union of remaining
child masks (zero for an empty mask), and depth is one
plus the maximum child depth at that level (zero when there are no refs).
Neither hashes payloads nor loads children; charge one gas per immediate ref,
in addition to the instruction cost. Byte/ref cursors, ref order and Builder
buffers are unchanged. Missing records, invalid commitments,
wrong types (`TypeNotByteSource`), and exhausted gas hard-fail before any stats are
pushed. There is no success flag and no change to `size`.

Pruned records can be inspected even though `slice` cannot parse them. They
report `34 * popcount(mask)` payload bytes (retained hashes/depths), zero refs,
factual depth zero, their highest level, and flag `1`. An ordinary Cell with
pruned descendants may have a nonzero level but still reports flag `0`.
These are stats of the supplied factual record, not the hidden logical tree;
lower-level depths retained in a pruning record are not this opcode's `depth`.

### runslice

ø → _s_

Copies the current frame's program Slice **after** the instruction cursor has
passed `runslice`. Its bytes start at the next instruction; its references
exclude those already consumed by `pushcell`. Copies share immutable public
data, have independent byte/ref cursors, and do not contain private assignments.
No body lookup, payload copy or heap allocation is needed; only base instruction
gas is charged. The result is copyable and droppable, but non-portable.

`loadref` can select any remaining reference on this copy without advancing the
actual program's reference cursor. `exec` may enter that Cell in the same frame.
The snapshot remains valid after execution advances, jumps, or replaces its
program; a subsequent `runslice` sees the new current Cell, never caller code.
An end-of-payload snapshot has zero bytes but may still have unread references.

### readuint

_s n_ → _s' x 1_ | _s 0_

Reads `0..=32` whole bytes as an unsigned little-endian canonical Scalar.
Zero bytes produce zero. Short input or an integer at least the scalar order
soft-fails without advancing either cursor. Invalid counts hard-fail.
No masking of partial bytes and no modular reduction occur.

### readscalar

_s_ → _s' x 1_ | _s 0_

Equivalent to `32 readuint`: read the full canonical 32-byte LE Scalar.

### readbytes

_s b n_ → _s' b' 1_ | _s b 0_

Copies exactly N unread bytes into the Builder. Short input restores both
operands and yields zero. Builder overflow hard-fails. Only the byte cursor
advances; unread references remain accessible.

### writeuint

_b x n_ → _b'_

Appends the first `0..=32` bytes of the Scalar's canonical LE representation.
Selected low bytes are truncated, not fit-checked. Invalid counts or capacity
overflow hard-fail; precharge copying before mutation.

### writescalar

_b x_ → _b'_

Equivalent to `32 writeuint`: append the full canonical 32-byte LE Scalar.

### appendbytes

_s b_ → _s' b'_

Appends all remaining Slice payload bytes, exhausts its byte cursor, and preserves
its reference cursor. Capacity overflow hard-fails without partial mutation.

### appendrefs

_s b_ → _s' b'_

Appends all remaining Slice references, exhausts its reference cursor, and
preserves its byte cursor. Child bodies are not resolved. Check the complete
reference capacity and every child's depth metadata before mutation.

### Byte consumers and temporary host compatibility

VM byte consumers accept zero-ref Cells, Slices with no remaining refs, or
Builders with no refs. They consume the supplied container and use only its
payload bytes: there is no implicit traversal, flattening or pruning hydration.
Cell bodies are resolved through the metered context; existing Slices/Builders
are read directly. Pruned Cells and remaining references hard-fail.
There is no String Value or String witness wrapper.
The host `push_literal(CellValue)` helper compiles a child Cell and `pushcell`; 0x19 is only
a compatibility source marker for byte-only host APIs, never a native opcode.
Private witnesses remain outside public Cell bytes. Load loop/branch constants
once and duplicate them: program reference cursors do not rewind on jumps.

### readpoint

_s_ → _s' p 1_ | _s 0_

Reads 32 bytes as a [Point](#point). Soft-fail on insufficient bytes.
The operand must be a Slice. Advance only its byte cursor, leaving refs intact;
short input changes neither cursor. Copies share the Cell allocation and no
suffix buffer is allocated. Point decompression remains a later consumer's job.

### writepoint

_b p_ → _b'_

Append the Point's 32-byte public compressed representation. Preserve Builder
refs and payload prefix. Capacity and gas are checked before writing. Private
commitment/predicate annotations are not serialized or attached to the Builder.

### writeref

_b c_ → _b'_

Append the Cell reference as a child of the Builder, preserving payload bytes.
This enables dynamically constructed Cell DAGs. The operand must be a Cell,
not a Contract or other typed Value. Unloaded and pruned references are allowed;
their bodies are not read. Check reference capacity and child depth metadata,
then charge one reference item before writing. Finalization hashes the graph.

### writezeros

_b n_ → _b'_

Append `0..=4095` zero bytes directly into the Builder, preserving its refs.
Capacity and gas are checked before mutation; no temporary zero buffer is used.

### bitnot

_b_ → _b'_

Invert every Builder payload byte in place. Preserve payload length, capacity
and child references; do not allocate a replacement buffer.

### bitor / bitand / bitxor

_b s_ → _b'_

Combine the Builder payload in place with an equal-length Cell payload or
Slice's unread bytes. A Cell body is resolved through the current metered
context; a Slice does not move its cursors. The source operand is consumed.
Builder sources are not accepted. Source refs must be zero (or already consumed
on a Slice), preventing implicit loss of an unrelated reference stream.
Mismatch hard-fails `BitwiseSizeMismatch`. Destination refs are preserved.

### shiftleft / shiftright

_b n_ → _b'_

Shift the Builder's fixed-length payload as a big-endian bit string (byte zero
is most significant), for `0..=256` bits. Discard displaced bits and fill the
vacated positions with zero. A shift at least the payload bit length produces
all zeroes. Payload length/capacity and references do not change. Invalid
counts hard-fail `IndexOutOfRange`; charge linear byte work before mutation.

## Arithmetic & logic instructions

### abs

_x_ → _|x| s_

Pops `Scalar` `x`; interprets it through the [centered lift](#scalar-values)
and pushes its magnitude followed by the derived sign (`0` for a non-negative
centered value, `1` for a negative one). Thus `abs(0) = (0, 0)`,
`abs(H) = (H, 0)`, and `abs(ℓ-1) = (1, 1)`. The magnitude is at most `H`.

### eq

_a b_ **eq** → _a b {0|1}_ (cleartext) | → _constraint_ (CS branch)

Two stack diagrams depending on operand types and context:

1. **Cleartext branch.** When neither operand is a CS type (`Variable` / `Expression`), or in internal context, peeks at the top two values and pushes `1` if equal or `0` otherwise. Operands stay on the stack. `Scalar` / `Point` compare by value. Any Cell/Slice/Builder pair compares by factual (highest-level) hash: Cells use the cached ID, Slices hash only their unread bytes and refs as an ordinary Cell, and Builders hash their current bytes and refs without finalizing or copying their payload buffers. Child bodies are not loaded for hashing, and private annotations are ignored. Explicit pruning changes factual identity; use `cellhash` at the intended level for semantic comparison. **Dict and all linear types (`Token`, `ClearToken`, `Contract`, …) hard-fail `TypeNotComparable`** when compared with the same type.
2. **Lifted branch.** When at least one operand is `Expression` or `Variable` *and* the context is external, both operands are popped, lifted to `Expression` (Scalar → `Expression::Constant`), and the result is `Constraint::eq(a, b)`.

### neg

_x_ → _−x_

`Scalar` is negated modulo ℓ (`0` stays `0`, and nonzero `x` becomes `ℓ-x`). `Expression` negates the linear combination. `Point` / `MultiscalarMul` lift to MSM (see [MultiscalarMul](#multiscalarmul)) with negated coefficients. Other types hard-fail `TypeNotScalar`.

### add

_x y_ → _z_

Dispatch by operand types:
- `Scalar + Scalar` → addition mod ℓ.
- `Point + Point` / `Point + MSM` / `MSM + Point` / `MSM + MSM` → [MultiscalarMul](#multiscalarmul) with concatenated terms (works in either context).
- Mixed Scalar/Expression in external context → lifts to `Expression` LC sum.

### mul

_x y_ → _z_

Dispatch by operand types:
- `Scalar * Scalar` → multiplication mod ℓ.
- `Scalar * Point` / `Point * Scalar` → [MultiscalarMul](#multiscalarmul) with one `(scalar, point)` term.
- `Scalar * MSM` / `MSM * Scalar` → MSM with scaled coefficients.
- Mixed Scalar/Expression in external context → may add a CS multiplier gate (or constant-fold when one side is `Expression::Constant`).
- `Point * Point`, `MSM * Point`, `MSM * MSM` → hard-fail `TypeNotScalar` (quadratic in group elements, no Sigma-protocol semantics).

### divmod

_x z_ → _d r_

Both operands must be raw `Scalar` values. Let `X` and `Z` be their
[centered lifts](#scalar-values). Computes integer quotient `D` truncated toward
zero and remainder `R`, so `X = D·Z + R`, `|R| < |Z|`, and nonzero `R` has
the same sign as `X`. Returns `D mod ℓ` and `R mod ℓ` as scalars. For example,
`divmod(ℓ-7, 3) = (ℓ-2, ℓ-1)`. Hard-fails `DivByZero` on a zero divisor.
This is integer division, not multiplication by a field inverse; no R1CS
division gadget is allocated. Available in external and internal contexts.

### mod252

_s_ → _scalar_

Consumes a zero-ref Cell/Slice/Builder payload of up to 64 bytes as a
little-endian unsigned integer, reduces modulo ℓ, and pushes a canonical Scalar.
Longer payloads fail `BytesTooLongForModReduction`.

### not

_x_ → _y_

`Scalar`: `0 → 1`, non-zero → `0`. `Constraint`: structural negation via `Constraint::not(c)`.

### and

_a b_ → _c_

Cleartext logical AND when both operands are `Scalar`. When at least one operand is a `Constraint` (external context only), both lift to `Constraint` (Scalar → `Cleartext(v != 0)`) and the result is `Constraint::and(a, b)`.

### or

_a b_ → _c_

Mirror of [`and`](#and) for disjunction.

## Constraint system instructions

These instructions require external context except `range` on a raw `Scalar`,
which performs an ordinary range check in either context.

**Rollback on call failure.** Every CS-touching opcode (`scalar`,
`commit`, `alloc`, `expr`, `range` on a linear combination, `eq` via `verify`, `mix`,
`fee`) appends to the delegate's R1CS. On `call` /
`open` / `signcall` entry the VM checkpoints the R1CS via
`bulletproofs::r1cs::CheckpointableConstraintSystem::checkpoint`;
on call failure the child's CS contributions (witness vectors,
constraint vectors, deferred constraints, transcript state) are
rolled back to the parent's snapshot via `rollback`. Both Prover
and Verifier hit the same checkpoint/rollback sites because they
walk the same script — the transcript stays in lockstep across
the failure boundary. On clean return the snapshot is dropped and
the child's CS contributions remain in the final proof. See
the rollback model described above.

### commit

_point_ → _var_

Consumes a Point and wraps its commitment as a `Variable::Committed`.
`Point::Commitment` retains its opening; ordinary Points and unrelated predicate
annotations yield a closed commitment. For raw bytes use `slice readpoint`
and handle its success flag first. Serialization does not carry private openings:
provers use typed Points or witness-preserving `amount` outputs.
`expr` registers the commitment in the constraint system. Byte-oriented `mix`
output commitments retain their separate authenticated Cell-annotation interface.

### alloc

ø → _var_

Allocates a low-level R1CS variable once. The prover's `Alloc(Some(scalar))`
retains its assignment; the verifier sees `Alloc(None)`. The result is a
copyable, droppable, non-portable `Variable::Allocated` handle. Each `expr`
consumes one handle into a non-copyable Expression referring to that same
allocation. Thus `alloc dup:0 expr roll:1 expr mul` squares one allocated
value without copying an expression tree. To reuse a larger computation,
allocate a variable, constrain one expression of it equal to that computation,
and consume further handle copies in other expressions. Dicts and Expressions
remain non-copyable. An allocated handle has no public commitment and is not
a substitute for a committed Variable in `borrow` or `issuepriv`.

### expr

_scalar/var_ → _expr_

Consumes a Scalar into `Expression::Constant`, without allocating CS variables.
A committed Variable registers its Point with `commit_variable`; an allocated
Variable reuses its existing R1CS handle. Both produce a one-term Expression.
External-only, including the constant case. The redundant `scalar` opcode is
removed: canonical byte parsing is `readscalar`, followed by `expr`.

### range

_x n_ → _x_

Pops bit count `n: Scalar` (must be in `[1, 64]`) and either a raw `Scalar`
or an `Expression`. A raw scalar is checked immediately against `[0, 2ⁿ)` in
both external and internal contexts. It remains a raw scalar on the stack;
the check does not lift it into the constraint system.

Expressions remain external-only, including `Expression::Constant`.
For a constant, checks its canonical residue against `[0, 2ⁿ)`.
For `Expression::LinearCombination`, adds the existing Bulletproofs
range-proof gadget. The original value is pushed back unchanged.
Hard-fails `BitCountOutOfRange`, `InvalidBitrange`, or `R1CSError`;
an Expression in internal context hard-fails `ExternalOnly`.

### size

_x_ → _x n_

Peek the Cell's own payload byte count, a Slice's unread byte count, a Builder's
used byte count, or a Dict's entry count. References and descendant payloads are
not counted. A Cell body must resolve and be non-pruned. Other types
hard-fail `TypeHasNoLength`; the inspected value stays on the stack.

### dict

_… val\_{n-1} key\_{n-1} … val\_0 key\_0 n_ → _dict_

Pops `n` (Scalar), then `n` `(value, key)` pairs (key on top of each pair). Builds a Dict. Hard-fails `DictKeyOccupied` on duplicate keys.

### put

_dict k v_ → _dict'_

Inserts `v` at key `k`. Hard-fails `DictKeyOccupied` if the key already exists.

### replace

_dict k v_ → _dict' {prev 1 | 0}_

Sets `v` at key `k`. If a prior value existed, pushes `prev 1`; otherwise pushes `0`.

### get

_dict k_ → _dict' k v_

Removes the value at `k` and pushes `(k, v)` above the modified dict. Hard-fails `DictKeyNotFound` if absent.

### getopt

_dict k_ → _dict' {v 1 | 0}_

Like [`get`](#get) but soft-fails when absent: pushes `0` instead of erroring.

### getdup

_dict k_ → _dict {v 1 | 0}_

Copies the value at `k` without modifying the dict. Pushes `0` if absent. Hard-fails `TypeNotCopyable` when the value exists but is a linear type.

### first / last / next

_dict_ → _dict {k 1 | 0}_  (first / last)  
_dict k_ → _dict {k' 1 | 0}_  (next)

Iteration helpers. Push the first/last key of the dict, or the key after `k`, or `0` if the dict is empty / `k` is the last entry. Keys are visited in canonical `Scalar` order.

## Cryptography instructions

### transcript

_label_ → _merlin_

Creates a fresh [Merlin transcript](#types) seeded with `label`. The transcript
is never copyable but is droppable; `twrite` / `tread` consume and return it
while building custom ZKP statements.
Labels and absorbed data are zero-ref Cell/Slice/Builder payloads. Slices use
only unread bytes. Containers are consumed; no temporary byte container is created.

### twrite

_merlin label s_ → _merlin_

Appends `(label, s)` to the transcript and returns the same transcript on top.

### tread

_merlin label b n_ → _merlin b'_

Append `0..=4095` challenge bytes under `label` directly into the provided
Builder, preserving its prefix and refs, and return the advanced transcript.
Capacity, count and gas checks precede transcript mutation. Challenge extraction
uses no intermediate buffer. Zero-length extraction follows Merlin's ordinary
challenge semantics; it still advances the transcript.

### sha256 / sha512 / sha3 / keccak256

_s b_ → _b'_

Hash the source payload bytes and append the digest to the provided Builder,
preserving its existing payload prefix and refs. Source may be a Cell, Slice,
or Builder. A Slice hashes only its unread bytes; a Builder hashes all its
current payload bytes. The source is consumed. Source refs must be zero;
ordinary Cell bodies must be available and unpruned. These are byte digests,
not Cell commitments: no descriptors, refs, level hashes or depths are included.

SHA-256, SHA3-256 (FIPS-202), and Keccak-256 produce 32 bytes; SHA-512 produces
64 bytes. SHA3-256 and Keccak-256 are distinct. Capacity overflow hard-fails
before digest allocation or destination mutation. Charge hashing work and
digest storage before computing the result.

### cellhash

_c level b_ → _b'_

Append the Cell's cached 32-byte SHA256 commitment at `level` (`0..=15`) to the
provided Builder. No body is loaded, so unloaded and explicit pruned handles
can supply their commitments. Levels above the Cell's highest significant level
alias its highest hash. Invalid counts and insufficient destination capacity
hard-fail. Lower hashes are claims until authenticated against an expected root.

For example, two partial trees may share their level-zero hashes while their
factual IDs differ. Explicitly append the desired hashes into Builders and
compare those Builders with `eq`; `eq` itself never chooses a lower level.

### log

_s_ → ø

Consumes a zero-ref Cell/Slice/Builder payload and emits `TxEntry::Data(bytes)`
into the txlog. It is visible to the outer verifier and does not occupy
persistent storage. Cells resolve through the metered read context before their
payload is copied; Slices and Builders are read directly. Remaining references
are not silently ignored and fail `InvalidFormat`.
This permits `pushcell log` to publish all 4095 payload bytes without putting a
large byte literal into the instruction stream.

### evalpoint

_msm_ → _point_

Consumes a MultiscalarMul and immediately computes `Σ scalarᵢ·pointᵢ`, returning
an ordinary compressed `Point::Opaque`. It makes no assertion and does not append
to a batch. Available in both external and actor execution. The result can be
compared, branched on, serialized with `writepoint`, stored or sent as plain data.
It carries no private commitment/predicate annotation or asset ownership.

Every point is decompressed, including terms with coefficient zero; an invalid
encoding immediately fails `InvalidPoint`. Empty sums yield the identity. Term
count, not coefficients, repetitions, cancellation, result, cache state or CPU
core count, determines gas. The full price is precharged before allocation or
group computation. Other operand types fail `TypeNotMultiscalarMul` and are
not consumed; this cannot evaluate or disclose secret R1CS Expressions.

Total price for `n` supplied terms is `4 × (221 + 40n) = 884 + 160n`, including
the base instruction charge. Examples: one term 1044, two terms 1204, eight terms
2164. This 4× VM price is a fixed policy premium for immediate, non-batchable
execution, not a measured timing ratio or the asynchronous-send ordering weight.
Use `evalpoint`, compare with the identity Point, then scalar `verify` for an
immediate group assertion in actor code. MSM `verify` remains external-only.

## Token instructions

See [Token / ClearToken / WideToken](#tokens) for type semantics.

### amount

_t_ → _t qty flv_

Peeks the top token-shaped value and pushes `(qty, flv)` above it. `ClearToken`: both as Scalar. `Token`: both as `Point::Commitment`, preserving the original open or closed commitments and private assignments; works without a live CS. The token itself is retained. `WideToken` hard-fails `TypeNotToken` — its quantity isn't yet range-proven. Logical copying costs are independent of private opening presence.

### issuepriv

_qty tag_ → _T_

Pops `tag` (zero-ref byte source) and `qty` (`Variable` — a Pedersen commitment lifted via [`commit`](#commit)). Allocates a 64-bit range proof on `qty`. Builds `Token(qty_commitment, unblinded(flavor))` where `flavor = flavor_from_predicate(current_predicate, tag)`. Emits `TxEntry::IssuePriv(qty_commitment_point, unblinded_flv_point)` — the confidential-issuance txlog effect. Pushes the `Token`.

The current_predicate is the predicate stored on the enclosing `CallKind::ContractOpen` frame — created by [`open`](#open) or [`signcall`](#signcall) against an empty contract whose predicate is the desired issuer. Hard-fails `OpcodeRequiresPredicateContext` from `ExternalRoot` (no enclosing predicate) and from any `ActorCall` frame (issuance binds to a predicate, not an actor; the two issuance domains are kept disjoint by construction). Hard-fails `ExternalOnly` in internal context (the CS lane is required for the range proof and the qty commitment).

`Scalar` or `Point` operands hard-fail `TypeNotVariable` — lift to a `Variable` via [`commit`](#commit) first.

**Non-fungible tokens.** Mix the contract's [`anchor`](#anchor) into `tag` (e.g. `anchor … builder keccak256 endcell` against domain bytes) to derive a fresh flavor per issuance — the result is a non-fungible Token, since no other issuance will share the flavor. Use [`issueprivflv`](#issueprivflv) on the consumer side to recompute the same flavor scalar for verification.

### issueprivflv

_pred tag_ → _scalar_

Pops `tag` (zero-ref byte source) and `pred` (zero-ref byte source, exactly 32 bytes — a compressed Ristretto predicate point). Pushes `flavor_from_predicate(pred, tag)` as `Scalar`. Pure helper: no CS, no txlog entry, no predicate-context requirement. Domain separator is `flamevm.issuepriv.flavor` (consensus-fixed). Hard-fails `IndexOutOfRange` if `pred` is not exactly 32 bytes.

The confidential token's flv commitment is unblinded, so its point uniquely determines this scalar — meaning a consumer who knows the issuing predicate's point and the tag can recompute the flavor and check that an incoming Token belongs to the expected issuance domain.

### issuepub

_qty tag_ → _CT_

Pops `tag` (zero-ref byte source) and `qty` (`Scalar` — cleartext). Builds `ClearToken(qty, flavor_from_actor(current_actor, tag))` and emits `TxEntry::IssuePub(qty, flv)` carrying the cleartext `(qty, flv)` pair directly as `Scalar`s — publicly auditable on the wire without commitment indirection. Pushes the `ClearToken`.

The current actor is stored on either `CallKind::InternalRoot` or
`CallKind::ActorCall`. Both may issue. The opcode hard-fails
`OpcodeRequiresActorContext` from `ExternalRoot` and `ContractOpen` (issuance binds
to an actor, not a predicate). It runs without CS in internal execution.

`Variable` or `Point` operands hard-fail `TypeNotScalar` — `issuepub` is the cleartext path; for confidential qty, use [`issuepriv`](#issuepriv) from a `ContractOpen` frame.

**Non-fungible tokens.** Mix the call's [`anchor`](#anchor) into `tag` to derive a fresh flavor per call — yields a unique non-fungible token. Use [`issuepubflv`](#issuepubflv) on the consumer side to recompute the same flavor scalar for verification.

### issuepubflv

_cid tag_ → _scalar_

Pops `tag` (zero-ref byte source) and `cid` (zero-ref byte source, exactly 32 bytes — an actor id). Pushes `flavor_from_actor(cid, tag)` as `Scalar`. Pure helper: no CS, no txlog entry, no actor-context requirement. Domain separator is `flamevm.issuepub.flavor` (consensus-fixed). Hard-fails `IndexOutOfRange` if `cid` is not exactly 32 bytes.

### retire

_t_ → ø

Consumes a non-negative token and emits `TxEntry::Retire(qty_point, flv_point)`.
`ClearToken` uses unblinded commitments; `Token` uses the live commitment
points. A negative `ClearToken` hard-fails `NegativeTokenRetirement`, preventing
storage-fee debt and other liabilities from being discarded. Other types
hard-fail `TypeNotToken`.

### borrow

_qty flv_ → _−T +T_

Cleartext branch (both `Scalar`): pushes `(ClearToken(-qty, flv), ClearToken(qty, flv))`. The negative half is non-portable until balanced.

Quantity and flavor independently accept Scalar or committed Variable. Any
Variable makes the operation external-only and yields `(WideToken, Token)`:

| Quantity | Flavor | Binding and checks |
| --- | --- | --- |
| Scalar | Scalar | ClearToken pair; no CS. |
| Scalar | committed Variable | Check public qty is a non-negative u64; bind it as a constant; register flavor. |
| committed Variable | Scalar | Range-prove qty to 64 bits; bind public flavor as a full-field constant. |
| committed Variable | committed Variable | Register both; range-prove qty to 64 bits. |

Public components use unblinded commitments. Every confidential branch binds
the debt quantity to the negative of the positive quantity, with identical
flavor. Quantity assignment/range failures never produce portable positive
tokens. The public-quantity branch charges three R1CS units; secret-quantity
branches charge 67, including the 64-bit range gadget. Gas is independent of
private assignments. Allocated Variables without a public commitment fail
`TypeNotCommittedVariable`.

Raw `Point` operand hard-fails `TokenRequiresCS` — lift to `Variable` via [`commit`](#commit) first.

### merge

_a b_ → _{c 1 | a b 0}_

`ClearTokens` only. On flavor match, adds the centered integer quantities.
If their sum lies in `[-H, H]`, pushes `(ClearToken(a.qty+b.qty, flv), 1)`.
Flavor mismatch or centered quantity overflow restores both original tokens
as `(a, b, 0)` (soft-fail). Overflow occurs exactly when both centered inputs
have the same sign but their modular sum has the opposite sign.
Non-`ClearToken` operands hard-fail `TypeNotClearToken`.

### split

_a q_ → _a' b_

`ClearTokens` only. Returns `(ClearToken(a.qty − q, flv), ClearToken(q, flv))`. Hard-fails `TokenSplitOutOfRange` when either `q` or `a.qty` is centered-negative, or when their canonical residues satisfy `q > a.qty`.

### mix

_tokens… commitments… m n_ → _tokens_

Pops `n` (output count) and `m` (input count) as `Scalar`; then `n` output Pedersen commitment pairs (qty/flv zero-ref byte sources); then `m` input token-shaped values (any of `Token`, `WideToken`, `ClearToken`). Invokes the [spacesuit cloak gadget](../spacesuit/spec.md) to constrain that inputs balance outputs per flavor and to 64-bit range-prove each output. Pushes `n` output `Token`s.

Hard-fails `MixDegenerate` if `m == 0` or `n == 0`.

### decrypt

_T f f' q q'_ → _CT_

The stack is bottom-to-top `Token, f, f', q, q'`, so the VM pops `q'`, `q`, `f'`, `f`, then `Token`. It verifies that the supplied openings reconstruct the Token's commitment points and pushes `ClearToken(q, f)` on success.

External execution appends the two opening equations to the transaction's randomized batch; an invalid opening surfaces as `BatchSignatureVerificationFailed` at final verification. Internal execution checks both equations immediately because internal transactions have no deferred batch; an invalid point hard-fails `InvalidPoint`, and a mismatch hard-fails `CommitmentOpeningMismatch`. In a nested call, either immediate error follows the ordinary call rollback path and restores the caller's entry arguments.

## Control-flow instructions

Control flow is structured-by-convention within the current program Cell:
`label`/`jump`/`jumpif` carry a **label number**, not an offset, so bytecode
is written without computing positions. Prover and verifier use byte offsets;
the prover additionally tracks a private-instruction overlay. The high-level builder (`build_if` / `build_while` /
`build_loop` / `build_switch` / `build_break` / `build_continue`) emits these
with a running label counter and forward-jump backpatching. Code only ever
enters through a CallFrame or replaces its code via `exec`; there is no inline `run`; see
[`open`](#open) / [`signcall`](#signcall) / [`call`](#call).

### verify

_x_ → ø

- `Scalar`: hard-fails `VerifyFailed` if zero; otherwise consumes the value.
- `Constraint`: enforces the constraint via the delegate's CS (external context only).
- `MultiscalarMul`: appends `sum(s_i · P_i) == identity` to the delegate's `BatchVerifier` alongside any Schnorr/Musig sigs (external context only). Returns success immediately; the batched check runs at finalize and on failure surfaces as `BatchSignatureVerificationFailed`. See [MultiscalarMul](#multiscalarmul).
- Other types hard-fail `TypeNotScalar`.

**Type-dependent total gas** (including the one-gas instruction charge):

| Operand | Total gas |
| --- | ---: |
| Scalar | `1` |
| Cleartext Constraint | `1` |
| Secret Constraint | `1 + C + 120m` |
| MSM with `n` terms | `221 + 40n` |

`C` is the existing structural copy/traversal estimate (`Value::clone_gas`);
`m` counts multipliers newly introduced by flattening the Constraint. Equality
and conjunction introduce none, secret OR introduces one, and secret NOT two.
Previously charged allocations are not charged again as new multipliers.
These existing verification prices are unchanged. The MSM price includes
`n` term-traversal units, `220 + 37n` crypto work and `2n` batch-vector items.
`evalpoint` uses exactly four times this full MSM price. All arithmetic is checked
for overflow, and charges never depend on private assignments or batch contents.

### fee

_qty_ → _−WT_

Pops `qty: Scalar` in sparks (non-negative, `≤ MAX_FEE = 2²⁴`). Fees always use the canonical native flavor `FLAME_FLAVOR = Scalar::ONE`. The `2²⁴`-spark cap is chosen so fee-rate arithmetic stays within `u64`: even a `2⁴⁰`-byte (~1 TB) transaction leaves 24 bits of headroom. Emits `TxEntry::Fee(qty as u64)` and bumps the per-tx [`CheckedFee`](#fees) accumulator (also capped at `MAX_FEE`). Allocates a fresh `WideToken` debt with `q = −qty`, `f = FLAME_FLAVOR` (both cleartext-constrained) and pushes it. The script must balance the debt against native Flame tokens, typically via [`mix`](#mix).

Hard-fails: `FeeQtyNegative`, `FeeTooHigh` (per-arg or aggregate overflow), `TypeNotScalar`, `ExternalOnly`. The blinded-fee branch is reserved for a future phase.

### label

ø → ø — operand: label number (sub-varint)

Marks a jump target. Each CallFrame keeps a `labels` array of positions
(byte offset plus the prover's overlay index), built
lazily as labels are reached. Labels must appear in strictly sequential order
— 0, 1, 2, … — so the array is indexed directly by label number. On reaching
`label N`:

- `N == labels.len()` → record the position after the label (first sight).
- `N < labels.len()` → a re-visit (a loop back-edge re-traversing a label in
  its own body): allowed iff the recorded position matches; otherwise
  hard-fails `LabelOutOfOrder`.
- `N > labels.len()` → hard-fails `LabelOutOfOrder` (gap / out of order).

Labels are Cell-local within a frame: opened/called programs start from zero,
and `exec` discards the old label table and starts a fresh scope.

### jump

ø → ø — operand: label number (sub-varint)

Sets the cursor to label `N`. If `N` is already recorded (`N < labels.len()`),
jumps immediately — backward, or forward to an already-seen label. Otherwise
enters *skipping mode*: scans forward **without executing**, recording each
`label` passed (each must be the next sequential number), until `label N` is
reached, then resumes execution after it. Reaching end-of-program while
skipping hard-fails `LabelNotFound`.

### jumpif

_x_ → ø — operand: label number (sub-varint)

Pops `x: Scalar`. If non-zero, behaves as [`jump`](#jump) to label `N`; if
zero, falls through to the next instruction. Hard-fails `TypeNotScalar` if
the top value is not a scalar.

### return

_result_ → ø

Atomic cross-frame return:

1. Asserts an enclosing call frame exists or this is an offchain `QueryRoot`
   (otherwise `ReturnAtRoot`).
2. Asserts the callee stack contains exactly one Value (`BadReturnArity` if empty,
   `StackNotClean` if additional values remain), and validates actor storage.
3. For a child, pops the frame and refunds leftover gas to the parent.
4. Pushes the result followed by success marker `1`; no count is emitted.
   An offchain root instead completes with the result for its host.

An empty-stack run-off-the-end child exit returns Scalar `0` (unit) followed by
`1`. A failed child restores its entry escrow followed by `0`: one original
argument for actor calls, or the original locked Contract and argument for
`open`/`signcall`. The Contract payload is never returned separately on failure.
Use a Dict for multiple application results, including query results whose
entries are recursively data-only. Error recovery uses prebuilt
entry snapshots and requires no new encoding or Cell resolution.

In read/write execution, `return` deliberately performs no portability check. Non-portable values,
including negative `ClearToken`s and `WideToken`s, may move upward to the
caller, which owns responsibility for balancing or consuming them. Every
downward boundary (`call`, `open`, `signcall`, and asynchronous `send`) accepts
portable values only. A read-only result must satisfy the recursive query-data
rule; its Dict contents and Cell graphs are made available before discarding
the callee's checkpoint.
Both return paths reject an unrestored read-only checkout.

At a transaction root, `return` always errors; a script terminates cleanly by running off the end of its instructions with an empty stack (jump to a trailing label to short-circuit). At an offchain query root, `return` completes the query with one data Value and no status suffix; empty-stack fall-through returns Scalar `0`.

### type

_x_ → _x typecode_

Pushes the type code of the top value as `Scalar`, leaving the value on the stack. Type codes are stable — **independent of the wire-encoding tags** in [Types](#types), which keep their own compact scheme (so the `type` opcode also covers non-serializable stack-only types). Code 1 is reserved after removing String; all other codes are unchanged:

| Code | Type | Code | Type |
| --- | --- | --- | --- |
| 0 | Scalar | 7 | Contract |
| 1 | reserved | 8 | Merlin |
| 2 | Dict | 9 | Variable |
| 3 | Point | 10 | Expression |
| 4 | Token | 11 | Constraint |
| 5 | WideToken | 12 | MultiscalarMul |
| 6 | ClearToken | 13 | Cell |
| 14 | Slice | 15 | Builder |

### Contract, actor, and send instructions

[`open`](#open), [`signcall`](#signcall), and [`call`](#call) all create isolated call frames as described in [Design overview](#design-overview).

### input

_contract_cell_ → _contract_

Resolves the supplied Cell reference through the execution context and decodes
the authenticated Contract root: predicate, anchor, and one payload Value. Its
factual CellID is the ContractID. Dict
branches can remain pruned. Sets `last_anchor = Anchor(contract_id)` and
emits `TxEntry::Input(contract_id)`.

A prover uses `push_cell(CellValue::contract(c)?)`; this emits `pushcell`
with the actual Contract root and its supplied descendants. Private atomic
openings are restored only after the same public path/value has resolved.
A private witness cannot supply a publicly missing branch. Generic untrusted
Contract imports remain fully validated; the VM input path relies on the
already-admitted UTXO identity.

The VM does not verify accumulator membership: the chain must check the
spend-once Utreexo proof and reject duplicate inputs. Missing/malformed witness
bodies, pruned roots, arbitrary malformed Cells, and old ID-only payloads hard-fail.
Copyable raw Cells do not create bearer ownership without chain admission.
`input` is external-only.

### contract

_payload pred_ → _contract_

Pops `pred: Point` and one portable payload Value. Splits `last_anchor`:
the left half becomes the new Contract anchor and the right half remains
the continuation. Use a Dict for multiple payload values; there is no count
operand. Hard-fails `AnchorMissing` or `NonPortableInOutput`.

### output

_payload pred_ → ø

Same construction as [contract](#contract), but emits an Output effect.

### open

_contract internal_key program_cell gas arg_ →
_{result 1 | contract arg 0}_

Pops one portable argument, gas grant, program
Cell, internal key (Point), and Contract. Resolves the program's own record and
checks `P = X + h(X, program_cell.hash(0))·B`, then executes its raw bytecode.
There is no native branch index, Trie lookup, or leaf tag. A single Cell can hold
the entire policy. Optional branch dispatch belongs to that committed code:
`runslice` exposes remaining references, `loadref` selects one, and `exec` enters
it. Child bodies are read only when requested. Pruned records authenticate but
cannot execute. Authentication does not globally virtualize the supplied graph:
pruned descendants remain pruned during execution. TxID commits the graph's
factual highest-level hash. Private overlays must compile to the exact factual
program Cell, including its operand refs.

The prover can build branches with `PredicateTree::from_scripts` and emit a
opening with `ScriptBuilder::push_taproot_proof(&tree, logical_index)`. The helper
builds salted executable leaves and bytecode dispatchers; its opening supplies
one branch and prunes the others, preserving hash(0). It pushes the inner Point
and root Cell, and attaches only the selected program's private assignments and
nested lookup witnesses. The inline graph is not duplicated in the lookup index.
`build_tx` packages public lookup bodies into the frozen execution witness hierarchy;
`UnsignedTx` preserves it through signing and `ExternalTx::verify` consumes it
directly, including after a CellEnvelope transport roundtrip.

On success, an isolated ContractOpen frame receives the **single payload**
followed by the single explicit argument. Its starting anchor is split from the
parent. The frame has only the immediate caller's actor ID for attribution,
never actor-state authority. Actor state/storage/code, public issuance,
`selfid`, and synchronous `call` hard-fail. Nested ContractOpen callerid
does not transitively inherit an earlier actor. Anonymous async `send` remains
available. CS access follows external/internal execution context.

Successful return yields `result 1`; clean empty-stack fall-through yields `0 1`.
An entered child's failure rolls back effects and returns the original locked
Contract followed by the original argument and status zero. Its payload is not returned
separately. Unused child gas is refunded on both success and failure; consumed
instruction, allocation and resolution work remains charged.

Bad operands/commitments, missing or pruned executable roots, non-portable arguments,
insufficient caller gas, and depth rejection hard-fail the current frame.
Missing data after entry follows normal child-failure escrow recovery.

### send

_arg refund gas addr_ → ø

Asynchronous message-send. Pops operands top-first:

1. `addr` (Cell) — exactly one canonical ActorAddress; Init and Restore carry
   their native constructor/state reference.
2. `gas` (`Scalar`) — gas allotment.
3. `refund` (32-byte zero-ref payload) — bounce predicate point.
4. `arg` — one portable Value, delivered as the initial stack item. A caller
   may pass a Flame Token directly or group it with other fields in a Dict
   for the destination actor to spend with `addstorage`.

The full `gas` grant is debited from the active frame before the Send effect is
recorded. Insufficient remaining gas hard-fails `OutOfGas` and no message is
emitted. The grant is never refunded: asynchronous execution has no live caller
to receive a remainder. An internal transaction therefore cannot reserve more
gas across its own descendant sends than it received in its triggering Message.

Splits the frame's `last_anchor` (see §Anchors): the `left` half becomes
the message's `anchor`, the `right` half replaces `last_anchor`. Hard-fails
`AnchorMissing` if no anchor has been claimed yet. Emits
`TxEntry::Send(Message)`; the block builder reads these records when constructing
internal deliveries. An actor frame contributes its actor id as
`Message.caller`; `ExternalRoot` and `ContractOpen` contribute `None`. `None` means
“no authenticated actor principal,” not necessarily “originated in an external
transaction.”

The payload is admitted by `Message::new`, not by `Message::encode`.
`Message::new` checks the single payload and uses the sticky Dict flag for O(1)
nested checks, then establishes encodability; the payload cannot be replaced.

The send's identity is the canonical Message root CellID. Its payload contains
anchor:32, ActorAddress, caller-presence:u8 and optional raw ActorID:32,
refund Predicate:32, and gas:u64 LE. Its payload is a referenced tagged Value
Cell, without an implicit list/Dict wrapper; constructor or restoration data uses an earlier ref.
The Send TxEntry references that Message Cell. Anchor ratcheting makes every
distinct send unique by construction. See [Data encoding](encoding.md).
Available in both contexts. Hard-fails with a Cell decoding error when `addr`
is not a complete canonical `ActorAddress`, or `MalformedAddress` when `refund`
has the wrong size; `NonPortableInSend` on non-portable args; and `InvalidBitrange` on a
gas allotment outside the `u64` range. It hard-fails `OutOfGas` when the active
frame cannot prepay the requested grant.

On delivery failure, consensus seals the original payload Value unchanged in one
fresh Contract under `refund_predicate` and emits an Output effect.

### call

_arg gas addr_ → _{result 1 | arg 0}_

Synchronous actor-to-actor call requesting write permission. The effective child
permission is `parent.readwrite && true`: a read-only caller stays read-only,
and then all [query rules](#actor-queries), including data-only arguments and
results and `Address`-only routing, apply. Same operand shape as [`send`](#send) minus
`refund` and, likewise, no method selector. A caller that wants the callee to
purchase storage passes Flame as its argument or inside an argument Dict; allocation remains
the callee's explicit decision. Init may provision an absent actor; Restore may
authenticate and retain a partial state tree. Both commit only if the callee
succeeds and retained usage fits its own leases. See [Actors](#actors).

Only `InternalRoot`, `ActorCall`, and `QueryRoot` may invoke `call`. `ExternalRoot` and
`ContractOpen` hard-fail `OpcodeRequiresActorContext` before consuming operands;
caller attribution stored in a `ContractOpen` is read-only and is never used to
fabricate an actor caller.

Before entering the callee, `call` rejects a non-portable argument with
`NonPortableInCall`. A Dict is checked in O(1)
through its sticky flag.

**Re-entrancy:** state is reachable **only** via `load`, which acquires the
state-checkout lock, and there is no peek-state opcode. A re-entrant `call` or
`load` into an actor that has already loaded its state cannot enter, so no other
frame can make that loaded value stale. Holding it across a `call`, `send`, or
`open` is legal, and it may be saved afterward. Correctly accounting for the
interaction's status and effects is ordinary actor logic rather than a
concurrency guarantee the VM can infer. Nested call depth is capped at
`MAX_CALL_DEPTH` (64).

**No Call entry.** Calls are intra-tx control flow; Init/Restore resolution may emit `ActorDeploy`/`ActorRestore`, and the callee emits ordinary structural effects. External TxID commits the body and its pruned effect claim; internal TxID commits the computed effects — see the effect model above.

Creates an isolated `CallKind::ActorCall { actor, caller }` frame (the frame's starting anchor lives in `CallFrame.anchor` — todo #4) with the popped gas allotment. The frame has the callee's actor identity — `op_load`/`op_save`/`op_call`/`op_send` operate on the callee.

Successful return is `result 1`; clean empty-stack fall-through is `0 1`. Before
entry, missing/frozen/checked-out actors, invalid restoration proofs, depth
rejection, and a grant too small to activate
the callee return the original `arg 0`. Before or after entry, unused gas
returns to the caller; pre-entry code-loading work remains charged. An undersized
child grant or runtime exhaustion consumes the full grant. Every runtime failure
rolls back the child and returns the same escrowed argument and failure suffix;
the failed child's current stack is discarded. `RegistryUnavailable`, malformed
operands, non-portable arguments, or inability of the caller to fund the grant
hard-fail the current frame before this soft boundary.

### query

_arg gas addr_ → _{result 1 | arg 0}_

Synchronous actor-to-actor read-only call, opcode `0xd7`. It uses the same
single-Value ABI, actor identity, gas grant/refund, depth bound, checkout lock,
and failure escrow as [`call`](#call), but always enters with `readwrite = false`.
It accepts only `ActorAddress::Address`; an `Init` or `Restore` destination
returns `arg 0` without initialization or restoration. Non-data arguments
hard-fail `NonDataInQuery` before entry. Callee write attempts, changed saves,
unrestored checkouts, non-data results and unavailable result bodies fail the
child and return the original `arg 0`.

Successful explicit return produces `result 1`; clean empty-stack fall-through
produces `0 1`. Success discards the child's registry checkpoint as failure does,
while retaining computation charges. No effects or anchors cross this boundary.
See [Actor queries](#actor-queries) for the full permission and host API rules.

### load

ø → _value_

**Checks out** the current actor's data: moves its portable `Value` out of the registry and pushes it onto the stack. The author chooses its structure. Code stays in the combined actor image; `setcode` replaces its code reference.

While checked out, synchronous `call`, `query`, and `load` against the actor fail
`ActorEmpty` — an **explicit checkout lock** guards ownership, independently
of whether bodies are available. `send` is
asynchronous and does not inspect recipient state when queued; availability is
resolved later during delivery. The state is a linear resource: `load` moves it
(no copy); the frame must discharge it before returning, per the frame-end
clean-stack rule. Re-loading an already-checked-out actor → `ActorEmpty`.

Hard-fails: `OpcodeRequiresActorContext`, `RegistryUnavailable`, `ActorEmpty` (already checked out), `ActorNotFound`.

**Self-destruct.** A frame discharges a loaded state by either `save`ing it back
(the actor persists) or **explicitly dismantling it** — recursively reading
every item out, retiring/spending tokens, and dropping the droppable residue. A
fully dismantled state leaves the actor empty; the transaction emits
`ActorDestroy` and removes its code and state. Unexpired leases are not refunded
and recycle only at their original expiry heights. Forgetting to discharge is
caught loudly by `StackNotClean` and rolls the transaction back.

In read-only execution, `load` also remembers the factual CellID of the public
state encoding. The state must be restored with an unchanged `save` before a
successful exit, even if it was droppable. Dismantling is not permitted;
otherwise the getter fails `QueryStateNotRestored` and rolls back its checkout.

### save

_value_ → ø

Pops the state `Value`, **validates portability**, and **moves it back** into the current actor — which must be **checked out** by a prior `load`, else `SaveWithoutLoad` (saving to a non-checked-out actor would clobber, and silently drop the tokens of, live state).

In read/write execution, measures the prospective actor usage according to [Actor storage](storage.md) and hard-fails `StorageCapacityExceeded` if usage exceeds capacity at the current core-block height. Emits `TxEntry::ActorSave { actor, state }` carrying the **full** post-save state — symmetric with `Output(Contract)` which carries the full Contract. The entry Cell contains the actor ID and a reference to the state Value Cell. The TxID commits to that root without flattening its graph.

In read-only execution, compares the factual CellID of the Value encoding to
the commitment remembered by its matching `load`. A different commitment
hard-fails `QueryStateChanged` before registry mutation. An unchanged save
restores the checkout without emitting `ActorSave`; the entire query checkpoint
is still discarded at exit. Temporary changes cannot be saved and observed by
another actor before restoring the original later.

**State is any portable Value.** The author structures state however they like (a Dict, a Scalar, a Token, …).

**Portability is the canonical storage gate.** Portable values: `Scalar`, `Cell`, `Point`, `Dict` of portable, non-negative `ClearToken`, `Token`. Non-portable values (`Slice`, `Builder`, `Contract`, `Merlin`, `Variable`, `Expression`, `Constraint`, `MultiscalarMul`, `WideToken`, negative `ClearToken`) hard-fail `NonPortableInState`.

The registry repeats this check on direct save and deploy entry points, so the
actor-state invariant does not depend on `op_save` being the caller. As at the
other domain boundaries, nested Dicts are checked through their cached flag and
state encoding performs no portability validation.

### setcode

_code_ → ø

Consumes a Cell reference, resolves its root, and replaces the current actor's
native **code graph**. All unpruned descendants must be available through the
scoped resolver and are materialized with gas charges before persistence;
explicit pruned terminal records stay pruned. Missing unpruned bodies fail,
rather than making persistent availability depend on prover/verifier cache
residency. A pruned root is not executable. It hard-fails `StorageCapacityExceeded` if the new
code would exceed current capacity. On success it records
`TxEntry::SetCode { actor, code }`, symmetric with [`save`](#save)). It is
method-agnostic and independent of the state checkout lock. Requires actor
context.

**Upgrade is author policy over a VM mechanism.** The VM provides `setcode`; *who* may call it is gated in the actor's own code. Actors normally authenticate by **caller identity** — `require(callerid() == GOV)` — and may use `signcall` when immediate signature authorization is required. Internal execution has no deferred signature batch: `signcall` verifies synchronously before entering its child. A naked `setcode` with no caller gate is an unconditionally-upgradeable (i.e. rug-able) actor — deliberately the author's call. See the design.

Hard-fails: `OpcodeRequiresActorContext`, `RegistryUnavailable`, `TypeNotCell`, `ActorNotFound`, Cell resolution/pruning errors, `StorageCapacityExceeded`.

**Atomicity.** A `save` failure (`SaveWithoutLoad`, `NonPortableInState`,
`StorageCapacityExceeded`, `ActorNotFound`) propagates as a frame failure. The
frame's call boundary rolls back both the txlog and actor registry. At the
outermost frame, `execute_internal` applies the same rollback at transaction
level.

Hard-fails: `SaveWithoutLoad` (actor not checked out), `NonPortableInState`, `StorageCapacityExceeded`, `OpcodeRequiresActorContext`, `RegistryUnavailable`, `ActorNotFound`.

### addstorage

_q_ → _debt 1 | 0_

Pops `q` as a `Scalar` whose canonical residue is a positive byte count. The request must be at least
`MIN_LEASE_BYTES`, must be a multiple of `STORAGE_UNIT_BYTES`, and must leave at
least `MIN_REMAINING_POOL_BYTES` in the global pool. The constants, quote
formula, block ordering, and lease rules are defined in
[Actor storage](storage.md).

On success, atomically:

1. recomputes the fee against the current reserve;
2. removes `q` bytes from that reserve;
3. adds or coalesces a lease for the current actor at
   `current_height + LEASE_DURATION_CORE_BLOCKS`;
4. emits `TxEntry::StoragePurchase { actor, bytes, expiry_height, fee_sparks }`;
5. pushes `ClearToken(-fee_sparks, FLAME_FLAVOR)` followed by `1`.

The negative token must be balanced with actual Flame before transaction
finalization. The storage-purchase effect burns that amount rather than paying a
minter. A later failure rolls back the pool, lease, effect, and burn.

An unavailable or unrepresentable request consumes `q`, pushes `0`, and changes
nothing. Type and context violations hard-fail: `TypeNotScalar`,
`OpcodeRequiresActorContext`, or `RegistryUnavailable`.

### quotestorage

_q_ → _fee 1 | 0_

Performs the same request, reserve, arithmetic, and representation checks as
[`addstorage`](#addstorage). On success it consumes `q` and pushes the positive
integral total fee in sparks followed by `1`. On an unavailable quote it
consumes `q` and pushes `0`.

This opcode is read-only: it does not reserve bytes, add a lease, emit an effect,
or change the price. `addstorage` always recomputes its quote, so an intervening
purchase may change the fee or make the request unavailable. Type and context
violations hard-fail under the same conditions as `addstorage`.

### signtx

_contract_ → _payload_

Pops the Contract, records a `DeferredSig::TxBound { verification_key: contract.predicate.point, contract_id }` for final authorization against TxID, and pushes its single payload Value. No arity is pushed.

**No new frame** — the contract-holder is authorizing the existing transaction in place.

`signtx` is external-only and checks its context before consuming the Contract.
Internal transactions have no signature envelope and their final TxID does not
exist when the opcode executes, so the TxID-bound signature cannot be verified
immediately. Internal authorization uses `signcall` instead.

The deferred signature is verified at finalize: the prover aggregates all `TxBound` keys via MuSig and supplies the envelope signature; the verifier batches all `TxBound` items against the `flamevm.signtx` transcript bound to TxID. Errors `BatchSignatureVerificationFailed` or `MissingTxBoundSignature` at finalize.

### signcall

_contract script sig gas arg_ →
_{result 1 | contract arg 0}_

Same call-frame mechanics as [`open`](#open) — taproot reveal is replaced by signature verification:

1. Pops `arg` (one portable Value) and `gas`. A non-portable
   argument hard-fails `NonPortableInCall` before child entry.
2. Pops `sig` (zero-ref byte source, exactly 64 bytes — Schnorr signature).
3. Pops `script` (Cell reference), resolves its executable root, and pops `contract`.
4. Builds `signcall_message(script.id())` via a Merlin transcript labelled
   `flamevm.signcall`, committing to the factual highest-level hash of the
   supplied graph, including references and explicit pruning. Scripts bind themselves to further context (anchor,
   actor identity, tx data) through explicit checks inside the signed program.
5. In external execution, records `DeferredSig::Explicit` for final batch
   verification. In internal execution, parses and verifies the signature
   immediately against the Contract predicate; malformed bytes fail
   `BadSignatureBytes`, while a validly encoded but incorrect signature fails
   `SignatureVerificationFailed`. Internal execution records no deferred item.
6. After verification or deferral, snapshots rollback state, creates the
   isolated `ContractOpen` frame with stored payload followed by the argument.

External deferred signatures are batch-verified at finalize alongside any
`signtx` items. Entered-child failure removes an external deferred signature
and returns the original Contract plus argument using the same failure
shape as `open`, followed by status zero without a count. Internal
signature failure occurs before child entry and hard-fails the current frame.

This changes the signed statement from source bytes to the factual program
CellID; old source-byte signatures are not accepted. `CellWitness::Script` and
host `push_script` retain private native instruction overlays only after their
compiled graph matches that ID. Continuations and child programs execute via
the ordinary Cell frame machinery; private overlays do not supply missing
public bodies. Actor addressing is unchanged and will be designed separately.

### timelock

ø → _n {0|1}_

Pushes the transaction's `locktime` (as `Scalar`) and a unit flag: `0` for block height, `1` for Unix timestamp. The split follows Bitcoin's BIP-65 convention — `flag = 1` iff `locktime ≥ 500_000_000` (`LOCKTIME_TIMESTAMP_THRESHOLD`). Values below the threshold are block heights; values at or above are Unix timestamps (the threshold corresponds to ~1985-11-05, before any practical timestamp range). Available in either transaction context; read-only execution fails `ReadOnlyViolation`.

### version

ø → _n_

Pushes `TxHeader::version` as a `Scalar`. Available in either transaction context; read-only execution fails `ReadOnlyViolation`.

### selfid

ø → _s_

Pushes the current frame's actor id in a 32-byte zero-ref Cell. Hard-fails `OpcodeRequiresActorContext` from `ExternalRoot` or `ContractOpen` (no actor identity).

### anchor

ø → _s_

Pushes the frame's *current* `last_anchor` in a 32-byte zero-ref Cell — the value the next consume site would split. Hard-fails `AnchorMissing` if no anchor has been claimed yet (same rule as `contract` / `output` / `send` / read/write `call`). Available in either transaction context; read-only execution fails `ReadOnlyViolation`.

### gas

ø → _n_

Pushes the current call's remaining gas budget — i.e. `gaslimit − gas_used` (saturating). Available in either context.

**Gas metering.** Every execution attempt costs 1 gas before checking for EOF;
executed instructions and instructions scanned while seeking a forward label
cost the same. Thus a clean program with `N` instructions consumes at least
`N + 1` gas. Prover-side decoded instructions and verifier/internal byte streams
walk the same sequence. Label tables are collected afresh per frame. Gas depends
on code, consensus inputs, and the logical execution history below, never on a
node's pre-existing caches.

Variable-size allocation work is additional gas: canonical code and decoded
payload activation, Builder byte/ref growth, Dict/collection growth, rollback
escrows, actor-state activation and cloning, constraint terms, range-proof
terms, MSM/batch vectors, and `mix`'s quadratic work are charged before the
corresponding allocation. Fixed-size stack pushes are bounded by the base
instruction charge. Allocation charges are monotonic and are not refunded when
memory is freed; this is a deterministic logical-work bound, not a dependency
on Rust object sizes or allocator behavior. Prover-only witness presence never
changes gas. Exhaustion hard-fails `OutOfGas`.

#### Cell-read gas

Each external or internal transaction starts with an empty set of resolved
physical CellIDs. All synchronous frames share this set. Every logical body
read first checks the current permitted sources and the requested identity;
an ID in the set is never itself a source of a body or permission to read one.
The caller still validates the complete reference commitment and the expected
typed encoding. This preserves pruning and actor-storage scope on warm reads.

| Body read | Gas, in addition to the instruction and other work |
| --- | --- |
| First successful read of a physical CellID | `1 + cell.record_size() + cell.refs().len()` |
| Subsequent successful read of that CellID in the same transaction | `1 + ceil((cell.record_size() + cell.refs().len()) / 4)` |
| Failed body resolution | `1`; the attempt does not warm the ID |

This follows [TON's 100/25 first/repeat Cell-load pricing pattern](https://docs.ton.org/tvm/gas),
not its absolute gas units. Warm reads keep a quarter of the bounded record-work
charge because validated access still does work; the discount is not a free read.

The initial `1` is charged before resolution. A first read enters the set only
after its full cold charge succeeds; out-of-gas never partially warms it.
Distinct physical IDs remain distinct even if their lower semantic hashes
match. Resident handles, actor storage, and transaction witnesses use the same
tariff. A newly constructed Cell is not warm merely because it exists in RAM.

Successful reads and consumed gas survive synchronous-call failure and rollback.
A later typed-operation failure does not undo preceding successful body reads.
Each asynchronous delivery starts a fresh set, even when the sender or an
earlier message read the same Cells. Prover and verifier follow the same logical
read sequence; local decoding and physical cache eviction do not alter it.

The discount covers body resolution only. Allocation, copying, typed decoding,
Trie rebuilding, serialization/hashing, and rollback cloning retain their
separate bounded charges. There is no new serialized size summary or gas-cache
metadata in a Dict or Cell.

#### Other metered work

Expensive deterministic work is prepaid where it is requested, even when the
actual cryptographic check is deferred until transaction finalization. External
roots prepay proof-finalization overhead; hashing is charged by compression
block; `open` charges point decompression; `signcall` and `signtx` charge
signature verification; R1CS-building instructions charge estimated
multipliers/items; and MSM/decrypt verification charges decompression plus the
scheduled final MSM. Prover and verifier take the same charges.

The reference cryptographic operation costs were calibrated with
`cargo bench -p flamevm --bench gas` on 2026-08-23 on an arm64 machine. One gas
was conservatively treated as approximately 100 ns of reference verifier work.
Representative medians were:

| Work | Median |
|---|---:|
| SHA-256, 64 KiB | 132.45 µs |
| SHA-512, 64 KiB | 85.12 µs |
| SHA3-256 / Keccak-256, 64 KiB | 71.5 µs |
| valid Ristretto decompression | 2.45 µs |
| immediate signature verification | 33.4 µs |
| 64-signature batch | 1.108 ms |
| R1CS verification, 64 multipliers | 0.968 ms |
| R1CS verification, 512 multipliers | 4.97 ms |
| MSM finalization, 64 terms | 98.5 µs |
| MSM finalization, 1,024 terms | 0.997 ms |

The measured hash, R1CS, and MSM curves remained bounded by the selected linear
prices over the tested ranges, so no arbitrary crypto-size cap is added. Checked
gas multiplication and the enclosing transaction/block gas limits provide the
bound. The logical allocation prices remain intentionally more conservative
than measured allocator work.

Call entry (`call` / `open` / `signcall`) debits the full gas grant from the
caller. A caller that cannot afford the grant hard-fails; unused gas is
refunded on success **and failure**, and `gaslimit` remains the immutable creation
cap. Consumed work is charged even when its effects roll back. Exhaustion leaves
no refundable gas. An actor rejected before entry returns the unused portion,
retaining charges for any code-loading work. A child grant too small to activate
its code is consumed in full. `send` debits its full grant and never refunds it;
delivery and every descendant send execute within that prepaid budget.

### usage

ø → _n_

Pushes the current actor's occupied persistent-storage bytes, including code,
canonical state encoding, and lease records, as defined in
[Actor storage](storage.md). While state is checked out, the result measures the
checked-out committed state until `save` supplies a replacement. Hard-fails
`RegistryUnavailable` or `OpcodeRequiresActorContext` outside actor execution.

### callerid

ø → _s_

Pushes the directly invoking actor's canonical 32-byte id. `InternalRoot` and
`ActorCall` use their recorded caller; `ContractOpen` uses its compact read-only
`caller_id`. An offchain `QueryRoot` has no authenticated caller. It pushes a zero-ref Cell containing 32 zero bytes when that caller is absent and
hard-fails only from `ExternalRoot`, which has no caller frame. A `ContractOpen`
opened by another `ContractOpen` sees zero: actor attribution is not propagated
through predicate frames.

### gaslimit

ø → _n_

Pushes the current call's total gas budget cap (the value set at frame creation, not the remaining amount). Available in either context.

### capacity

_h_ → _n_

Pops a nonnegative current or future core-block height and pushes the current
actor's leased capacity in bytes at that height:

```text
capacity(h) = STORAGE_UNIT_BYTES
            * sum(lease.units where lease.expiry_height > h)
```

The query is read-only and does not expose the lease list. A canonical residue
outside the `u64` height range hard-fails `InvalidBitrange`; a height below the current
core-block height hard-fails `StorageHeightInPast`. Outside actor execution it
hard-fails `OpcodeRequiresActorContext` (or `RegistryUnavailable` when no actor
registry was supplied).

### Chain-info instructions

`height` reads the implemented consensus-supplied `BlockContext`. The historical
opcodes `blockhash`, `blockburn`, `blockweight`, `blockrate`, and `chainstate`
below remain planned: their bytes are reserved but currently fail
`UnknownOpcode`. Their proposed 100-block maturity rule is not active.

### height

ø → _n_

Pushes the current core-block height during an internal transaction. Pushes zero
throughout an external transaction, including its nested `ContractOpen` frames.

### blockhash

_h_ → _s_

Pushes the 32-byte block hash at height `h`.

### blockburn

_h_ → _n_

Pushes the total satoshis burned at height `h` (Bitcoin-coupled metric).

### blockweight

_h_ → _n_

Pushes the block weight at height `h`.

### blockrate

_h_ → _n_

Pushes the average mint rate (sparks per satoshi) at height `h`.

### chainstate

_n_ → _dict_

Pushes a Dict of block stats at height `n`.

---


## Isolation & binding notes

**Contract-open trust model.** `open`, `signcall`, and `call` all create isolated call frames: the unlocked / signed / called script runs in its own stack, gas budget, and identity scope, with no implicit access to the host's actor state, gas pool, or identity. This eliminates the confused-deputy class of bugs — an actor accepting an untrusted-source contract need not audit the predicate as a global authorization filter, because the script cannot reach the actor's state regardless of what the predicate authorizes .

**`signcall` binding policy.** The `signcall` signature commits to the policy Cell's factual highest-level hash, including its referenced graph, whether verified immediately in internal execution or deferred in external execution. The program binds further context (anchor, actor identity, transaction data) through explicit checks such as `anchor <expected> eq verify`.
