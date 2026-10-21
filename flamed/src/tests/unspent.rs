//! `unspent`: what one address can spend, with and without the mempool.

use flamekd::util::{CHANGE, RECEIVING};
use flamekd::Network;
use flamepayments::{open_note, InputSpec};
use tempfile::TempDir;

use super::*;
use crate::node::{Node, NodeError};

/// An address as a wallet would paste it.
fn address<K: AccountKey>(account: &Account<K>, branch: u32, n: u32) -> String {
    account
        .address_at(branch, n)
        .expect("address")
        .to_bech32(Network::Testnet)
}

#[test]
fn unspent_follows_an_address_through_the_mempool_and_into_a_block() {
    let dir = TempDir::new().expect("temp dir");
    let a = account(&SEED_A);
    let b = account(&SEED_B);
    let mut rng = rng(70);
    let (genesis, cfg) = devnet(dir.path(), &a);
    let mut node = Node::open(&genesis, &cfg).expect("open");
    let genesis_id = genesis.contracts[0].id.0;

    // Genesis: A holds everything, with a proof that verifies.
    let a_address = address(&a, RECEIVING, 0);
    let held = node.unspent(&a_address, false).expect("unspent");
    assert_eq!(held.tip.height, 0);
    assert_eq!(held.predicate, genesis.contracts[0].predicate);
    assert_eq!(held.outputs.len(), 1);
    let entry = &held.outputs[0];
    assert_eq!(entry.id.0, genesis_id);
    assert_eq!(entry.height, Some(0));
    // Genesis anchors to no core block, so it has no time.
    assert_eq!(entry.time, None);
    let proof = crate::cells::proof_from_bytes(&entry.proof.as_ref().expect("a proof").0)
        .expect("a served proof decodes");
    assert!(node.verify_proof(&genesis_id, &proof));

    // The tracking address names the same predicate as the receiving one.
    let tracking = a
        .address_at(RECEIVING, 0)
        .expect("address")
        .to_tracking_address()
        .to_bech32(Network::Testnet);
    assert_eq!(
        node.unspent(&tracking, false).expect("unspent").outputs,
        held.outputs
    );

    // A pays B, and the payment waits in the mempool.
    let payment = 400 * FLAME;
    let input = InputSpec::clear(
        contract_of(&node, &genesis_id),
        live(&node, &genesis_id),
        a.spending_key_at(RECEIVING, 0).expect("key"),
    )
    .expect("clear input");
    let to_b = output(&b, RECEIVING, 0, payment);
    let to_change = output(&a, CHANGE, 0, GENESIS_SPARKS - payment - FEE);
    let (packaged, contracts) = signed_transfer(vec![input], &[to_b, to_change], FEE, &mut rng);
    let b_id = paying(&contracts, &b, RECEIVING, 0).id();
    let since = crate::indexer::unix_now();
    let txid = node.submit(&submitted(&packaged)).expect("A's payment");

    // Confirmed only: nothing has changed yet.
    assert_eq!(
        node.unspent(&a_address, false).expect("unspent").outputs,
        held.outputs
    );
    let b_address = address(&b, RECEIVING, 0);
    assert!(node
        .unspent(&b_address, false)
        .expect("unspent")
        .outputs
        .is_empty());

    // With the mempool: A's genesis contract is spent, and B's contract
    // exists without a height or a proof, with a note that opens.
    assert!(node
        .unspent(&a_address, true)
        .expect("unspent")
        .outputs
        .is_empty());
    let incoming = node.unspent(&b_address, true).expect("unspent");
    assert_eq!(incoming.outputs.len(), 1);
    let pending = &incoming.outputs[0];
    assert_eq!(pending.id.0, b_id);
    assert_eq!(pending.txid.0, txid.0);
    assert_eq!(pending.height, None);
    assert_eq!(pending.time, None);
    let received = pending.received_at.expect("an admission time");
    assert!((since..=crate::indexer::unix_now()).contains(&received));
    assert_eq!(pending.proof, None);
    let contract = crate::cells::contract_from_bytes(&pending.bytes.0).expect("decode");
    let note = open_note(
        &contract,
        pending.note.as_ref().map(|note| note.0.as_slice()),
        &b.address_at(RECEIVING, 0).expect("address"),
        &b.viewing_key_at(RECEIVING, 0).expect("viewing key"),
    )
    .expect("the pending note opens");
    assert_eq!(note.opening.qty, payment);

    // Once minted, B's contract is confirmed, with a proof at the new tip.
    node.mint_block().expect("block 1");
    for with_pending in [false, true] {
        let confirmed = node.unspent(&b_address, with_pending).expect("unspent");
        assert_eq!(confirmed.tip.height, 1);
        assert_eq!(confirmed.outputs.len(), 1);
        let entry = &confirmed.outputs[0];
        assert_eq!(entry.id.0, b_id);
        assert_eq!(entry.height, Some(1));
        assert_eq!(entry.time, node.block_time(1));
        assert!(entry.time.is_some());
        assert_eq!(entry.received_at, None);
        let proof = crate::cells::proof_from_bytes(&entry.proof.as_ref().expect("a proof").0)
            .expect("a served proof decodes");
        assert!(node.verify_proof(&b_id, &proof));
    }
    assert!(node
        .unspent(&a_address, false)
        .expect("unspent")
        .outputs
        .is_empty());
}

#[test]
fn unspent_refuses_an_address_it_cannot_read() {
    let dir = TempDir::new().expect("temp dir");
    let a = account(&SEED_A);
    let (genesis, cfg) = devnet(dir.path(), &a);
    let node = Node::open(&genesis, &cfg).expect("open");

    let mainnet = a
        .address_at(RECEIVING, 0)
        .expect("address")
        .to_bech32(Network::Mainnet);
    let mut corrupted = address(&a, RECEIVING, 0);
    corrupted.replace_range(10..11, if &corrupted[10..11] == "q" { "p" } else { "q" });

    assert!(matches!(
        node.unspent(&mainnet, false),
        Err(NodeError::WrongNetwork {
            address: Network::Mainnet,
            node: Network::Testnet,
        })
    ));
    for text in [corrupted.as_str(), "", "t1"] {
        assert!(
            matches!(node.unspent(text, false), Err(NodeError::Address(_))),
            "{text:?} is refused"
        );
    }
}
