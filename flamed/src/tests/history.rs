//! `history`: a wallet's transactions, paged both ways from a cursor.

use flamekd::util::{CHANGE, RECEIVING};
use flamekd::Network;
use flamepayments::InputSpec;
use flamevm::ContractID;
use tempfile::TempDir;

use super::*;
use crate::node::{Node, NodeError, GENESIS_TXID};

/// Addresses as a wallet would paste them.
fn addresses<K: AccountKey>(account: &Account<K>, paths: &[(u32, u32)]) -> Vec<String> {
    paths
        .iter()
        .map(|&(branch, n)| {
            account
                .address_at(branch, n)
                .expect("address")
                .to_bech32(Network::Testnet)
        })
        .collect()
}

/// A pays B `qty` from `from`, worth `have`: the change at `CHANGE/n - 1`,
/// or the genesis allocation for `n == 0`. The rest goes to `CHANGE/n`,
/// whose id comes back.
fn pay(
    node: &mut Node,
    a: &SpendAccount,
    b: &SpendAccount,
    n: u32,
    (from, have): (ContractID, u64),
    qty: u64,
    rng: &mut StdRng,
) -> ContractID {
    let input = if n == 0 {
        InputSpec::clear(
            contract_of(node, &from),
            live(node, &from),
            a.spending_key_at(RECEIVING, 0).expect("key"),
        )
        .expect("clear input")
    } else {
        let (contract, opening) = received(node, a, CHANGE, n - 1);
        InputSpec::confidential(
            &contract,
            &opening,
            live(node, &from),
            a.spending_key_at(CHANGE, n - 1).expect("key"),
        )
        .expect("A opens its change")
    };
    let to_b = output(b, RECEIVING, n, qty);
    let to_change = output(a, CHANGE, n, have - qty - FEE);
    let (packaged, contracts) = signed_transfer(vec![input], &[to_b, to_change], FEE, rng);
    node.submit(&submitted(&packaged)).expect("A's payment");
    paying(&contracts, a, CHANGE, n).id()
}

fn heights(entries: &[flamed_rpc::HistoryEntry]) -> Vec<u64> {
    entries
        .iter()
        .map(|entry| entry.height.expect("confirmed"))
        .collect()
}

