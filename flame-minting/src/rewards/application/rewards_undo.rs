use crate::rewards::{PendingReward, RewardIndex};
use flamechain::CoreFlameHeight;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct RewardsUndo {
    pub rewards: BTreeMap<(CoreFlameHeight, RewardIndex), Option<PendingReward>>,
}

#[cfg(test)]
mod tests;
