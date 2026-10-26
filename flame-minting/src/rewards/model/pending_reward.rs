use flamevm::Predicate;

#[derive(Clone, Debug)]
pub struct PendingReward {
    pub flame_predicate: Predicate,
    pub access_predicate: Predicate,
    pub amount: u64,
}
