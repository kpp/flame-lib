# flamewallet-cli

`flamewallet`, a development wallet for a `flamed` node: something to play
with, to drive end-to-end tests with, and to script against. The library
behind it (`flamewallet_cli`) is the wallet; `main.rs` only parses
arguments and prints, so a terminal or graphical wallet can take the
library and leave the binary.

It signs through `flamewallet-ffi`, the facade Safely and the browser
wallet run, so a send from here goes through the same code as a send from a
phone.

**The wallet file holds the secret in the clear.** It is created readable
by its owner only, and that is all. Do not put real funds behind it.

## State

The wallet keeps one file (`./wallet.json`, or `--wallet` / `FLAMEWALLET`):
the secret, the network, the node URL and the receiving counter. Nothing
else is cached, and nothing needs to be: the node indexes outputs by
predicate, so a balance is one `unspent` call per watched address and a
history page is one `history` call, however long the chain.

Watched addresses are every receiving address issued (index 0 always) and
one change address, `m/…/1/0`, that every send returns change to.

## Commands

```console
$ flamewallet new                      # fresh mnemonic, prints it and the first address
$ flamewallet restore < phrase.txt     # from a mnemonic on stdin
$ printf 'a1%.0s' $(seq 64) | flamewallet restore --seed   # the devnet's genesis holder
$ flamewallet address [--new]
$ flamewallet balance
$ flamewallet coins
$ flamewallet send ADDRESS 12.5 [--memo TEXT] [--flavor HEX] [--fee SPARKS] [--wait]
$ flamewallet history [--limit N] [--older CURSOR]
$ flamewallet status TXID
$ flamewallet wait TXID
$ flamewallet tip
```

`--json` on any command prints one JSON value instead of text. Quantities
in JSON are exact decimal strings in base units (sparks for Flame), as the
node writes them; flavors are 64 hex digits.

`--node URL` (or `FLAMEWALLET_NODE`) names the node. `new` and `restore`
store it in the file; every other command uses it instead of the stored
one.

## Flavors

Every amount is per flavor. Flame (`FLM`) is written in flames with up to
eight decimals; any other flavor in whole units, since nothing records how
many decimals it has. `balance`, `coins` and `history` list every flavor
the wallet holds, Flame first.

`send --flavor HEX` pays another flavor. The fee is always Flame, so such
a send spends coins of both flavors and returns change in each.

The wallet cannot issue tokens yet: `flamewallet-ffi` has no issuance.

## Sending

A send reads the coins with the mempool applied, picks the largest coins of
each flavor it needs until they cover the amount (and the fee, in Flame),
proves, signs and submits. Coins created by a pending transaction, change
included, have no membership proof until they are mined, so they show as
pending and cannot be spent until the next block: wait with `--wait`
before sending again from the same coins.

If a block lands between reading the coins and submitting, the proofs are
stale and the node refuses the transaction; the send then rebuilds once
from fresh coins.

The default fee is 1000 sparks and the default gas limit 1 000 000, what
the node's tests and Safely use.

## Devnet smoke test

```sh
flamewallet-cli/scripts/devnet-smoke.sh
```

Starts a throwaway node with two-second blocks, restores the genesis
wallet, pays a fresh wallet with a memo, pays part of it back, and checks
every balance. `KEEP=1` keeps the scratch directory; `PORT` changes the
RPC port (default 18545).
