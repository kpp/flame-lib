# Merkle compression

Compression replaces Cell contents with authenticated commitments. Supplied
contents can later be checked against those commitments and used in execution.
This preserves integrity and ownership, not availability: owners or archives
must retain the bodies needed for recovery.

[Cells](cells.md) define pruning, commitment levels and virtualization;
[encoding](encoding.md) defines the CTL schemas, and the
[VM specification](flamevm.md) defines execution and ownership.

## Commitments and availability

An unloaded ordinary reference has the same commitment as its resident body.
An explicit pruned Cell instead has its own factual CellID and retains the
hashes and depths of the views it hides. Virtualization authenticates those
views; it does not recover missing contents. To prove a snapshot at factual
level `L`, prune at `L + 1` and check the original hash and depth at `L`.
Older cuts remain cuts in that snapshot.

Execution resolves bodies from attached values, the current actor's retained
storage and the initiating transaction's committed witness graph. Synchronous
calls and immediate asynchronous descendants share that transaction graph.
Explicit Init/Restore attachments can also carry graphs materialized from
the sender's storage, without exposing its unrelated bodies to the recipient.
Unrelated stores and private node caches cannot make missing data available.
Logical reads are gas-metered; retaining witnessed bodies in actor storage is
an explicit, rent-bearing state transition.

## Taproot programs

`open` authenticates a policy Cell's `hash(0)` against the predicate and inner
public key, then executes its program. Child programs use ordinary bytecode
dispatch (`runslice`, `loadref`, `exec`). Unselected branches may be pruned;
accessing a missing branch fails.

TxID and `signcall` bind factual CellIDs, including the supplied pruning
records. An outer proof of that transaction uses a fresh pruning level, so
opening it preserves the transaction's existing Taproot cuts.

## Instantiating UTXOs

`input` accepts the actual Contract Cell root. Its factual CellID is the
ContractID; its contents are the predicate, anchor and tagged payload Value.
Referenced payload data, including nested Dicts, can resolve lazily.

Authenticating public Cells does not create owned values. The Contract-input
boundary instantiates its payload; Utreexo membership and duplicate-input
checks enforce single consumption at the chain layer. Private prover
annotations are applied only after their public commitments match.

Newly pruning a full Contract changes its factual CellID. Spending such a
proof under the original ContractID would need an explicit binding to the
original view; `input` does not currently provide that operation. Ordinary
unloaded references preserve identity and already support lazy access.
Utreexo retains its specialized forest and proof format.

## Tries and Dicts

`cells::Trie` is a radix-4 Patricia trie with fixed-length byte keys. VM Dicts
use 32-byte Scalar paths and authenticated Value leaves. Reads and mutations
load only the required ordinary paths; unavailable data is a failure, never
evidence that a key is absent. Nested Dicts resolve independently.

Dict capability summaries remain authenticated even when values are hidden.
Pruning cannot make tokens droppable or Dicts copyable, and removing entries
does not reset sticky portability. Arbitrary serialized bytes cannot grant
ownership or establish trusted capability summaries.

Explicit selective pruning and persistent restoration would let an active
actor reduce rented Dict storage. These need a VM persistence boundary and
typed traversal of virtualized proofs; the current VM has no general
`prune`/`virtualize` instructions, and Trie decoding uses physical Cells.

## Freezing and unfreezing actors

An initialized or frozen actor's total record stores two references: the
content root and the protocol cell containing registry metadata. Code and
data belong to the same content root. Freezing replaces that whole root with
one pruned Cell. The actor's identity and ownership
of hidden values survive, while protocol metadata remains available.

The canonical [ActorAddress](encoding.md) selects `Init`,
`Address` or `Restore`. `Restore` supplies the immutable ActorID and a
possibly partial proof of the currently registered content snapshot. It
cannot replace that snapshot or rewind the actor to an older one.

Restoration projects supplied Cells back to the original snapshot: fresh cuts
become unloaded references, while older deliberate cuts remain pruned.
Compatible proofs merge with already retained bodies, so successive
transactions can restore different portions of a large actor. An unavailable
path still fails when read. Restoration, method execution and storage
purchases share a checkpoint; failure rolls them back together.