#[test]
fn history_pages_both_ways_from_its_cursors() {
    let dir = TempDir::new().expect("temp dir");
    let a = account(&SEED_A);
    let b = account(&SEED_B);
    let mut rng = rng(90);
    let (genesis, cfg) = devnet(dir.path(), &a);
    let mut node = Node::open(&genesis, &cfg).expect("open");
    let genesis_id = genesis.contracts[0].id.0;
    let wallet = addresses(&a, &[(RECEIVING, 0), (CHANGE, 0), (CHANGE, 1), (CHANGE, 2)]);

    // Before anything happens: the genesis allocation, and a newer cursor
    // at the tip.
    let first = node.history(&wallet, None, 10, false).expect("history");
    assert_eq!(first.predicates.len(), 4);
    assert_eq!(first.entries.len(), 1);
    let allocation = &first.entries[0];
    assert_eq!(allocation.txid.0, GENESIS_TXID.0);
    assert_eq!(allocation.height, Some(0));
    assert_eq!(allocation.fee_sparks, "0");
    assert_eq!(allocation.created[0].id.0, genesis_id);
    assert!(allocation.spent.is_empty());
    assert_eq!(first.older, None);
    assert!(!first.has_newer);
    assert_eq!(first.pending, None);
    let head = first.newer.expect("a newer cursor");
    assert!(head.starts_with("newer:0:end:"), "{head}");

    // A pays B. Pending, it is on a first or newer page with the mempool,
    // and on no confirmed page.
    let change0 = pay(
        &mut node,
        &a,
        &b,
        0,
        (genesis_id, GENESIS_SPARKS),
        100 * FLAME,
        &mut rng,
    );
    let pending = node.history(&wallet, None, 10, true).expect("history");
    assert_eq!(pending.entries, first.entries);
    let waiting = &pending.pending.as_ref().expect("asked for")[0];
    assert_eq!(waiting.height, None);
    assert!(waiting.received_at.is_some());
    assert_eq!(waiting.fee_sparks, FEE.to_string());
    assert_eq!(waiting.spent[0].id.0, genesis_id);
    assert_eq!(waiting.created[0].id.0, change0);
    let b_side = node
        .history(&addresses(&b, &[(RECEIVING, 0)]), None, 10, true)
        .expect("history");
    assert!(b_side.entries.is_empty());
    assert_eq!(b_side.pending.expect("asked for")[0].created.len(), 1);
    let unchanged = node
        .history(&wallet, Some(&head), 10, true)
        .expect("history");
    assert!(unchanged.entries.is_empty());
    assert_eq!(unchanged.pending.expect("asked for").len(), 1);
    assert_eq!(unchanged.newer.as_deref(), Some(head.as_str()));

    // Confirmed, it comes from the head cursor exactly once: the spend of
    // the allocation and the change, two addresses, one entry.
    node.mint_block().expect("block 1");
    let caught = node
        .history(&wallet, Some(&head), 10, true)
        .expect("history");
    assert_eq!(heights(&caught.entries), [1]);
    assert_eq!(caught.entries[0].spent[0].id.0, genesis_id);
    assert_eq!(caught.entries[0].created[0].id.0, change0);
    assert_eq!(caught.entries[0].time, node.block_time(1));
    assert_eq!(caught.older, None);
    assert!(!caught.has_newer);
    assert_eq!(caught.pending, Some(Vec::new()));
    let head = caught.newer.expect("a newer cursor");

    // Two more payments, from change to change.
    let change1 = pay(
        &mut node,
        &a,
        &b,
        1,
        (change0, GENESIS_SPARKS - 100 * FLAME - FEE),
        50 * FLAME,
        &mut rng,
    );
    node.mint_block().expect("block 2");
    let _ = pay(
        &mut node,
        &a,
        &b,
        2,
        (change1, GENESIS_SPARKS - 150 * FLAME - 2 * FEE),
        25 * FLAME,
        &mut rng,
    );
    node.mint_block().expect("block 3");

    // Forward from the head, one at a time: `has_newer` says to go on.
    let step = node
        .history(&wallet, Some(&head), 1, false)
        .expect("history");
    assert_eq!(heights(&step.entries), [2]);
    assert!(step.has_newer);
    let step = node
        .history(&wallet, step.newer.as_deref(), 1, false)
        .expect("history");
    assert_eq!(heights(&step.entries), [3]);
    assert!(!step.has_newer);

    // Backward from the top, one at a time, down to the allocation.
    let mut page = node.history(&wallet, None, 1, false).expect("history");
    assert!(page.newer.is_some());
    let mut walked = heights(&page.entries);
    while let Some(older) = page.older.clone() {
        page = node
            .history(&wallet, Some(&older), 1, false)
            .expect("history");
        assert_eq!(page.newer, None);
        walked.extend(heights(&page.entries));
    }
    assert_eq!(walked, [3, 2, 1, 0]);

    // The same walk in one page, with addresses in another order and
    // repeated: the same history.
    let mut shuffled = wallet.clone();
    shuffled.reverse();
    shuffled.push(wallet[0].clone());
    let all = node.history(&shuffled, None, 10, false).expect("history");
    assert_eq!(heights(&all.entries), [3, 2, 1, 0]);
    assert_eq!(all.older, None);
    assert_eq!(all.predicates.len(), 5);
}

#[test]
fn history_refuses_cursors_it_cannot_honor() {
    let dir = TempDir::new().expect("temp dir");
    let a = account(&SEED_A);
    let b = account(&SEED_B);
    let (genesis, cfg) = devnet(dir.path(), &a);
    let mut node = Node::open(&genesis, &cfg).expect("open");
    node.mint_block().expect("block 1");
    let wallet = addresses(&a, &[(RECEIVING, 0)]);
    let head = node
        .history(&wallet, None, 10, false)
        .expect("history")
        .newer
        .expect("a newer cursor");

    // Made for other addresses, or not made by this node at all.
    let others = addresses(&b, &[(RECEIVING, 0)]);
    for (asked, cursor) in [
        (&others, head.as_str()),
        (&wallet, "newer:1:end"),
        (&wallet, ""),
    ] {
        assert!(
            matches!(
                node.history(asked, Some(cursor), 10, false),
                Err(NodeError::Cursor(_))
            ),
            "{cursor:?}"
        );
    }

    // Standing on a block the chain no longer has, as after a reorg.
    let mut parts: Vec<String> = head.split(':').map(str::to_owned).collect();
    parts[3] = hex::encode([0xee; 32]);
    assert!(matches!(
        node.history(&wallet, Some(&parts.join(":")), 10, false),
        Err(NodeError::StaleCursor { height: 1 })
    ));

    // Limits.
    assert!(matches!(
        node.history(&[], None, 10, false),
        Err(NodeError::Limit { .. })
    ));
    assert!(matches!(
        node.history(&wallet, None, 0, false),
        Err(NodeError::Limit { .. })
    ));
    assert!(matches!(
        node.history(&vec![wallet[0].clone(); 257], None, 10, false),
        Err(NodeError::Limit { .. })
    ));
}
