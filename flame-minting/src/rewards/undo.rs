use std::collections::BTreeMap;

use flamechain::CoreFlameHeight;

use super::{PendingReward, RewardIndex};

mod manager;

pub use manager::RewardsUndoManager;

#[derive(Clone, Debug, Default)]
pub struct RewardsUndo {
    pub rewards: BTreeMap<(CoreFlameHeight, RewardIndex), Option<PendingReward>>,
}

#[cfg(test)]
mod tests;
