//! Times: a block's from the core indexer, a transaction's from its
//! signer and from this node's mempool.

use std::fs;

use flamekd::util::{CHANGE, RECEIVING};
use tempfile::TempDir;

use super::*;
use crate::indexer::{core_block_hash, unix_now, CoreBlock, CoreError, CoreIndexer, MockIndexer};

/// A signing time well past `LOCKTIME_TIMESTAMP_THRESHOLD`.
const SIGNED_AT: u32 = 1_800_000_000;

#[test]
fn block_times_come_from_the_indexer_and_survive_a_restart() {
    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let since = unix_now();
    let mut node = Node::open(&genesis, &cfg).unwrap();
    node.mint_block().unwrap();
    node.mint_block().unwrap();

    // Genesis anchors to no core block.
    assert_eq!(node.block_time(0), None);
    let times: Vec<u64> = (1..=2).map(|h| node.block_time(h).unwrap()).collect();
    for time in &times {
        assert!((since..=unix_now()).contains(time));
    }
    assert_eq!(
        node.block_summary(1).unwrap().header.core_block_hash,
        core_block_hash(1)
    );

    // The indexer is what remembers them, in its own directory.
    let indexer = MockIndexer::open(&cfg.indexer_dir()).unwrap();
    assert_eq!(indexer.len(), 2);
    for (height, time) in (1..=2).zip(&times) {
        assert_eq!(
            indexer.time_of(&core_block_hash(height)).unwrap(),
            Some(*time)
        );
    }
    drop(indexer);

    drop(node);
    let node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.block_time(1), Some(times[0]));
    assert_eq!(node.block_time(2), Some(times[1]));

    // An indexer that lost its table costs the times, not the chain.
    drop(node);
    fs::remove_dir_all(cfg.indexer_dir()).unwrap();
    let mut node = Node::open(&genesis, &cfg).unwrap();
    assert_eq!(node.tip().height, 2);
    assert_eq!(node.block_time(1), None);
    assert_eq!(node.block_time(2), None);
    node.mint_block().unwrap();
    assert!(node.block_time(3).unwrap() >= since);
}

#[test]
fn a_transaction_carries_its_signing_time_and_admission_time() {
    let dir = TempDir::new().unwrap();
    let a = account(&SEED_A);
    let (genesis, cfg) = devnet(dir.path(), &a);
    let mut node = Node::open(&genesis, &cfg).unwrap();

    let id = genesis.contracts[0].id.0;
    let input = InputSpec::clear(
        contract_of(&node, &id),
        live(&node, &id),
        a.spending_key_at(RECEIVING, 0).unwrap(),
    )
    .unwrap();
    let header = TxHeader {
        version: 1,
        locktime: SIGNED_AT,
    };
    let to_change = output(&a, CHANGE, 0, GENESIS_SPARKS - FEE);
    let unsigned = build_transfer(
        std::slice::from_ref(&input),
        &[to_change],
        FEE,
        header,
        limits(),
        &mut rng(90),
    )
    .unwrap();
    let tx = block_tx(
        sign(unsigned, &[input.signing_key()]).unwrap(),
        limits(),
        vec![input.proof().clone()],
    );

    let since = unix_now();
    let txid = node.submit(&submitted(&tx)).unwrap();
    let pending = node.pending_transactions(1).unwrap().transactions;
    assert_eq!(pending[0].locktime, SIGNED_AT);
    let received_at = pending[0].received_at;
    assert!((since..=unix_now()).contains(&received_at));

    node.mint_block().unwrap();
    let detail = node.transaction(flamed_rpc::TxId(txid.0), false).unwrap();
    assert_eq!(detail.summary.locktime, SIGNED_AT);
    assert_eq!(detail.received_at, Some(received_at));
    assert_eq!(detail.block_time, node.block_time(1));
    assert!(detail.block_time.is_some());

    // A wallet's history reads the creating block's time beside each
    // output; the genesis allocation has none.
    let change = a.predicate_at(CHANGE, 0).unwrap().to_point().to_bytes();
    let hits = node.scan(&[change], 0).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].time, node.block_time(1));
    let allocation = genesis.contracts[0].predicate.0;
    assert_eq!(node.scan(&[allocation], 0).unwrap()[0].time, None);
}

/// An indexer with a clock of its own, standing in for the real one.
struct FixedIndexer;

impl CoreIndexer for FixedIndexer {
    fn anchor(&mut self, flame_height: u64) -> Result<CoreBlock, CoreError> {
        Ok(CoreBlock {
            hash: core_block_hash(flame_height),
            time: 1_000 + flame_height,
        })
    }

    fn time_of(&self, hash: &[u8; 32]) -> Result<Option<u64>, CoreError> {
        Ok((1..=16)
            .find(|height| core_block_hash(*height) == *hash)
            .map(|height| 1_000 + height))
    }
}

#[test]
fn any_indexer_drops_in() {
    let dir = TempDir::new().unwrap();
    let (genesis, cfg) = devnet(dir.path(), &account(&SEED_A));
    let mut node = Node::open_with(&genesis, &cfg, Box::new(FixedIndexer)).unwrap();
    node.mint_block().unwrap();
    node.mint_block().unwrap();
    assert_eq!(node.block_time(1), Some(1_001));
    assert_eq!(node.block_time(2), Some(1_002));

    drop(node);
    let node = Node::open_with(&genesis, &cfg, Box::new(FixedIndexer)).unwrap();
    assert_eq!(node.block_time(2), Some(1_002));
}

#[test]
fn the_mock_indexer_keeps_the_first_time_it_gave() {
    let dir = TempDir::new().unwrap();
    let mut indexer = MockIndexer::open(dir.path()).unwrap();
    let first = indexer.anchor(7).unwrap();
    assert_eq!(first.hash, core_block_hash(7));
    assert_eq!(indexer.anchor(7).unwrap(), first);
    assert_eq!(indexer.len(), 1);
    drop(indexer);

    let mut indexer = MockIndexer::open(dir.path()).unwrap();
    assert_eq!(indexer.anchor(7).unwrap(), first);
    assert_eq!(indexer.time_of(&core_block_hash(8)).unwrap(), None);
}

#[test]
fn the_mock_indexer_drops_a_torn_line_and_refuses_a_corrupt_one() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(MockIndexer::FILE);
    let line = format!("{} 1700000000\n", hex::encode(core_block_hash(1)));
    fs::write(&path, format!("{line}{}", &line[..20])).unwrap();

    let mut indexer = MockIndexer::open(dir.path()).unwrap();
    assert_eq!(indexer.len(), 1);
    indexer.anchor(2).unwrap();
    drop(indexer);
    let indexer = MockIndexer::open(dir.path()).unwrap();
    assert_eq!(indexer.len(), 2);
    assert_eq!(
        indexer.time_of(&core_block_hash(1)).unwrap(),
        Some(1_700_000_000)
    );
    drop(indexer);

    fs::write(&path, format!("{line}not a line\n")).unwrap();
    assert!(matches!(
        MockIndexer::open(dir.path()),
        Err(CoreError::Corrupt { line: 2, .. })
    ));
}
