use crate::rewards::{PendingReward, RewardIndex};
use flamechain::CoreFlameHeight;
use std::future::Future;

pub trait RewardsStorage {
    type Error;

    fn get_pending_rewards(
        &self,
        _target_block_height: CoreFlameHeight,
    ) -> impl Future<Output = Result<Vec<PendingReward>, Self::Error>> + Send {
        async { unimplemented!() }
    }

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
