use flamevm::Predicate;

pub struct RewardWeight {
    pub flame_predicate: Predicate,
    pub access_predicate: Predicate,
    pub weight: f64,
}
