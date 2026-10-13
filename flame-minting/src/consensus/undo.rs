use std::collections::BTreeMap;

use btc_integration::MinterP2wsh;
use flamechain::{BlockTip, CoreBlockTip, CoreFlameHeight};

use super::{
    AcquisitionIndex, DoubleSign, EndorsementIndex, IncludedAcquisition, IncludedEndorsement,
    WeightedBlockHeader, WeightedEndorsement,
};

mod manager;

pub use manager::ConsensusUndoManager;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConsensusUndo {
    pub acquisitions: BTreeMap<AcquisitionIndex, Option<IncludedAcquisition>>,
    pub endorsements: BTreeMap<(MinterP2wsh, CoreFlameHeight), Option<WeightedEndorsement>>,
    pub pending_endorsements:
        BTreeMap<(CoreBlockTip, EndorsementIndex), Option<IncludedEndorsement>>,
    pub weighted_blocks: BTreeMap<BlockTip, Option<WeightedBlockHeader>>,
    pub double_signs: BTreeMap<(MinterP2wsh, CoreFlameHeight), Option<DoubleSign>>,
}

#[cfg(test)]
mod tests;
