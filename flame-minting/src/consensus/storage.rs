use std::collections::HashMap;
use std::future::Future;

use crate::consensus::{
    DoubleSign, IncludedAcquisition, IncludedEndorsement, MintingProtocolParams,
    PendingEndorsements, WeightedBlockHeader, WeightedEndorsement,
};
use btc_integration::{BtcBlockTip, MinterP2wsh};
use flamechain::{BlockTip, CoreBlockTip, CoreFlameHeight};

pub mod in_memory_storage;
mod index;

pub use in_memory_storage::{InMemoryConsensusStorage, InMemoryConsensusStorageError};
pub use index::{AcquisitionIndex, EndorsementIndex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MinterAcquisitions {
    pub is_double_signed: bool,
    pub acquisitions: Vec<IncludedAcquisition>,
}

pub trait ConsensusStorage {
    type Error;

    fn get_block_tip_with_most_weight(
        &self,
    ) -> impl Future<Output = Result<Option<BlockTip>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_cumulative_weight(
        &self,
        _tip: BlockTip,
    ) -> impl Future<Output = Result<Option<WeightedBlockHeader>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn store_cumulative_weight(
        &self,
        _tip: BlockTip,
        _weighted_block: &WeightedBlockHeader,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn restore_cumulative_weight(
        &self,
        _tip: BlockTip,
        _previous: Option<&WeightedBlockHeader>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn store_acquisition(
        &self,
        _btc_block: BtcBlockTip,
        _acquisition: &IncludedAcquisition,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_acquisition(
        &self,
        _index: AcquisitionIndex,
    ) -> impl Future<Output = Result<Option<IncludedAcquisition>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn restore_acquisition(
        &self,
        _index: AcquisitionIndex,
        _previous: Option<&IncludedAcquisition>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    /// Returns acquisitions recorded through `btc_height`, including immature and expired ones.
    /// Acquisition activity is evaluated by consensus at each endorsement's target Bitcoin height.
    fn get_acquisitions_by_minters(
        &self,
        _btc_height: u64,
    ) -> impl Future<Output = Result<HashMap<MinterP2wsh, MinterAcquisitions>, Self::Error>> + Send
    {
        async { unimplemented!() }
    }

    /// Returns this minter's acquisitions active at the endorsement's target Bitcoin height.
    /// An unknown minter or one without active acquisitions has an empty list.
    /// Double-sign status is taken from the current storage snapshot, even when
    /// the requested height is in the past or the acquisition list is empty.
    fn get_active_minter_acquisitions_at_height(
        &self,
        _minter: &MinterP2wsh,
        _btc_height: u64,
        _params: &MintingProtocolParams,
    ) -> impl Future<Output = Result<MinterAcquisitions, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn store_endorsement(
        &self,
        _btc_block: BtcBlockTip,
        _endorsement: &WeightedEndorsement,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_endorsements_for_block(
        &self,
        _tip: CoreBlockTip,
    ) -> impl Future<Output = Result<Vec<WeightedEndorsement>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn remove_endorsement(
        &self,
        _endorsement: &WeightedEndorsement,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn restore_endorsement(
        &self,
        _minter: &MinterP2wsh,
        _flame_height: CoreFlameHeight,
        _previous: Option<&WeightedEndorsement>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn store_pending_endorsement(
        &self,
        _tip: CoreBlockTip,
        _endorsement: &IncludedEndorsement,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_pending_endorsement(
        &self,
        _tip: CoreBlockTip,
        _index: EndorsementIndex,
    ) -> impl Future<Output = Result<Option<IncludedEndorsement>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_pending_endorsements_for_block(
        &self,
        _tip: CoreBlockTip,
    ) -> impl Future<Output = Result<PendingEndorsements, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn restore_pending_endorsement(
        &self,
        _tip: CoreBlockTip,
        _index: EndorsementIndex,
        _previous: Option<&IncludedEndorsement>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_minter_endorsement_for_height(
        &self,
        _minter: &MinterP2wsh,
        _flame_height: CoreFlameHeight,
    ) -> impl Future<Output = Result<Option<WeightedEndorsement>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn add_double_sign(
        &self,
        _double_sign: &DoubleSign,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn get_double_sign(
        &self,
        _minter: &MinterP2wsh,
        _flame_height: CoreFlameHeight,
    ) -> impl Future<Output = Result<Option<DoubleSign>, Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn restore_double_sign(
        &self,
        _minter: &MinterP2wsh,
        _flame_height: CoreFlameHeight,
        _previous: Option<&DoubleSign>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send {
        async { unimplemented!() }
    }

    fn is_minter_double_signed(
        &self,
        _minter: &MinterP2wsh,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        async { unimplemented!() }
    }
}
