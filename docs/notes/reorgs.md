# How to store things to survive reorgs

## First iteration

Start by collecting all valid bitcoin chains (valid txs, valid min difficulty, valid timestamps).

Within each bitcoin chain, collect all valid Flame chains. Min minting difficulty: 8192 sat per block, halving every four years.

Within each Flame chain collect all valid iblock subchains.

Validating a chain means verifying syntax of the transactions and blocks, as well as computing the state by applying transactions to it, from block to block.

To determine validity and finality of the payment:
- Choose a bitcoin main chain.
- Calculate weights of burn proofs on that chain.
- Choose the most-weight Flame chain with the proofs on bitcoin main chain.
- Take the state on the latest valid iblock on the most-weight Flame chain.
- Check if the required payment is in the state and accept the payment if it's sufficiently deep (inside an iblock, or inside a core block at some depth).

Limitations of this scheme:
- requirement to store all blocks,
- redoing all the work to compute the latest state,
- redownloading all the blocks by new nodes.

## Second iteration

- Every block is a tree of cells.
- Every block commits to a pruned state formed after applying that block.
- Every transaction is a tree of cells. 
- Every transaction commits to a pruned list (trie) of its effects.
- Nodes store last N blocks. N+1 block is removed from storage. Reorging past N blocks is impossible. N is decided by each node.
- Nodes store full trees of cells for rebuilt state as they verify transactions. This is the state for each stored block.
- All stored cells are deduplicated: unchanged parts of state are represented by unchanged cells that are not stored twice, but can be referenced by multiple parent cells. Basic reference counting needed to garbage-collect orphan cells.

Limitations addressed:
- requirement to store all blocks: solved, each nodes stores comfortable amount of recent blocks.
- redoing all the work to compute the latest state: the state is now stored explicitly.
- redownloading all the blocks by new nodes: new nodes can choose a well-known trusted block and a corresponding state commitment and continue from there.

New limitations:
- even with deduplication, storing copies of state for each of N blocks may be excessive, especially since reorgs are rare.

## Third iteration

Same as above, but store states for log(N) blocks selected as follows:

Keep one state snapshot per trailing-zero level of the block height. 0 trailing zeroes: level=0 (1,3,5,7...), 1 trailing zero (2,6,10,...): level=1 and so on.

```
level = trailing_zero_bits(height)
snapshots[level] = state_after_block(height) // replaces the previous snapshot when block at new height arrives.
```

The node still stores N last blocks of its choice, but only log(N) interim states. 
The shared portions of the state trees are deduplicated via the general cell-storage mechanism.

## Reorgs

Flame branches can keep their state snapshots independently. Most of the storage is shared and deduplicated, so reorg is a simple switch to a different tip. However, the interesting thing is how to cleanup and transfer mempool transactions to the new tip. Especially given their utreexo proofs.


## Utreexo proofs adjustments

tbd: when utreexo proof is transmitted between nodes at mempool level, every pair of peers decides on the most effective (short) part of the tree based on what's cached by the node. Then, the node unpacks the proof to the full size and then "catches up" to the current block - by adjusting the proof cells to the updated states, from block to block. Questions: how would we 





