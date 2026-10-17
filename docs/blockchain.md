# Flame blockchain state machine

## Scope and trust boundary

`flamechain` is the deterministic state machine around FlameVM. Given a prior
state, consensus parameters, an authenticated core-block context, and a
candidate Flame block, it either produces one new state plus complete undo data
or rejects the block without changing state.

It owns contracts and their accumulator, actors, storage leases, block resource
accounting, block application, local reorganization, and basic transaction
admission. It does **not** choose a Bitcoin branch, talk to Bitcoin Core, select
minters, run BFT, persist data, or provide networking. Those responsibilities
remain outside the state machine:

| Component | Responsibility |
| --- | --- |
| `flamevm` | Verify external scripts and execute actor messages under an explicit context. |
| `flamechain` | Apply deterministic Flame state transitions and make them reversible. |
| `btc-integration` | Observe and validate Bitcoin data, then supply authenticated core-block context and branch changes. |
| consensus / node | Choose the accepted branch, authenticate context, store blocks and undo, and call detach/attach operations. |

The block header commits an opaque `core_block_hash`, while FlameVM receives the
corresponding height as a plain integer rather than a Bitcoin RPC object. This
split makes the state machine reproducible and easy to test, but creates an
important trust boundary: `flamechain` cannot by itself prove that a supplied
Bitcoin hash or height is genuine or that two supplied hashes are parent and
child. The caller must authenticate that context according to
[consensus.md](consensus.md).

This document is the architectural target, not a claim that every wire format,
commitment, or persistence path is implemented. Open launch requirements are
called out explicitly below.

## Chain state

At a selected tip the persistent state contains:

- tip height and block identifier;
- the Utreexo forest committing to every live contract;
- the actor registry: actor code, state, and leases;
- the available actor-storage pool and a lease-expiry index derived from the
  committed per-actor leases; and
- any consensus resource counters that affect later validation.

Mempool contents, proof caches, Bitcoin RPC state, and transient VM frames are
not consensus state.

All state arithmetic is checked integer arithmetic. A failed check, invalid
proof, resource-limit breach, commitment mismatch, or VM failure required by
the transaction rules rejects the candidate transition. Applying a block is
atomic: no persistent mutation becomes visible until all checks pass.

## Contracts and Utreexo

Contracts are single-use values. An external transaction proves each input's
membership in the parent accumulator, deletes it once, and inserts its outputs.
The Flame-specific Utreexo implementation represents the accumulator as a
forest of perfect binary Merkle trees:

- `Forest` is the committed compact state;
- `WorkForest` stages insertions and deletions without mutating `Forest`;
- a `Proof` identifies a transient item or supplies a committed Merkle path;
- normalization produces the next canonical `Forest`; and
- `Catchup` updates surviving proofs across that transition.

Committed inputs prove membership in the declared parent root and no contract may
be deleted twice. A `Transient` proof intentionally permits a later transaction
in the same ordered block to spend an output created earlier in that block; the
same rule lets FIFO mempool children depend on admitted parents. The work forest
is normalized once after the ordered batch, and its resulting commitment must
equal the block's committed contract root.

