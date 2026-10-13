use std::{
    collections::BTreeSet,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use flamevm::{Predicate, Scalar};

use crate::rewards::{
    RewardsApplier, RewardsOutcome, RewardsStorage, storage::InMemoryRewardsStorage,
};

use super::*;

fn reward(flame: u64, access: u64, amount: u64) -> PendingReward {
    let point = Predicate::unspendable_key().decompress().unwrap();
    PendingReward {
        flame_predicate: Predicate::opaque((point * Scalar::from(flame).to_dalek()).compress()),
        access_predicate: Predicate::opaque((point * Scalar::from(access).to_dalek()).compress()),
        amount,
    }
}

fn contents(rewards: &[PendingReward]) -> BTreeMap<RewardIndex, u64> {
    rewards
        .iter()
        .map(|reward| (RewardIndex::from(reward), reward.amount))
        .collect()
}

#[tokio::test]
async fn undo_restores_previous_amounts_removes_new_rewards_and_preserves_untouched_records() {
    let storage = InMemoryRewardsStorage::new();
    let manager = RewardsUndoManager {
        rewards_storage: &storage,
    };
    let original = reward(1, 2, 10);
    let untouched = reward(3, 4, 30);
    let other_height = reward(1, 2, 40);
    storage
        .store_pending_reward(200.into(), &original)
        .await
        .unwrap();
    storage
        .store_pending_reward(200.into(), &untouched)
        .await
        .unwrap();
    storage
        .store_pending_reward(300.into(), &other_height)
        .await
        .unwrap();
    let outcome = RewardsOutcome {
        target_block_height: 200.into(),
        rewards: vec![original.clone(), reward(1, 3, 0)],
    };
    let undo = manager.prepare(&outcome).await.unwrap();
    assert_eq!(undo.rewards.len(), 2);
    assert!(undo.rewards[&(200.into(), RewardIndex::from(&outcome.rewards[1]))].is_none());
    let expected = contents(&[original, untouched]);
    assert_eq!(
        contents(&storage.get_pending_rewards(200.into()).await.unwrap()),
        expected
    );
    RewardsApplier {
        rewards_storage: storage.clone(),
    }
    .apply(&outcome)
    .await
    .unwrap();
    storage
        .restore_pending_reward(
            200.into(),
            RewardIndex::from(&outcome.rewards[0]),
            Some(&reward(1, 2, u64::MAX)),
        )
        .await
        .unwrap();
    for _ in 0..2 {
        manager.restore(&undo).await.unwrap();
        assert_eq!(
            contents(&storage.get_pending_rewards(200.into()).await.unwrap()),
            expected
        );
        assert_eq!(
            contents(&storage.get_pending_rewards(300.into()).await.unwrap()),
            contents(&[other_height.clone()])
        );
    }
}

struct RecordingStorage {
    reads: Mutex<BTreeSet<(CoreFlameHeight, RewardIndex)>>,
}

impl RewardsStorage for RecordingStorage {
    type Error = &'static str;

    async fn get_pending_reward(
        &self,
        height: CoreFlameHeight,
        index: RewardIndex,
    ) -> Result<Option<PendingReward>, Self::Error> {
        assert!(self.reads.lock().unwrap().insert((height, index)));
        Ok(None)
    }
}

#[tokio::test]
async fn preparation_reads_each_affected_key_once_without_writes() {
    let storage = RecordingStorage {
        reads: Mutex::new(BTreeSet::new()),
    };
    let outcome = RewardsOutcome {
        target_block_height: 200.into(),
        rewards: vec![reward(1, 2, 10), reward(1, 2, 20), reward(2, 1, 30)],
    };
    let undo = RewardsUndoManager {
        rewards_storage: &storage,
    }
    .prepare(&outcome)
    .await
    .unwrap();
    let expected = BTreeSet::from([
        (200.into(), RewardIndex::from(&outcome.rewards[0])),
        (200.into(), RewardIndex::from(&outcome.rewards[2])),
    ]);
    assert_eq!(*storage.reads.lock().unwrap(), expected);
    assert_eq!(
        undo.rewards.keys().copied().collect::<BTreeSet<_>>(),
        expected
    );
    assert!(undo.rewards.values().all(Option::is_none));
}

struct UnavailableStorage;

impl RewardsStorage for UnavailableStorage {
    type Error = &'static str;

    async fn get_pending_reward(
        &self,
        _: CoreFlameHeight,
        _: RewardIndex,
    ) -> Result<Option<PendingReward>, Self::Error> {
        Err("storage unavailable")
    }
}

#[tokio::test]
async fn preparation_propagates_read_failure_without_writes() {
    let outcome = RewardsOutcome {
        target_block_height: 200.into(),
        rewards: vec![reward(1, 2, 10)],
    };
    let manager = RewardsUndoManager {
        rewards_storage: &UnavailableStorage,
    };
    assert_eq!(
        manager.prepare(&outcome).await.unwrap_err(),
        "storage unavailable"
    );
}

#[tokio::test]
async fn empty_outcome_needs_no_storage_operations() {
    let manager = RewardsUndoManager {
        rewards_storage: &UnavailableStorage,
    };
    let outcome = RewardsOutcome {
        target_block_height: 200.into(),
        rewards: Vec::new(),
    };
    let undo = manager.prepare(&outcome).await.unwrap();
    assert!(undo.rewards.is_empty());
    manager.restore(&undo).await.unwrap();
}

struct InterruptedStorage {
    inner: InMemoryRewardsStorage,
    fail_once: AtomicBool,
}

impl RewardsStorage for InterruptedStorage {
    type Error = &'static str;

    async fn restore_pending_reward(
        &self,
        height: CoreFlameHeight,
        index: RewardIndex,
        previous: Option<&PendingReward>,
    ) -> Result<(), Self::Error> {
        self.inner
            .restore_pending_reward(height, index, previous)
            .await
            .map_err(|_| "restore failed")?;
        if self.fail_once.swap(false, Ordering::SeqCst) {
            return Err("interrupted after write");
        }
        Ok(())
    }
}

#[tokio::test]
async fn retry_finishes_restore_after_a_write_succeeded_but_reported_failure() {
    let storage = InterruptedStorage {
        inner: InMemoryRewardsStorage::new(),
        fail_once: AtomicBool::new(true),
    };
    let outcome = RewardsOutcome {
        target_block_height: 200.into(),
        rewards: vec![reward(1, 2, 10), reward(2, 1, 20)],
    };
    let undo = RewardsUndoManager {
        rewards_storage: &storage.inner,
    }
    .prepare(&outcome)
    .await
    .unwrap();
    RewardsApplier {
        rewards_storage: storage.inner.clone(),
    }
    .apply(&outcome)
    .await
    .unwrap();
    let manager = RewardsUndoManager {
        rewards_storage: &storage,
    };
    assert_eq!(manager.restore(&undo).await, Err("interrupted after write"));
    assert_eq!(
        storage
            .inner
            .get_pending_rewards(200.into())
            .await
            .unwrap()
            .len(),
        1
    );
    for _ in 0..2 {
        manager.restore(&undo).await.unwrap();
        assert!(
            storage
                .inner
                .get_pending_rewards(200.into())
                .await
                .unwrap()
                .is_empty()
        );
    }
}
