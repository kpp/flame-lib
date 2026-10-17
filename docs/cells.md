# Flame Cells

Cell is an immutable content-addressed data structure used to encode all of the Flame's data types: transactions, blocks, contracts and tokens. Cells contain binary data (up to 4095 bytes) and up to 4 references to other cells, forming a directed acyclic graph of cells.

## What are cells

There are two kinds of Cell:

- **Ordinary:** a byte payload and an ordered list of child references.
- **Pruned:** no children; retained hashes and depths stand in for an omitted
  subtree. The pruning record is itself factual data with its own hash.

A **reference** carries a child's complete commitment: its level mask and
significant hash/depth pairs. It may also carry the child's body. **Resident**
and **unloaded** describe body availability, not different Cell kinds: unloading
preserves the commitment and every ancestor's identity. Either kind can be
resident or unloaded.

A bare hash identifies a body for lookup, but is insufficient to construct a
reference: it does not supply the other hashes and depths. Loading a body must
validate its complete expected commitment, not only its highest hash.

References are ordered; changing their order changes identity. Repeating a
reference is valid. Cells encode data, not ownership or authority to instantiate
the assets that data may describe.

### Limits

| Property | Limit |
| --- | --- |
| Ordinary payload | 0–4095 bytes |
| Ordinary references | 0–4 |
| Hash | 32 bytes, SHA-256 |
| Level mask | 15 bits; hash levels 0–15 |
| Depth | 0–65535 child edges |
| Pruned Cell | 1–15 retained hash/depth pairs; no references |
| Standalone ordinary record | At most 6281 bytes |
| Standalone pruned record | At most 512 bytes |

A parent adds one to child depth, so a child with depth 65535 at **any** level
cannot be attached. Proof levels and graph depth are independent.

## Hashes and levels

### Identity

**CellID** is the Cell's highest significant hash: the commitment to the factual
tree as supplied, including pruning records. An ordinary graph without pruning
has only level zero.

Lower hashes commit views in which some pruning records stand for their hidden
contents. A protocol must explicitly choose its expected commitment level:
the supplied proof and the underlying logical data are different objects.

### Significant levels

Level zero always exists. Mask bit `i` marks another significant level
`i + 1`; therefore a complete commitment has `popcount(mask) + 1` pairs.
A query in a gap uses the preceding significant pair; a query above the highest
level, but no higher than 15, uses the highest pair.

For mask `0b101`, the significant levels are 0, 1 and 3:

| Requested level | 0 | 1 | 2 | 3–15 |
| --- | --- | --- | --- | --- |
| Selected significant level | 0 | 1 | 1 | 3 |

An ordinary Cell's mask is the OR of its child masks. At each significant level,
its depth is zero without children, otherwise one plus the maximum child depth
at that level. A pruned Cell retains lower pairs and computes its own highest
pair; its highest depth is zero because the physical record has no children.
Its retained pairs are claims until authenticated against an expected root.

### Hash preimages

For each significant ordinary level `l`, in ascending order:

```text
D = payload_length | (reference_count << 12)
M = mask & ((1 << l) - 1)
data = original payload at level 0; previous significant hash otherwise

H(l) = SHA256(
    D:U16 LE || M:U16 LE || data
    || child depths(l):U16 LE, in reference order
    || child hashes(l):32 bytes, in reference order
)
```

