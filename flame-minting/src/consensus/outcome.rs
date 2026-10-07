use std::collections::{BTreeMap, HashMap};

use btc_integration::{Acquisition, AuthenticatedMintingEndorsement, BtcBlockTip, MinterP2wsh};
use flamechain::{BlockHash, BlockTip, CoreBlockTip, CoreFlameHeight};

use super::{MintingProtocolParams, WeightedBlockHeader};

/// Outcome for 1 new BTC block
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintingOutcome {
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
    /// The processed cursor.
    pub next_btc_cursor: BtcBlockTip,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncludedEndorsement {
    pub btc_block: BtcBlockTip,
    pub endorsement: AuthenticatedMintingEndorsement,
}

impl IncludedEndorsement {
    pub fn block_height(&self) -> CoreFlameHeight {
        self.endorsement.output().data.block_height()
    }

    pub fn block_hash(&self) -> BlockHash {
        self.endorsement.output().data.block_hash()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeightedEndorsement {
    pub original: IncludedEndorsement,
    pub effective_minting_power: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncludedAcquisition {
    pub btc_block: BtcBlockTip,
    pub acquisition: Acquisition,
}

impl IncludedAcquisition {
    pub fn duration(&self, params: &MintingProtocolParams) -> u16 {
        self.acquisition
            .data()
            .duration
            .unwrap_or(params.default_acquisition_duration.get())
    }

    pub fn is_active_at(&self, target_btc_height: u64, params: &MintingProtocolParams) -> bool {
        // X + maturity <= target < X + maturity + duration, without overflowing.
        target_btc_height
            .checked_sub(self.btc_block.height)
            .and_then(|age| age.checked_sub(u64::from(params.acquisition_maturity)))
            .is_some_and(|active_age| active_age < u64::from(self.duration(params)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoubleSign {
    pub minter: MinterP2wsh,
    pub target_flame_height: CoreFlameHeight,
    pub endorsements: Vec<IncludedEndorsement>,
}

pub type PendingEndorsements = Vec<IncludedEndorsement>;
