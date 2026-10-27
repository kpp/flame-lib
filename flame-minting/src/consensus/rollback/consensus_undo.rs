use crate::consensus::{
    AcquisitionIndex, DoubleSign, EndorsementIndex, IncludedAcquisition, IncludedEndorsement,
    WeightedBlockHeader, WeightedEndorsement,
};
use btc_integration::MinterP2wsh;
use flamechain::{BlockTip, CoreBlockTip, CoreFlameHeight};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsensusUndo {
    pub included_endorsements: Vec<IncludedEndorsement>,
    pub acquisitions: BTreeMap<AcquisitionIndex, Option<IncludedAcquisition>>,
    pub endorsements: BTreeMap<(MinterP2wsh, CoreFlameHeight), Option<WeightedEndorsement>>,
    pub pending_endorsements:
        BTreeMap<(CoreBlockTip, EndorsementIndex), Option<IncludedEndorsement>>,
    pub weighted_blocks: BTreeMap<BlockTip, Option<WeightedBlockHeader>>,
    pub double_signs: BTreeMap<(MinterP2wsh, CoreFlameHeight), Option<DoubleSign>>,
}

#[cfg(test)]
mod tests;