Utreexo is not part of the Cell migration. Its retained byte encodings and
block proof transport are specified in
[Blocks and Utreexo](encoding.md#blocks-and-utreexo). The Serde forms of
`Forest`, `WorkForest`, `Catchup`, and `Proof` remain local working-state formats.

Utreexo retains its specialized Merkle hashing and proof-update algorithm, and
is the remaining consumer of `merkle` / `readerwriter`. Other chain collections
use Cell Tries; Contract and Taproot formats are fully Cell-based.

This lets a validating node keep a small accumulator instead of every contract
payload. It does not make contracts literally free or solve data availability:
owners must retain payloads and proofs, proofs need updating, blocks still carry
witness data, and archival history remains a separate cost.

## Actors and leased storage

Actors are reusable code-and-state records and therefore remain in the common
validated state. Their capacity, pricing, expiry, destruction, and opcodes are
defined in [storage.md](storage.md). `flamechain` owns this state; FlameVM reaches
it only through the actor-registry interface and an explicit block context.

Lease expiry, issuance, purchase ordering and end-of-block freezing follow
[Global state and block order](storage.md#global-state-and-block-order).
Restoration and retained-body charging follow
[Content, availability, and recovery](storage.md#content-availability-and-recovery).

Serial pricing is deterministic and maintains a hard storage bound, but ordering
has economic value: a proposer can place one valid purchase before another.
Burning storage fees prevents the proposer from directly recovering the fee; it
does not remove ordering advantage or general MEV. Purchase-splitting economics
are discussed with the canonical [pricing formula](storage.md#pricing).

## Blocks and commitments

A block has a header and an ordered body of external transactions. Internal
transactions are execution records derived by the state machine; they are not
independently submitted or selected by a proposer.

Block/header layouts, sequence commitments and BlockHash are defined in
[Blocks and Utreexo](encoding.md#blocks-and-utreexo); TxID, WitnessID and the
execution witness hierarchy are defined in
[Transactions and execution witnesses](encoding.md#transactions-and-execution-witnesses).
Generic transport and typed snapshot wrappers follow
[Cells](cells.md#root-last-graph-transport).

Validation binds the authenticated core-block context, ordered transaction
witnesses, execution records and resulting contract and actor/storage states to
those header commitments. Actor commitments include exact body availability,
not just logical contents. The outer block transport graph is not an execution
witness pool. Bounded decoders
check root code payload length before execution, enforce counts/depths and exact
typed payload/reference consumption, and reject malformed signatures/proofs.
Re-encoding must reproduce the exact outer envelope, rejecting unused transport
bodies. Unused bodies inside an execution Cell index are permitted because their
availability is explicitly committed. Network code never accepts a supplied
TxLog in place of execution.

Tests cover canonical round trips, exact bounds, witness isolation, and retained
Utreexo golden vectors.

## Staged block application

Block application follows one shared validation path for normal extension and
reorganization attachment:

1. Check parent, height, version, context relation, static limits, and canonical
   encodings that are already defined.
2. Stage lease expiry, recycling, actor marking, and issuance.
3. Verify the next external transaction against the current staged contract
   state. Reject missing or duplicate inputs and accumulate resource use.
4. Consume each derived external log through the ordered effect applier and
   enqueue its sends in effect order.
5. Fully drain this external transaction's FIFO descendant queue using its
   immutable execution Cell index before returning to step 3. Execute each message
   under an actor-registry checkpoint. Its one portable payload is normalized
   to the committed cell-backed representation before metering and execution;
   native sender maps and decoded messages must execute identically. A
   synchronous `call` remains inside its current internal transaction. On
   success, capture the actor commitment and storage pool, roll the direct VM
   mutations back, then consume the derived log through the same ordered effect
   applier. The replayed actor commitment and pool must match execution. Output
   and send lanes exist only as effects and are moved into Utreexo and the FIFO
   queue by this applier. A failed delivery instead derives exactly one refund
   output containing the original portable payload.
6. Recompute storage purchases from the ordered pool state while replaying and
   require their expiry and fee fields to match the derived effects.
7. Freeze marked actors by dropping resident code/state bodies while preserving
   their authenticated contents and metadata. No values are retired in bulk.
8. Normalize Utreexo, recompute all state commitments, compare them with the
   header, and reject any actual resource total above the active limits.
9. Commit the staged state and retain complete undo data.

The message discipline is FIFO within each external transaction's complete
execution closure. Sends made by an internal transaction append in effect order;
the next external transaction starts only after this queue is empty. Every
descendant uses the same initiating execution Cell index, never the next transaction's
bag or an uncommitted global cache. This is deterministic, but it makes a
long send chain serial and lets early transactions influence all later actor and
storage outcomes. Gas, message-count, call-depth, and block limits must bound
that work.

No duplicate-message table is part of consensus: message uniqueness follows
the VM's [anchor ratchet](flamevm.md#anchors). The queue is derived once
from each executed `Send` effect and fully drained before commit. A
reorganization may execute a message again only after rolling back its former
delivery or refund.

Delivery and recovery share the block's outer atomic checkpoint. Actor changes
from a failed delivery are rolled back before the original payload is sealed in
one refund Contract under `refund_predicate`. If constructing or applying that Contract
fails, the whole candidate is rejected, including the Send that originated the
message; a failed candidate never commits consumption without recovery.

## Derived internal transactions

Successful logs have shape `Header, Receive, effects...`; address resolution may
add `ActorDeploy` or `ActorRestore` before the target's effects, including for
synchronous callees. Any `ActorDestroy` entries form a suffix. Failed delivery
has exactly `Header, Receive, Output(refund)`. External logs start with `Header`
and cannot contain internal-only actor effects. Entry layouts are defined in
[TxLog](encoding.md#txlog).

An internal transaction is a receipt of deterministic execution, not another
consensus input. It starts from a `Send`, records its `Receive`, runs the target
actor and synchronous calls, and records the resulting ordered effects. First
successful delivery to a constructor-form target records `ActorDeploy`
immediately after `Receive`, binding the canonical actor id and full code; its
initial state is the canonical empty state. Explicit actor destruction is an
`ActorDestroy(actor)` effect of successful execution after its checked-out
state has been dismantled. Lease expiry only freezes bodies and produces no
retirement or destruction receipt. A failed delivery records `Receive` and
exactly one refund `Output`; the Contract uses the left split of the message anchor,
the same unique slot the delivery's first successful output would have used.
Its execution-record kind distinguishes failure from success.

Re-derivation prevents a proposer from forging actor changes and avoids a second
admission path. Its cost is that every validator must repeat serial actor
execution. Internal receipts may be stored or committed for audit and light
clients, but they cannot replace re-execution unless a future proof system is
specified.

Trust-boundary ownership is deliberately narrow:

| Value | Source and decoder rule |
| --- | --- |
| `Block`, `BlockTx`, `ExternalTx` | Network consensus input; use their bounded canonical decoders and reject trailing bytes before application. |
| Utreexo `Forest` and `Proof` | Consensus state/witness; use the exact bounded encodings above. |
| FlameVM `TxLog`, `TxEntry`, `Message` | Derived by verified execution; Cell codecs support commitments and archives, but consensus never admits decoded copies as execution results. |
| `WorkForest`, `Catchup`, actor undo/checkpoints | Local transient or persistence data; Serde representation is non-consensus and cannot enter block application. |

## Consensus limits

The current v1 defaults are:

| Limit | Default | Accounting rule |
| --- | ---: | --- |
| Transactions | 10,000/block | External `BlockTx` count. |
| Canonical witness bytes | 16 MiB/block | Both the complete block envelope and the sum of standalone `BlockTx` envelope sizes are bounded. |
| External script | 1 MiB/tx, 4 MiB/block | `ExternalTx.script` bytes. |
| Declared gas | 35,000,000/tx | Envelope cap for `Limits.gas`. |
| Gas credit | 10,000,000/tx, 100,000,000/block | Actual `gas_used - direct_send_gas`. |
| Internal gas | 25,000,000/block | Direct external `Send` grants; descendants are not counted twice. |
| R1CS multiplications | 1,024/tx, 100,000/block | Exact final constraint-system multipliers. |
| Delivered messages | 100,000/block | Every dequeued message, including descendants. |
| Utreexo proofs | 100,000/tx, depth 63 | One proof per derived `Input`. |

Active values are consensus parameters selected by protocol version; the table
records the implementation defaults rather than granting nodes local freedom to
change validity. Together they independently bound work that is not already
bounded by a smaller enclosing value, including:

- encoded block and external-transaction bytes;
- external and internal gas;
- contract inputs, outputs, and proof work;
- messages, actor executions, and call depth;
- cryptographic multiplication/MSM work; and
- fixed storage issuance and pool-bounded purchases.

Execution RAM has no independent storage-derived allowance. FlameVM charges
logical byte/item allocation work against external or internal gas, so these
gas ceilings also bound hostile active-memory growth. Persistent actor capacity
continues to bound stored state only.

Cell admission also bounds logical expansion, not just serialized Cell index size.
Typed block decoding has a shared work budget of four times the configured
witness-byte limit, charging each resolved Cell's canonical size, reference
count, and one lookup unit before decoding its contents. Repeated references
are charged each time: a small shared DAG cannot allocate an unbounded number
of proof vectors or other typed values. In-memory block candidates pass the
same bounded admission check as received blocks.

For resource accounting, `gas_used` is the verified external execution's metered
total, including gas reserved for sends. `direct_send_gas` is the sum of Message
gas grants in its derived TxLog; `gas_credit = gas_used - direct_send_gas` is the
actual external work. The VM's [gas grant rules](flamevm.md#gas)
make each descendant message partition that already-counted budget. Consensus
sums only direct grants for the internal-gas bound; summing nested grants again
would double-count it. The message-count limit independently bounds queue fan-out.

Storage parameters are listed in [storage.md](storage.md). Every active parameter
set must be selected by a committed protocol version; node-local configuration
must not silently alter consensus validity. Separate limits are easier to audit
and prevent one cheap resource from exhausting another, at the cost of more
parameters and possible under-utilization between pools.

## Basic bounded mempool

The mempool is local policy, never part of chain state or block validity. A
minimal implementation:

- admits only transactions that pass canonical decoding and the reusable
  transaction checks against the current tip;
- rejects an input conflict already represented by an admitted transaction;
- enforces explicit local count/byte/work bounds;
- enforces a configurable minimum fee and otherwise preserves FIFO order, so a
  transient child is never sorted before its parent; and
- after every tip change, updates Utreexo proofs with `Catchup` where possible,
  then replays or drops transactions that are no longer valid.

No replacement-by-fee, package scoring, persistent mempool, peer reputation, or
fair ordering is implied. A block remains valid even when local policy would
have rejected one of its transactions. Hard bounds prevent unbounded local
memory, but first-admitted FIFO is vulnerable to churn and low-value occupation;
peer-level rate limits belong in networking.

## Bitcoin-like reorganization

Fork choice is deliberately external. The caller supplies an exact sequence of
tip blocks to detach and candidate blocks to attach. `flamechain` must:

1. verify that every requested detach matches the current tip;
2. apply stored undo records in reverse order;
3. validate and stage each attachment through the ordinary block path; and
4. expose the new state only if the whole requested reorganization succeeds.

Undo is *complete* when it can restore the byte-for-byte prior persistent state
without re-running VM code or querying Bitcoin. It therefore covers the tip,
Utreexo forest, actor changes, freezing and explicit destruction, storage purchases, issuance,
lease expiry/recycling, expiry indexes, and all other committed counters.
Mempool changes are not undone; the pool is revalidated against the resulting
tip.

Normal detachment stores compact first-write actor/storage undo plus the prior
small Utreexo forest and header. A multi-block reorganization additionally
takes one full in-memory snapshot before detaching, so a failed replacement can
restore the original branch atomically. That rare-path clone is easy to audit
but costs time and memory proportional to live actor state; retained forward
blocks or a persistent transactional store should replace it if profiling shows
the cost matters. A node cannot detach deeper than its retained undo window and
must obtain an older trusted snapshot/state sync instead.

## Decision ledger

| Decision | Benefit | Cost / risk |
| --- | --- | --- |
| Keep Bitcoin tracking outside `flamechain`. | Pure, repeatable state transitions and no RPC dependency in consensus code. | The caller must authenticate and commit context; a forged height corrupts lease timing. |
| Derive internal transactions. | One admission path; actor effects cannot be forged. | Validators repeat serial work; receipts alone are not proofs. |
| Use Flame Utreexo for contracts. | Small common live-contract state and user-carried proofs. | Proof transport/catchup and owner availability become operational requirements. |
| Lease resident actor bodies while retaining frozen commitments. | Prices common payload storage without destroying linear values. | Frozen metadata persists; applications need witnesses to recover missing bodies. |
| Use endpoint reserve pricing. | One checked rational formula; reserve pressure is immediate. | Purchase splitting follows a much cheaper harmonic path; ordering creates MEV. |
| Stage a whole block atomically. | Invalid tails cannot leave partial state. | Requires transient working state; naive cloning may be expensive. |
| Retain complete undo. | Simple, exact detach and safe failed-branch handling. | Disk/memory grows with state size and reorg window. |
| Keep mempool policy local and bounded. | Limits RAM/CPU DoS and avoids making relay policy consensus. | Nodes may hold different candidates; basic eviction is gameable. |
| Use canonical Cell IDs and exact outer envelopes. | Shared encoding and proof structure, with separately committed execution availability. | Any grammar change changes commitments and requires protocol coordination. |
| Freeze actor bodies at the expiry boundary. | Linear ownership survives without bulk traversal/retirement. | Archives and transaction witnesses become necessary for recovery; permanent metadata still has a cost. |

## Launch-critical TBDs

Before this state machine can define production consensus, the project must
freeze and test:

1. archival receipt and persistent undo encodings;
2. state-sync verification rules;
3. authenticated core-block context and its behavior across Bitcoin reorgs;
4. protocol-version activation;
5. the failure-receipt/error taxonomy, concentrated-expiry work bounds, and all
   queue edge cases; and
6. persistence/crash recovery and the retained undo/state-sync policy.
