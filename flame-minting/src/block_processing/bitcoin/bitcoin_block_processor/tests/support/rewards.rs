use super::*;

impl RewardsStorage for Rewards {
    type Error = &'static str;
    async fn get_pending_rewards(
        &self,
        height: CoreFlameHeight,
    ) -> Result<Vec<PendingReward>, Self::Error> {
        self.inner
            .get_pending_rewards(height)
            .await
            .map_err(|_| "reward read failed")
    }
    async fn get_pending_reward(
        &self,
        height: CoreFlameHeight,
        index: RewardIndex,
    ) -> Result<Option<PendingReward>, Self::Error> {
        self.inner
            .get_pending_reward(height, index)
            .await
            .map_err(|_| "reward read failed")
    }
    async fn store_pending_reward(
        &self,
        height: CoreFlameHeight,
        reward: &PendingReward,
    ) -> Result<(), Self::Error> {
        let entry = self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .unwrap()
            .operation
            .entry;
        assert_eq!(
            entry.status,
            JournalStatus::Applying(ApplyStage::ApplyRewards)
        );
        assert!(entry.rewards_changes.is_some());
        self.inner
            .store_pending_reward(height, reward)
            .await
            .map_err(|_| "reward write failed")?;
        if self.fail_after_write.swap(false, Ordering::SeqCst) {
            return Err("reward interrupted");
        }
        Ok(())
    }
    async fn restore_pending_reward(
        &self,
        height: CoreFlameHeight,
        index: RewardIndex,
        previous: Option<&PendingReward>,
    ) -> Result<(), Self::Error> {
        self.inner
            .restore_pending_reward(height, index, previous)
            .await
            .map_err(|_| "reward restore failed")?;
        if self.fail_after_write.swap(false, Ordering::SeqCst) {
            return Err("reward interrupted");
        }
        Ok(())
    }
}

pub(in super::super) struct Rewards {
    pub(in super::super) inner: InMemoryRewardsStorage,
    pub(in super::super) journal: InMemoryBitcoinJournal,
    pub(in super::super) fail_after_write: AtomicBool,
}
