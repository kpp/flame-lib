use super::{RewardsOutcome, RewardsStorage};

pub struct RewardsApplier<S: RewardsStorage> {
    pub rewards_storage: S,
}

impl<S: RewardsStorage> RewardsApplier<S> {
    pub async fn apply(&self, outcome: &RewardsOutcome) -> Result<(), S::Error> {
        apply_outcome(&self.rewards_storage, outcome).await
    }
}

pub(crate) async fn apply_outcome<S: RewardsStorage>(
    storage: &S,
    outcome: &RewardsOutcome,
) -> Result<(), S::Error> {
    for reward in &outcome.rewards {
        storage
            .store_pending_reward(outcome.target_block_height, reward)
            .await?;
    }
    Ok(())
}
