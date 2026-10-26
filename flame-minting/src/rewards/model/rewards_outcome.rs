use super::PendingReward;
use flamechain::CoreFlameHeight;

#[derive(Clone, Debug)]
pub struct RewardsOutcome {
    pub target_block_height: CoreFlameHeight,
    pub rewards: Vec<PendingReward>,
}
