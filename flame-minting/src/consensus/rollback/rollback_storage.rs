use super::bitcoin_rollback::BitcoinRollbackInput;
use crate::consensus::{
    AcquisitionIndex, ConsensusStorage, MinterAcquisitions, MintingProtocolParams,
    WeightedBlockHeader, WeightedEndorsement,
};
use btc_integration::MinterP2wsh;
use flamechain::{BlockTip, CoreFlameHeight};
use std::collections::{BTreeSet, HashMap};

pub(super) struct RollbackStorage<'a, C> {
    pub(super) storage: &'a C,
    pub(super) input: &'a BitcoinRollbackInput,
}

impl<C: ConsensusStorage + Sync> ConsensusStorage for RollbackStorage<'_, C> {
    type Error = C::Error;

    async fn is_minter_double_signed(&self, minter: &MinterP2wsh) -> Result<bool, Self::Error> {
        if self.input.minters.contains_key(minter) {
            return Ok(false);
        }
        self.storage.is_minter_double_signed(minter).await
    }

    async fn get_minter_endorsement_for_height(
        &self,
        minter: &MinterP2wsh,
        height: CoreFlameHeight,
    ) -> Result<Option<WeightedEndorsement>, Self::Error> {
        if self.input.minters.contains_key(minter) {
            return Ok(None);
        }
        self.storage
            .get_minter_endorsement_for_height(minter, height)
            .await
    }

    async fn get_cumulative_weight(
        &self,
        tip: BlockTip,
    ) -> Result<Option<WeightedBlockHeader>, Self::Error> {
        self.storage.get_cumulative_weight(tip).await
    }

    async fn get_acquisitions_by_minters(
        &self,
        height: u64,
    ) -> Result<HashMap<MinterP2wsh, MinterAcquisitions>, Self::Error> {
        let mut minters = self.storage.get_acquisitions_by_minters(height).await?;
        let replaced: BTreeSet<_> = self.input.acquisition_undo.keys().copied().collect();
        for (&minter, state) in &mut minters {
            state
                .acquisitions
                .retain(|entry| !replaced.contains(&AcquisitionIndex::from(&entry.acquisition)));
            if self.input.minters.contains_key(&minter) {
                state.is_double_signed = false;
            }
        }
        for acquisition in self
            .input
            .acquisition_undo
            .values()
            .flatten()
            .filter(|entry| entry.btc_block.height <= height)
        {
            let minter = acquisition.acquisition.data().minter_p2wsh;
            let state = minters.entry(minter).or_insert(MinterAcquisitions {
                is_double_signed: false,
                acquisitions: Vec::new(),
            });
            state.acquisitions.push(acquisition.clone());
        }
        Ok(minters)
    }

    async fn get_active_minter_acquisitions_at_height(
        &self,
        minter: &MinterP2wsh,
        height: u64,
        params: &MintingProtocolParams,
    ) -> Result<MinterAcquisitions, Self::Error> {
        let mut state = self
            .get_acquisitions_by_minters(height)
            .await?
            .remove(minter)
            .unwrap_or(MinterAcquisitions {
                is_double_signed: false,
                acquisitions: Vec::new(),
            });
        state
            .acquisitions
            .retain(|entry| entry.is_active_at(height, params));
        Ok(state)
    }
}
