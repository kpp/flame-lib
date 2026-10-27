use super::{
    DoubleSign, EndorsementIndex, IncludedAcquisition, PendingEndorsements, WeightedBlockHeader,
    WeightedEndorsement,
};
use flamechain::{BlockTip, CoreBlockTip};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsensusOutcome {
    /// New valid endorsements.
    pub accepted_endorsements: Vec<WeightedEndorsement>,
    /// Previously counted endorsements invalidated by double signing.
    pub removed_endorsements: Vec<WeightedEndorsement>,
    /// Updated weight state for endorsed blocks and their affected descendants.
    pub weighted_blocks: HashMap<BlockTip, WeightedBlockHeader>,
    /// New valid acquisitions.
    pub accepted_acquisitions: Vec<IncludedAcquisition>,
    /// Double endorsements.
    pub double_signs: Vec<DoubleSign>,
    /// Endorsements for the blocks that node doesn't yet have.
    pub pending_endorsements: BTreeMap<CoreBlockTip, PendingEndorsements>,
    pub removed_pending_endorsements: BTreeSet<(CoreBlockTip, EndorsementIndex)>,
}
