use flamechain::{BlockTip, Blockchain, ChainParams, CoreBlockHeader};

use crate::consensus::{
    ConsensusStorage, WeightedBlockHeader,
    storage::{InMemoryConsensusStorage, InMemoryConsensusStorageError},
};

fn weighted_header(seed: u8, parent_weight: u64, effective_power: u64) -> WeightedBlockHeader {
    let mut chain = Blockchain::new(ChainParams::default()).unwrap();
    let mut header = chain.build_block([seed; 32], Vec::new()).unwrap().header;
    header.core_block = Some(CoreBlockHeader {
        height: 1.into(),
        target_btc_height: 100,
    });
    WeightedBlockHeader {
        header,
        parent_weight,
        effective_power,
    }
}

#[tokio::test]
async fn restores_previous_weights_and_best_tip() {
    let storage = InMemoryConsensusStorage::new();
    let original = weighted_header(1, 10, 4);
    let competing = weighted_header(2, 10, 8);
    let tip = original.block_tip();
    let competing_tip = competing.block_tip();
    for weighted in [&original, &competing] {
        storage
            .store_cumulative_weight(weighted.block_tip(), weighted)
            .await
            .unwrap();
    }
    let previous = storage.get_cumulative_weight(tip).await.unwrap();
    assert_eq!(
        storage.get_block_tip_with_most_weight().await.unwrap(),
        Some(competing_tip)
    );

    let mut updated = original.clone();
    updated.parent_weight = 20;
    updated.effective_power = 16;
    storage
        .store_cumulative_weight(tip, &updated)
        .await
        .unwrap();
    assert_eq!(
        storage.get_block_tip_with_most_weight().await.unwrap(),
        Some(tip)
    );

    for _ in 0..2 {
        storage
            .restore_cumulative_weight(tip, previous.as_ref())
            .await
            .unwrap();
        assert_eq!(
            storage.get_cumulative_weight(tip).await.unwrap(),
            Some(original.clone())
        );
        assert_eq!(
            storage.get_cumulative_weight(competing_tip).await.unwrap(),
            Some(competing.clone())
        );
        assert_eq!(
            storage.get_block_tip_with_most_weight().await.unwrap(),
            Some(competing_tip)
        );
    }
}

#[tokio::test]
async fn restores_absence_and_recreates_removed_weight() {
    let storage = InMemoryConsensusStorage::new();
    let strongest = weighted_header(1, 20, 16);
    let other = weighted_header(2, 10, 8);
    let tip = strongest.block_tip();
    let other_tip = other.block_tip();
    let previous = storage.get_cumulative_weight(tip).await.unwrap();
    assert_eq!(previous, None);
    for weighted in [&strongest, &other] {
        storage
            .store_cumulative_weight(weighted.block_tip(), weighted)
            .await
            .unwrap();
    }
    assert_eq!(
        storage.get_block_tip_with_most_weight().await.unwrap(),
        Some(tip)
    );

    for _ in 0..2 {
        storage
            .restore_cumulative_weight(tip, previous.as_ref())
            .await
            .unwrap();
        assert_eq!(storage.get_cumulative_weight(tip).await.unwrap(), None);
        assert_eq!(
            storage.get_cumulative_weight(other_tip).await.unwrap(),
            Some(other.clone())
        );
        assert_eq!(
            storage.get_block_tip_with_most_weight().await.unwrap(),
            Some(other_tip)
        );
    }
    for _ in 0..2 {
        storage
            .restore_cumulative_weight(tip, Some(&strongest))
            .await
            .unwrap();
        assert_eq!(
            storage.get_cumulative_weight(tip).await.unwrap(),
            Some(strongest.clone())
        );
        assert_eq!(
            storage.get_block_tip_with_most_weight().await.unwrap(),
            Some(tip)
        );
    }

    storage.restore_cumulative_weight(tip, None).await.unwrap();
    storage
        .restore_cumulative_weight(other_tip, None)
        .await
        .unwrap();
    assert_eq!(
        storage.get_block_tip_with_most_weight().await.unwrap(),
        None
    );
}

#[tokio::test]
async fn rejects_mismatched_hash_or_height_without_changing_weights() {
    let storage = InMemoryConsensusStorage::new();
    let original = weighted_header(1, 10, 4);
    let competing = weighted_header(2, 20, 8);
    let tip = original.block_tip();
    let competing_tip = competing.block_tip();
    for weighted in [&original, &competing] {
        storage
            .store_cumulative_weight(weighted.block_tip(), weighted)
            .await
            .unwrap();
    }

    for wrong_tip in [
        competing_tip,
        BlockTip {
            height: 2.into(),
            ..tip
        },
    ] {
        let before = storage.get_cumulative_weight(wrong_tip).await.unwrap();
        assert_eq!(
            storage
                .restore_cumulative_weight(wrong_tip, Some(&original))
                .await,
            Err(InMemoryConsensusStorageError::BlockTipMismatch)
        );
        assert_eq!(
            storage.get_cumulative_weight(wrong_tip).await.unwrap(),
            before
        );
        assert_eq!(
            storage.get_cumulative_weight(tip).await.unwrap(),
            Some(original.clone())
        );
        assert_eq!(
            storage.get_cumulative_weight(competing_tip).await.unwrap(),
            Some(competing.clone())
        );
        assert_eq!(
            storage.get_block_tip_with_most_weight().await.unwrap(),
            Some(competing_tip)
        );
    }
}

#[tokio::test]
async fn restoring_equal_weights_preserves_hash_tie_break() {
    let storage = InMemoryConsensusStorage::new();
    let first = weighted_header(1, 10, 4);
    let second = weighted_header(2, 10, 4);
    let (preferred, other) = if first.block_tip().hash < second.block_tip().hash {
        (first, second)
    } else {
        (second, first)
    };
    let tip = preferred.block_tip();
    let other_tip = other.block_tip();
    for weighted in [&preferred, &other] {
        storage
            .store_cumulative_weight(weighted.block_tip(), weighted)
            .await
            .unwrap();
    }
    assert_eq!(
        storage.get_block_tip_with_most_weight().await.unwrap(),
        Some(tip)
    );

    storage.restore_cumulative_weight(tip, None).await.unwrap();
    assert_eq!(
        storage.get_block_tip_with_most_weight().await.unwrap(),
        Some(other_tip)
    );
    for _ in 0..2 {
        storage
            .restore_cumulative_weight(tip, Some(&preferred))
            .await
            .unwrap();
        assert_eq!(
            storage.get_block_tip_with_most_weight().await.unwrap(),
            Some(tip)
        );
    }
}
