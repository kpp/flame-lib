# Actor storage

This document defines the consensus rules for persistent actor storage. VM
opcode encodings and stack behavior are specified in
[flamevm.md](flamevm.md).

The `flamechain` actor registry and the FlameVM storage opcodes implement this
lease model.

Actors lease storage by burning Flame. Storage is not a token: a lease cannot
be transferred, shortened, or refunded. Every lease lasts exactly one storage
year, and its bytes return to the global pool only when the lease expires.

## Units and parameters

All byte quantities exposed to FlameVM are literal bytes. Lease purchases and
the global pool are restricted to whole `STORAGE_UNIT_BYTES` units.

| Parameter | Initial value |
| --- | ---: |
| `STORAGE_UNIT_BYTES` | `1_024` |
| `INITIAL_POOL_BYTES` | `128 * 1_024 * 1_024 = 134_217_728` |
| `INITIAL_POOL_UNITS` | `131_072` |
| `LEASE_DURATION_CORE_BLOCKS` | `52_500` |
| `ISSUED_BYTES_PER_CORE_BLOCK` | `8 * 1_024 = 8_192` |
| `ISSUED_UNITS_PER_CORE_BLOCK` | `8` |
| `MIN_LEASE_BYTES` | `1_024` |
| `MIN_REMAINING_POOL_BYTES` | `1_024` |
| `LEASE_RECORD_BYTES` | `16` |
| `INITIAL_PRICE_SPARKS_PER_UNIT` | `10 * SPARKS_PER_FLAME = 1_000_000_000` |

