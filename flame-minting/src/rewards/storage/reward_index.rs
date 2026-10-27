use crate::rewards::PendingReward;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RewardIndex {
    pub flame_predicate: [u8; 32],
    pub access_predicate: [u8; 32],
}

impl From<&PendingReward> for RewardIndex {
    fn from(reward: &PendingReward) -> Self {
        Self {
            flame_predicate: reward.flame_predicate.to_point().to_bytes(),
            access_predicate: reward.access_predicate.to_point().to_bytes(),
        }
    }
}
