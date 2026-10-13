use std::{
    collections::{BTreeMap, btree_map::Entry},
    fmt,
    sync::{Arc, RwLock},
};

use flamechain::CoreFlameHeight;

use super::{PendingReward, RewardIndex, RewardsStorage};

type RewardsByPredicate = BTreeMap<RewardIndex, PendingReward>;

#[derive(Clone, Default)]
pub struct InMemoryRewardsStorage {
    rewards: Arc<RwLock<BTreeMap<CoreFlameHeight, RewardsByPredicate>>>,
}

impl InMemoryRewardsStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_pending_rewards(
        &self,
        target_block_height: CoreFlameHeight,
    ) -> Result<Vec<PendingReward>, InMemoryRewardsStorageError> {
        let rewards = self
            .rewards
            .read()
            .map_err(|_| InMemoryRewardsStorageError::LockPoisoned)?;
        Ok(rewards
            .get(&target_block_height)
            .into_iter()
            .flat_map(|rewards| rewards.values())
            .cloned()
            .collect())
    }
}

impl RewardsStorage for InMemoryRewardsStorage {
    type Error = InMemoryRewardsStorageError;

    async fn get_pending_reward(
        &self,
        target_block_height: CoreFlameHeight,
        index: RewardIndex,
    ) -> Result<Option<PendingReward>, Self::Error> {
        let rewards = self.rewards.read().map_err(|_| Self::Error::LockPoisoned)?;
        Ok(rewards
            .get(&target_block_height)
            .and_then(|rewards| rewards.get(&index))
            .cloned())
    }

    async fn restore_pending_reward(
        &self,
        target_block_height: CoreFlameHeight,
        index: RewardIndex,
        previous: Option<&PendingReward>,
    ) -> Result<(), Self::Error> {
        if previous.is_some_and(|reward| RewardIndex::from(reward) != index) {
            return Err(Self::Error::RewardKeyMismatch);
        }
        let mut rewards = self
            .rewards
            .write()
            .map_err(|_| Self::Error::LockPoisoned)?;
        match previous {
            Some(previous) => {
                rewards
                    .entry(target_block_height)
                    .or_default()
                    .insert(index, previous.clone());
            }
            None => {
                if let Entry::Occupied(mut entry) = rewards.entry(target_block_height) {
                    entry.get_mut().remove(&index);
                    if entry.get().is_empty() {
                        entry.remove();
                    }
                }
            }
        }
        Ok(())
    }

    async fn store_pending_reward(
        &self,
        target_block_height: CoreFlameHeight,
        reward: &PendingReward,
    ) -> Result<(), Self::Error> {
        let key = RewardIndex::from(reward);
        let mut rewards = self
            .rewards
            .write()
            .map_err(|_| Self::Error::LockPoisoned)?;
        match rewards.entry(target_block_height).or_default().entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(reward.clone());
                Ok(())
            }
            Entry::Occupied(entry) if entry.get().amount == reward.amount => Ok(()),
            Entry::Occupied(entry) => Err(Self::Error::ConflictingReward {
                target_block_height,
                stored_amount: entry.get().amount,
                requested_amount: reward.amount,
            }),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InMemoryRewardsStorageError {
    LockPoisoned,
    RewardKeyMismatch,
    ConflictingReward {
        target_block_height: CoreFlameHeight,
        stored_amount: u64,
        requested_amount: u64,
    },
}

impl fmt::Display for InMemoryRewardsStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LockPoisoned => write!(formatter, "rewards storage lock poisoned"),
            Self::RewardKeyMismatch => write!(formatter, "reward does not match the requested key"),
            Self::ConflictingReward {
                target_block_height,
                stored_amount,
                requested_amount,
            } => write!(
                formatter,
                "conflicting reward at core height {target_block_height:?}: stored amount {stored_amount}, requested {requested_amount}"
            ),
        }
    }
}

impl std::error::Error for InMemoryRewardsStorageError {}

#[cfg(test)]
mod tests;
