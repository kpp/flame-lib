# Merkle compression

Compression is authenticated omission, not a guarantee that omitted data can
be recovered. Owners or archives must retain the bodies needed later. This
document records the use cases and unfinished work; it does not define another
format or execution protocol.

The canonical definitions are [Cells and virtualization](cells.md#pruning-and-virtualization),
[typed encodings](encoding.md), [VM execution](flamevm.md) and
[actor storage](storage.md#content-availability-and-recovery).
Body availability and authority are specified by the
[resolution context](cells.md#resolution-and-witness-context).

## Commitments and availability

An important design distinction is unloading versus explicit pruning:
unloading changes availability, while pruning changes the factual graph.
See the [Cell model](cells.md#logical-structure) and
[commitment levels](cells.md#significant-levels) for the exact rules.
A proof of a proof must preserve the first proof's existing omissions;
[multilevel virtualization](cells.md#pruning-and-virtualization) provides that
composition without wrapper Cell kinds.

## Taproot programs

A policy author can commit several executable branches while revealing only
those needed for one invocation. The canonical
[`open` semantics](flamevm.md#open) specify authentication and branch execution;
[transaction identities](encoding.md#transactions-and-execution-witnesses)
bind the factual supplied proof rather than only the hidden policy commitment.

## Instantiating UTXOs

A Contract's public Cell graph can carry its authenticated payload without
granting ownership merely through serialization. The
[`input` boundary](flamevm.md#input) and
[chain membership/spend checks](blockchain.md#contracts-and-utreexo) provide that
authority.

Ordinary unloaded references already support lazy access without changing
ContractID. Spending a newly pruned proof under the original full ContractID
would additionally require explicit original-view binding; `input` does not
currently expose that operation. Utreexo's specialized codec remains deferred.

## Tries and Dicts

Cell paths can reveal a narrow portion of a large dictionary. The
[Trie format](cells.md#trie) defines the path structure, while
[Dict](flamevm.md#dict) owns capabilities and the distinction between complete
native maps and incomplete cell-backed caches. Missing data must never be
mistaken for an absent key or released asset ownership.

Explicit selective pruning and persistent restoration could let an actor
reduce rented Dict storage. This still needs a VM persistence boundary and
typed traversal of virtualized proofs: the current Trie traverses physical
Cells, and general VM `prune`/`virtualize` instructions are not implemented.

## Freezing and unfreezing actors

Freezing preserves ownership instead of destroying hidden linear values.
[Actor lifecycle and atomic Restore](flamevm.md#actors) define recovery behavior;
[storage](storage.md#content-availability-and-recovery) defines residency and
capacity, and [encoding](encoding.md#actor-addresses-state-and-messages) defines
the combined content root and registry record.

Successful restore-only executions can accumulate a large state over multiple
transactions. This avoids requiring one complete restoration transaction.
Archive availability and the economics of permanently retained frozen registry
records remain separate design work.
