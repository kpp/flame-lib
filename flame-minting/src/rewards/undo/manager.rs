use std::collections::BTreeSet;

use crate::rewards::{RewardIndex, RewardsOutcome, RewardsStorage};

use super::RewardsUndo;

pub struct RewardsUndoManager<'a, S: RewardsStorage> {
    pub rewards_storage: &'a S,
}

impl<S: RewardsStorage> RewardsUndoManager<'_, S> {
    pub async fn prepare(&self, outcome: &RewardsOutcome) -> Result<RewardsUndo, S::Error> {
        let mut undo = RewardsUndo::default();
        let indices: BTreeSet<_> = outcome.rewards.iter().map(RewardIndex::from).collect();
        for index in indices {
            undo.rewards.insert(
                (outcome.target_block_height, index),
                self.rewards_storage
                    .get_pending_reward(outcome.target_block_height, index)
                    .await?,
            );
        }
        Ok(undo)
    }

    pub async fn restore(&self, undo: &RewardsUndo) -> Result<(), S::Error> {
        for (&(height, index), previous) in &undo.rewards {
            self.rewards_storage
                .restore_pending_reward(height, index, previous.as_ref())
                .await?;
        }
        Ok(())
    }
}
