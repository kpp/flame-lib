use super::{ConsensusStorage, MintingProtocolParams};
use crate::rewards::RewardsStorage;
use flame_storage::{CanonicalStorage, ChainStorage};

pub struct MintingManager<
    C: ConsensusStorage,
    R: RewardsStorage,
    H: ChainStorage,
    S: CanonicalStorage,
> {
    pub protocol_params: MintingProtocolParams,
    pub consensus_storage: C,
    pub rewards_storage: R,
    pub chain_storage: H,
    pub canonical_storage: S,
}