The descriptor always contains the **original payload length**, including when
`data` is a previous 32-byte hash. Gaps reuse the preceding significant hash
and preimage. A pruned Cell's highest hash is SHA-256 of its entire
[pruned record](#canonical-cell-record); its lower hashes have no available
preimages.

There are no application domains, schema versions or double hashes in Cell
hashing. Residency does not affect hashes. An ordinary CellID is **not** the
SHA-256 of either its standalone record or graph transport: those encodings
carry different reference information.

## Canonical Cell record

A standalone record contains one body and complete child summaries, but no
child bodies. It supports independent reconstruction, storage accounting and
debugging. All integers below are little-endian.

```text
descriptor: U16
  bit 15 = 0: ordinary
    bits 12..14 = reference count, 0..4
    bits  0..11 = payload length, 0..4095
  bit 15 = 1: pruned
    bits  0..14 = nonzero level mask

ordinary record:
  descriptor
  payload
  child summaries, in reference order

child summary:
  level mask: U16, bit 15 zero
  hashes: 32 * (popcount(mask) + 1) bytes
  depths:  2 * (popcount(mask) + 1) bytes

pruned record:
  descriptor = 0x8000 | mask
  retained hashes: 32 * popcount(mask) bytes
  retained depths:  2 * popcount(mask) bytes
```

All hashes precede all depths, in increasing significant-level order. Child
summaries include the factual pair; pruned records omit that pair because it is
computed from the record itself. Ordinary masks are derived from child masks,
not repeated in the descriptor.

Reference counts 5–7 and pruned descriptor `0x8000` are invalid. Records are
self-delimiting, with no residency flags, type tags or versions. The largest
ordinary record is `2 + 4095 + 4*(2 + 16*34) = 6281` bytes; the largest pruned
record is `2 + 15*34 = 512` bytes.

## Pruning and virtualization

**Pruning at level p** replaces a subtree with a record preserving commitments
through `p - 1`. It requires `1 <= p <= 15` and `p >= source.level`. The new
mask is `(source.mask & ((1 << (p - 1)) - 1)) | (1 << (p - 1))`; retained pairs
come from the corresponding source levels. Partial proofs rebuild ordinary
ancestors around the replaced descendants.

Choose one target level throughout a proof:

- Prune more branches at level 1 to thin a proof of the original level-zero
  data. Its factual proof identity changes; its level-zero target does not.
- Prune at level 2 to prove the previous level-1 proof **as factual data**.
  Old level-1 pruning records remain unchanged.

A mask records significant commitments, not a count of pruning operations.

**Virtualization** is a read-only view capped at an absolute level. The initial
cap is between zero and the Cell's highest level; later virtualization can only
lower it. Children inherit the cap, with lower-level children aliasing their
highest pair. References still resolve by physical CellID and validate all
committed pairs.

A pruning record above the cap supplies a hash and depth but no readable
contents. At its own factual level it exposes the pruning record, not the
hidden subtree. Virtualization neither fetches omitted data nor substitutes a
different body with a matching lower hash; views have no physical encoding.

For example, an outer block proof can introduce level-2 cuts around a
transaction containing a level-1 Taproot proof. Viewing at level 1 preserves the
transaction's factual identity and its old cuts; Taproot separately checks
level zero. No wrapper Cell or level reset is required.

## Trie

A Trie is a radix-4 Patricia tree built entirely from ordinary Cells. Keys have
one fixed width chosen by the containing schema, currently **0–32 bytes**, and
are traversed as 2-bit digits, most-significant first.

```text
node payload:
  child_mask: U8
  label_length: U16 big-endian, in 2-bit digits
  label: packed digits, four per byte, with zero padding

node references:
  leaf: one value Cell
  branch: two to four child Cells, in ascending selector order
```

A node's label is the common path prefix. Mask zero denotes a leaf, whose
label completes the remaining key. Otherwise the low four mask bits identify
the next digit's child selectors 0–3; that selector consumes one additional
digit. Other mask bits, mismatched reference counts, excess label bytes and
nonzero padding are invalid. Branches with fewer than two children are
noncanonical: path compression merges them into the label. Labels must fit the
remaining path; a branch must leave at least one digit for its selector.

An empty Trie has no root. Raw nodes carry no key width, entry count or VM
capability flags; the owning schema supplies these separately. Traversal checks
only the required paths; unavailable data is an error, not an absent key.
Mutations rebuild the affected path atomically, sometimes needing a surviving
sibling for compression. Full import validates any enclosing claimed count
without preallocating from it.

The same Cells authenticate both paths and sibling commitments, so there is no
separate Merkle-path format. Current traversal uses physical Cells, not
virtualized proof views. VM Dict semantics are specified under
[Dict](flamevm.md#dict).

## Root-last graph transport

Transport packs one complete physical DAG, not standalone child summaries:

```text
packet:
  cell_count: V128, at least 1
  cell records, children before parents
  root: implicit final record

ordinary record at index i:
  ordinary descriptor: U16 LE
  payload
  backward reference distances: V128, in reference order

pruned record:
  canonical pruned record, unchanged
```

`V128` is shortest unsigned u64 LEB128, as defined by [CTL](../cells/ctl.md#type-reference).
A distance `d` selects record `i - d`, with `1 <= d <= i`.
Child summaries and hashes are reconstructed from preceding records; indexes
never enter hash preimages.

Canonical order is ordered depth-first postorder: visit children in reference
order, emit each body once after its children, deduplicating by factual CellID.
For `root -> [A, B]` with both A and B referencing leaf, order is
`leaf, A, B, root`. Repeated child references retain their positions.

Decoders reject zero count, noncanonical/overflowing integers, invalid records,
bad distances, duplicate IDs, unreachable records, alternate ordering, trailing
bytes and depth overflow. Work and allocation are bounded before they occur;
deep graphs require iterative traversal or an explicit small bound.

Every referenced physical body must be present. An intentional omission must
be represented by an explicit pruned Cell, changing the factual root while
preserving lower commitments. There are no extra roots, root-ID prefixes,
optional indexes or bag commitments. Multiple objects use an ordinary enclosing
Cell hierarchy.

## Cell indexes and typed snapshots

A **Cell index** maps original CellIDs to detached bodies. Detaching references
prevents one indexed body from implicitly supplying unlisted descendants.
An index defines exact availability, not another kind of Cell.

### A snapshot is an ordinary Cell hierarchy

```text
snapshot root:
  payload: body_count:U32 LE, original_level_cap:U8
  refs: one Trie root iff nonempty

Trie:
  key: original CellID, 32 bytes
  value: reference to the body's physical proof tree
```

The original cap is the maximum level of the indexed bodies, or zero for an
empty index. The root's factual hash commits the supplied set. Proof trees
contain only indexed bodies; absent descendants become fresh pruning records above the
original cap. Older cuts at or below that cap remain literal data.

Decoding projects bodies back to the original cap, checks every original ID
against its Trie key, and reconstructs detached bodies. Fresh cuts are not
indexed as available original bodies; prior hash/depth commitments and cuts
are preserved. The resulting index must reproduce the same snapshot root.

Missing bodies need one fresh pruning level. A sparse snapshot already at the
maximum level fails explicitly; a complete graph can use direct transport.
Snapshot encoding errors propagate through transaction rollback, not panics.

### Typed-object snapshots

A typed snapshot root has one byte of payload, the original root's level, and one
reference to the physical proof root. It carries a partial typed object while
preserving its original content view. This is an ordinary hierarchy, not a
transport-header variant. [Encoding](encoding.md) specifies which application
types use this envelope rather than direct root transport.

## Resolution and witness context

Body lookup must preserve the meaning of an exact availability commitment.
A reference read validates the full summary; a bare-ID lookup validates the ID.
Resolving an unloaded reference does not change its parent or persist the body.
Reading explicit pruning as ordinary payload fails.

In Flame execution, permitted sources are, in order:

1. a body attached to the value in RAM;
2. the current actor's consensus-retained code/state body set;
3. the initiating external transaction's committed witness hierarchy.

Registry-layout and lease-index metadata are not implicit actor witnesses.
Other actors' stores, private caches, archives, disk records outside the
committed scope and the network must not expand availability. A cache may
accelerate access only after scope membership is established. Fallback may
continue on a missing ID, never conceal integrity or resource errors.

Storage authority does not travel with arguments. Attached resident bodies may
cross a call/send boundary; remaining unloaded references use the recipient's
own retained set or the shared external witnesses, never the sender's store
merely because it originated the value. Returning restores the caller's scope.
All handoffs also obey [VM ownership rules](flamevm.md#types); hydration alone
does not create assets or buy storage.

The same immutable external witness scope covers synchronous calls and the
external transaction's entire asynchronous descendant closure. Transport
normalization unions inline resident bodies with the decoded lookup snapshot
before execution without changing lookup success. A minter cannot replace that
scope with a block-wide witness pool or omit supplied bodies.

Availability must be frozen **before proof-producing execution**, not merely
before signing: proving already derives TxID and binds it into the R1CS
transcript. Assemble a conservative witness set, or discover it in a separate
simulation, freeze it, then prove and sign against that scope. External proving
does not execute actor descendants; discovering their needs requires simulating
the closure or supplying a superset. Missing data fails at access.

Logical reads are metered independently of cache residency; the exact
[gas tariff](flamevm.md#cell-read-gas), [failure recovery](flamevm.md#failure-modes),
[persistence](storage.md#content-availability-and-recovery) and
[scheduling](blockchain.md#staged-block-application) have their own specifications.

## Comparison with TON Cells

Flame retains TON's immutable DAG, ordered references, SHA-256 commitments and
significant hash/depth levels, but is not wire-compatible.

| Aspect | TON | Flame |
| --- | --- | --- |
| Payload | Up to 1023 bits | Up to 4095 whole bytes |
| Child references | Up to four | Up to four |
| Levels | 0–3 | 0–15 |
| Reference depths | Two-byte big-endian | Two-byte little-endian |
| Cell kinds | Ordinary, pruned, Merkle proof/update and library references | Ordinary and pruned only |
| Proof views | Special proof/update Cells participate in level handling | Explicit absolute-level views, no wrappers |

TON's [Cell structure and representation](https://docs.ton.org/foundations/serialization/cells)
and [proof wrappers](https://docs.ton.org/foundations/serialization/merkle), together
with its [level-mask implementation](https://github.com/ton-blockchain/ton/blob/master/crypto/vm/cells/LevelMask.h),
are the basis for this comparison. Flame's descriptors and root-last transport
are deliberately its own format.

## Rust implementation

The `cells` crate depends on `sha2` and `thiserror`, not FlameVM or Utreexo.
It implements the structures above; exact method signatures belong in Rustdoc
and source, not a second API specification here.

| Component | Source |
| --- | --- |
| Cell, reference, commitment and virtual view | [cell.rs](../cells/src/cell.rs) |
| Builder and Slice | [builder.rs](../cells/src/builder.rs), [slice.rs](../cells/src/slice.rs) |
| Typed codecs and CTL | [codec.rs](../cells/src/codec.rs), [CTL specification](../cells/ctl.md) |
| Root-last transport | [transport.rs](../cells/src/transport.rs) |
| Index snapshots and proof projection | [index.rs](../cells/src/index.rs) |
| Trie | [trie.rs](../cells/src/trie.rs) |
| Resolution and metering hooks | [cell.rs](../cells/src/cell.rs) |

Cells and virtual views share immutable storage. References carry either a
resident body or a complete unloaded commitment; there is no bare-ID reference.
Builder and Slice are separate: writes append to bounded buffers, while reads
share a body with independent byte/ref cursors. Ordinary writes do not spill.
Stores check capacity and depth before mutation. Compound append-only stores
roll back lengths on error; tentative reads advance cursors only on success.
Finalization is infallible after successful stores. Exact typed decoding rejects
trailing bytes/refs and pruned payloads.

All body reads, including resident ones, pass through `CellReader` hooks;
`CellResolver` supplies ID lookups. Physical reads validate commitments before
returning data. `load_graph` performs transport resident-frontier normalization;
VM execution uses `load_graph_scoped` so reads are metered before traversal.
Structural errors propagate as `CellError`; codecs do not enforce VM
portability or ownership. Separate VM Values and opcodes are specified in
[FlameVM](flamevm.md#cell-bytereference-instructions).

### Remaining extensions

Virtualized Trie traversal, general VM pruning operations and disk adapters are
not implemented. Utreexo retains its [separate format](encoding.md#blocks-and-utreexo);
changed consensus identities require coordinated activation, not automatic
legacy-state migration. Broader work is tracked in [the plan](plan.md) and
[compression overview](compression.md).
