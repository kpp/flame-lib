# Cell-based encoding

FlameVM values are [Cells](cells.md), described using [CTL](../cells/ctl.md).
Fields follow declaration order: inline fields append payload bytes; `^T`
appends a child reference. Only enums add tags. Integers are little-endian.
Human-readable formats are specified in [Addresses](address.md).

## Primitives and containers

| Type | Encoding |
| --- | --- |
| `U8 / U16 / U32 / U64` | Fixed-width unsigned LE integer. |
| `V128` | Canonical unsigned LEB128, bounded to U64. |
| `[U8; N]` | Exactly N bytes, no length prefix. |
| `Cell` | 0..4095 payload bytes, 0..4 refs, or explicit pruning record. |
| Scalar | Canonical 32-byte LE residue below the Ristretto scalar order. |
| Point / Predicate | 32-byte CompressedRistretto representation; curve validation at cryptographic use. |
| Byte blob | Dedicated zero-ref Cell, 0..4095 raw payload bytes with no length prefix. |
| Trie | One radix-4 Patricia Cell format; owning schema fixes key width. |

`CellID` is the highest-level factual hash. References commit hash/depth
summaries; pruning preserves lower commitments but changes the factual ID.
Transport is a root-last DAG: V128 record count, Cell records and backward
V128 reference distances. See [Cells](cells.md) for records and proof levels.

Schemas below share preceding declarations. Conditional refs use handwritten
codecs; CTL does not automatically spill fields into children. Applications
define explicit layouts for larger pure-data transfers.

## Values and Contracts

```ctl
type Hash = [U8; 32];
type Scalar = [U8; 32];
type Point = [U8; 32];

struct Token { qty: Point, flavor: Point }
struct ClearToken { qty: Scalar, flavor: Scalar }

enum Value: U8 {
    Scalar { value: Scalar } = 0,
    Dict { dict: ^Cell } = 2,
    Point { point: Point } = 3,
    Token { token: Token } = 4,
    ClearToken { token: ClearToken } = 6,
    Cell { cell: ^Cell } = 13,
}

struct DictHeader { count: U64, flags: U8 }
// Dict: header plus one Trie ref iff count != 0.

struct Contract {
    predicate: Point,
    anchor: Hash,
    payload: Value,
}
```

Dict keys reverse canonical Scalar bytes into 32-byte BE ordered paths.
The count and sticky flags belong to the Dict, not the Trie: portable bit 0,
droppable bit 1; other bits are invalid. A drained Dict is droppable regardless
of its sticky bit. No special list encoding exists.

Other Value tags and runtime-only types are rejected. Private witnesses never
enter encoding or hashes. ContractID is the Contract root's factual CellID;
`input` takes that root. Authenticated Dict summaries can be read lazily;
arbitrary imports validate their contents. Portability and ownership are
enforced by the VM separately from layout validation.

## Programs and predicates

Programs are raw bytecode in Cell payloads. `pushcell` consumes a program ref;
`exec` enters its continuation. There is no length prefix or automatic spilling.

A Taproot predicate is a compressed key, with the tweak committing
`policy.hash(0)`. `open` executes the supplied policy Cell; optional child
selection uses ordinary `runslice / loadref / exec` bytecode. Unselected branches
may be pruned. `signcall` binds `policy.id()`, the factual highest-level hash.

## Actor addresses, state and messages

[actor.ctl](../flamevm/actor.ctl) is the canonical address schema:

```ctl
type ActorID = [U8; 32];

enum ActorAddress: U8 {
    Init { ctor: ^Cell } = 0,
    Address { addr: ActorID } = 1,
    Restore { addr: ActorID, state: ^Cell } = 2,
}

struct ActorState { code: ^Cell, data: ^Cell }

enum Caller: U8 {
    None = 0,
    Some { actor: ActorID } = 1,
}

struct Message {
    anchor: Hash,
    target: ActorAddress,
    caller: Caller,
    refund: Point,
    gas: U64,
    arguments: ^Cell,
}
```