`SPARKS_PER_FLAME` is the native [Flame denomination](flamevm.md#tokens).
Storage fees use those integral sparks.

A core-block height is the protocol's Bitcoin-coupled block counter. Lease
duration and expiry use that counter, never timestamps; every internal
transaction executed in one core block observes the same height.

The fixed price numerator is derived from the initial conditions:

```text
STORAGE_PRICE_PRODUCT
    = INITIAL_POOL_UNITS * INITIAL_PRICE_SPARKS_PER_UNIT
    = 131_072_000_000_000 sparks * units
```

These names identify consensus parameters. Their initial values may be tuned
before launch; changing one on a live network requires an explicit protocol
upgrade. `STORAGE_PRICE_PRODUCT` is derived once from the activated initial
values and does not change as bytes are issued, recycled, or purchased.

The exact global unit price is represented by two integers:

```text
price_dividend = STORAGE_PRICE_PRODUCT
price_divisor  = available_storage_units
unit_price     = price_dividend / price_divisor
```

Only the divisor changes. No rounded or floating-point base price is stored.
Storage units, byte amounts, expiry heights, and the available pool are
consensus `u64` values; every conversion and update is checked.

## Global state and block order

The chain state contains:

- `available_storage_units`, the unleased reserve;
- a derived expiry index containing every outstanding lease; and
- for each actor, an ordered list of `(expiry_height, units)` leases.

Leases with the same actor and expiry height must be coalesced. The expiry index
survives actor destruction because destroying an actor does not refund its
unexpired leases. It is a cache derived from the committed per-actor lease
lists; block validation checks that both representations match before commit.

The storage supply invariant is: the activated initial pool plus all units
issued so far equals available units plus all outstanding lease units, including
leases retained after actor destruction. Purchases allocate existing units;
they do not issue storage. Expiry returns units to the pool without changing
this total.

At the beginning of core block `h`, before executing transactions:

1. Remove leases whose `expiry_height == h` and return their units to the pool.
2. Mark every actor whose expiring lease leaves its capacity below usage for freezing.
   Such actors cannot execute in this block.
3. Add `ISSUED_UNITS_PER_CORE_BLOCK` to the pool.

Storage purchases then execute serially in transaction order and update the
pool immediately. This makes every quote a deterministic function of preceding
block execution.

At the end of the block, marked actors are frozen in lexicographic actor-ID
order. Their entire ActorState root becomes one explicit pruned
Cell; application bodies are removed, but identity, size metadata and
outstanding leases remain. No system internal
transaction is generated and no token or other linear value is retired.
Unexpired leases recycle only at their original expiration heights.

Freezing affects only content, not protocol metadata; the actor-state and
registry layouts are specified in
[Cell-based encoding](encoding.md#actor-addresses-state-and-messages).

A later transaction uses the canonical ActorAddress Restore variant with an
authenticated partial image. This is not redeployment: actor identity and
linear ownership remain intact. Code and data are committed by one root,
not frozen independently; the registry still retains metadata, so the record
is not literally only 32 bytes. Explicit self-destruction
after dismantling checked-out state remains a separate operation.

## Actor usage and capacity

Actor usage is measured deterministically as:

```text
usage = sum(canonical Cell record bytes in the resident ActorState/code/data graph)
      + LEASE_RECORD_BYTES * number_of_lease_records
```

Shared resident bodies are counted once within one actor's graph, including
the ordinary ActorState root. Unloaded ordinary descendants contribute only
parent reference summaries, not missing body bytes; retained explicit pruning
records count their own encoded bytes. A fully frozen actor with no remaining
leases has zero charged usage, although its registry metadata remains.

Each coalesced lease record retains the policy charge for two canonical `u64`
fields: expiry height and storage units. Registry/header and lease-Trie wrapping
are not additionally charged; bounding the permanent metadata of frozen actors
remains a node/protocol design concern rather than a claim that it is free.

An actor's capacity at core-block height `h` is:

```text
capacity(h) = STORAGE_UNIT_BYTES
            * sum(lease.units where lease.expiry_height > h)
```

Expiration is exclusive: bytes from a lease expiring at height `E` are not
available during block `E`.

An existing actor must satisfy `usage <= capacity(current_height)` whenever its
state is committed with `save`. A newly constructed actor may execute
provisionally so it can purchase its first lease, but its creating transaction
commits only if the same invariant holds at the end. Failure rolls back the
actor, purchases, burns, and all other transaction effects.

Execution memory does not count toward persistent usage, and storage capacity
does not grant execution RAM. FlameVM charges variable-size allocation work to
the active frame's gas budget. A provisional constructor therefore needs gas,
not a storage-derived bootstrap allowance, to execute `addstorage`. The
transaction-end `usage <= capacity` check remains unchanged. Fetching a missing
body into the current execution is not a storage purchase or persistent restore.

### Content, availability, and recovery

Actor layouts and their ordered lease Tries are specified in
[Cell-based encoding](encoding.md#actor-addresses-state-and-messages).
The registry commits the exact retained graph's snapshot CellID.
Initialized images can be partially available; frozen images contain one
whole-root cut. Actor images reserve the outermost available pruning level
so expiry cannot fail because another proof level is unavailable.

Resolver sources are specified in
[Resolution and witness context](cells.md#resolution-and-witness-context).
Loaded bodies do not silently become rent-bearing. `save` and `setcode` retain newly constructed
reachable bodies and previously owned reachable bodies; they do not union the
external witness bag into actor storage. Restore explicitly merges supplied
bodies without erasing prior retention; its authentication and atomic execution
rules are specified in [Actors](flamevm.md#actors). The actor buys any required
capacity itself during that execution. Removed branches cease to contribute
to stored availability or rent. Partial Dicts retain their authenticated
portability and linearity metadata when their descendants are absent.

Canonical storage reads must not depend on earlier executions' private witnesses
or host caches. Metering follows [Cell-read gas](flamevm.md#cell-read-gas);
checkout ownership and failure recovery follow [`load`](flamevm.md#load),
[`save`](flamevm.md#save) and [Actors](flamevm.md#actors).

`Blockchain::actor_storage` returns a read-only `StoredActor` snapshot containing
the layout root and exact resident Cell index for archival and witness construction.
Expiry freezing and ActorAddress restoration are implemented. A dedicated VM
operation for arbitrary partial pruning is not yet provided.

## Pricing

Let:

- `R` be `available_storage_units` immediately before an operation;
- `q` be the requested byte amount;
- `Q = q / STORAGE_UNIT_BYTES` be the requested number of units; and
- `K` be `STORAGE_PRICE_PRODUCT`.

The current marginal price of one unit is the exact rational `K / R`. A quote
uses the reserve after the requested purchase:

```text
R_after    = R - Q
fee_sparks = ceil(Q * K / R_after)
```

Equivalently, the quoted unit price is `K / R_after`, multiplied by `Q` and
rounded up once. Implementations must use checked wide intermediates and the
identity `ceil(a / b) = a / b + (a % b != 0)`; floating-point arithmetic is
forbidden.

A request is unavailable when:

- its byte amount is less than `MIN_LEASE_BYTES`;
- its byte amount is not a multiple of `STORAGE_UNIT_BYTES`;
- `R_after` would be below `MIN_REMAINING_POOL_BYTES / STORAGE_UNIT_BYTES`;
- the byte amount, pool update, or expiry height does not fit its consensus
  `u64` representation;
- an intermediate calculation overflows; or
- the resulting positive fee cannot be represented as a `Scalar` with a
  non-negative centered `ClearToken` quantity.

The final unit is therefore never purchasable: its quoted price is
mathematically unbounded.

### Default quote vectors

These exact values pin rounding and the reserve boundary for the initial
parameters:

| Request from the initial pool | Reserve after | Fee (sparks) | Result |
| ---: | ---: | ---: | --- |
| `1_024` bytes at height `10` | `131_071` units | `1_000_007_630` | lease expires at height `52_510` |
| `134_216_704` bytes | `1` unit | `17_179_738_112_000_000_000` | available |
| `134_217_728` bytes | `0` units | — | unavailable |

Storage-purchase effects and registry commitments use the canonical
[TxLog](encoding.md#txlog) and
[actor registry](encoding.md#actor-addresses-state-and-messages) encodings.
Tests cover exact price vectors, content/availability commitments, canonical
public storage reads, expiry freezing, and checkpoint/reorganization recovery.

This endpoint-price formula is deliberately **not path independent**. Splitting
one large request into many minimum-size purchases pays the successive marginal
prices and is cheaper than pricing the whole batch at its final, highest price.
That is equivalent to adopting a discrete harmonic marginal-price curve, not a
sybil-resistant bulk premium. If bulk and split purchases must cost exactly the
same, this formula must be replaced before activation by a state-potential
delta; per-actor or per-transaction purchase limits do not solve the sybil case.

## Purchasing and quoting

Only an actor may purchase storage. It may fund itself from Flame in its state,
its one call argument, or a delivered message. There is no transferable virtual-
byte asset and callers do not allocate storage directly to callees.

[`quotestorage`](flamevm.md#quotestorage) is read-only.
[`addstorage`](flamevm.md#addstorage) applies the same pricing rules, immediately
deducts the units from the pool, and adds a lease expiring at:

```text
current_height + LEASE_DURATION_CORE_BLOCKS
```

The VM's storage-purchase debt must be balanced with actual Flame. A successful
purchase burns that amount irrevocably; minters do not collect it. Debt creation
and validation are specified with [`addstorage`](flamevm.md#addstorage) and
[`retire`](flamevm.md#retire).

`quotestorage` is advisory, not a reservation. Any intervening successful
purchase changes the reserve and therefore the result of a later `addstorage`.
Both operations have the same request, reserve, arithmetic, and representation
checks. Insufficient Flame is not an opcode failure: an unbalanced transaction
fails at finalization and rolls the purchase back.

## Introspection

Actors can query storage without receiving the underlying lease list. Stack
interfaces, permitted contexts and failures are specified with
[`height`](flamevm.md#height), [`usage`](flamevm.md#usage),
[`capacity`](flamevm.md#capacity), [`quotestorage`](flamevm.md#quotestorage) and
[`addstorage`](flamevm.md#addstorage).
