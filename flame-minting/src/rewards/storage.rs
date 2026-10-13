use std::future::Future;

use flamechain::CoreFlameHeight;

use super::PendingReward;

pub mod in_memory;
mod index;

pub use in_memory::{InMemoryRewardsStorage, InMemoryRewardsStorageError};
pub use index::RewardIndex;

pub trait RewardsStorage {
    type Error;

    fn get_pending_reward(
        &self,
        _target_block_height: CoreFlameHeight,
        _index: RewardIndex,
    ) -> impl Future<Output = Result<Option<PendingReward>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn restore_pending_reward(
        &self,
        _target_block_height: CoreFlameHeight,
        _index: RewardIndex,
        _previous: Option<&PendingReward>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn store_pending_reward(
        &self,
        _target_block_height: CoreFlameHeight,
        _reward: &PendingReward,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }
}