ActorID is the constructor's factual CellID: 32 bytes, without a tag or reversal.
`send` and `call` take an ActorAddress Cell. ActorState has empty payload and two
refs: code and a portable Value root. Freeze prunes the entire ActorState;
Restore authenticates it against the saved snapshot, preserving older cuts.
See [Actors](flamevm.md#actors) for execution and rollback rules.

Message arguments are a Dict with consecutive keys `0..count-1`. MessageID is
the Message Cell's factual ID; each send ratchets the anchor for uniqueness.

Actor registry and lease shapes:

```ctl
struct ActorRegistryHeader { count: U64 }
// Optional Trie ref; 32-byte BE ActorID keys.

struct ActorEntry { availability: Hash, slot: ^Cell }

enum ActorSlot: U8 {
    LeasesOnly { leases: ^Cell } = 0,
    Present { state: ^Cell, protocol: ^ActorProtocol } = 1,
}
struct ActorProtocol { code_bytes: U64, state_bytes: U64, leases: ^Cell }

struct LeasesHeader { count: U32 }
// Optional Trie ref; U64 BE expiry keys, U64 LE unit-count leaves.
```

Present stores the content root and protocol cell as two refs. Freezing prunes
only content; protocol metadata survives. LeasesOnly retains outstanding leases
after destruction. Storage charging is specified in [Actor storage](storage.md).

## Transactions and execution witnesses

[tx.ctl](../flamevm/tx.ctl) generates the transaction codecs; ProgramGraph is
the handwritten program wrapper:

```ctl
struct Tx {
    body: ^TxBody,
    signature: [U8; 64],
    r1cs_length: U16,
    r1cs_proof: [U8; r1cs_length],
}
struct TxBody { header: TxHeader, program: ^Cell, log: ^TxLog }
struct TxHeader { version: U32, locktime: U32 }
struct TxLog { entry_count: U64, entries: ^Cell }
// External logs are nonempty; generic empty logs omit the entries ref.
struct ProgramGraph { code: ^Cell, witnesses: ^Cell }
```

ProgramGraph witnesses have U32 count, U8 original cap and a 32-byte-keyed Trie
ref iff nonempty, mapping original factual IDs to proof bodies. This immutable
transaction scope is shared by calls and async descendants. Private prover
assignments are excluded; caches and other transactions do not enlarge it.

The transmitted log is a canonical mask-one pruning Cell retaining the computed
log's hash(0) and LE depth(0). It is checked by re-execution, not decoded as a log.
TxID = TxBody.id(); WitnessID = Tx.id(); EffectID = computed TxLog.hash(0).
Internal execution IDs use their factual computed logs, including Receive.

An all-zero signature means absent authorization; otherwise it must parse as
Schnorr. The inline R1CS proof is at most 4029 bytes.

### TxLog

TxLog entries use consecutive U64 BE Trie keys:

```ctl
enum TxEntry: U8 {
    Header { header: TxHeader } = 0,
    Data { blob: ^Cell } = 1,
    Input { contract: Hash } = 2,
    Receive { message: Hash } = 3,
    Output { contract: ^Contract } = 4,
    IssuePub { qty: Scalar, flavor: Scalar } = 5,
    IssuePriv { qty: Point, flavor: Point } = 6,
    Retire { qty: Point, flavor: Point } = 7,
    Fee { sparks: U64 } = 8,
    ActorSave { actor: ActorID, data: ^Cell } = 9,
    SetCode { actor: ActorID, code: ^Cell } = 10,
    Send { message: ^Message } = 11,
    StoragePurchase { actor: ActorID, bytes: U64, expiry: U64, fee: Scalar } = 12,
    ActorDestroy { actor: ActorID } = 13,
    ActorDeploy { actor: ActorID, code: ^Cell } = 14,
    ActorRestore { actor: ActorID, state: ^Cell } = 15,
}
```

Data is a raw byte blob in one zero-ref Cell, at most 4095 bytes, without a
length prefix. ActorSave data is a Value root; ActorRestore state is the
supplied ActorState proof. External Tx transport carries its root directly.
Typed CellEnvelope snapshots wrap U8 cap and one proof-root reference.

## Blocks and Utreexo

```ctl
struct Block { header: ^BlockHeader, transactions: ^Cell }
struct BlockTx { gas: U64, tx: ^Tx, proofs: ^Cell }
struct BlockHeader {
    version: U32, height: U64, core_block: Hash, parent: Hash,
    witnesses: Hash, effects: Hash, contracts: Hash, actors: Hash,
    available_storage_units: U64,
}
enum ExecutionKind: U8 { External = 0, Internal = 1, Failed = 2 }
struct ExecutionRecord { kind: ExecutionKind, txid: Hash }
struct SequenceHeader { count: U32 }
// Optional Trie ref; consecutive U32 BE keys, expected-type leaves.
```

BlockHeader has 212 payload bytes and no refs; BlockHash is its CellID.
Witness root is the transaction sequence's factual ID; effects root is the
execution-record sequence's factual ID. Block transport uses CellEnvelope.

Utreexo proofs keep their existing `merkle / readerwriter` byte encoding,
stored as raw payloads in dedicated zero-ref Cells inside BlockTx, with no
additional length prefix. Each depth-63 proof fits the 4095-byte Cell limit.
These Cell layouts change consensus identities; no legacy decoder or state
migration is provided.
