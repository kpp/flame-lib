use std::collections::{BTreeMap, btree_map::Entry};

use btc_integration::{BtcBlockTip, MinterP2wsh};
use corepc_client::bitcoin::Txid;
use flamechain::{CoreBlockTip, CoreFlameHeight};

use crate::consensus::{IncludedEndorsement, WeightedEndorsement};

use super::{InMemoryConsensusStorageError, insert_unique};

#[derive(Default)]
pub(super) struct EndorsementStorage {
    endorsements: BTreeMap<(MinterP2wsh, CoreFlameHeight), WeightedEndorsement>,
    pub(super) pending_endorsements:
        BTreeMap<CoreBlockTip, BTreeMap<(Txid, usize), IncludedEndorsement>>,
}

impl EndorsementStorage {
    pub(super) fn store_endorsement(
        &mut self,
        btc_block: BtcBlockTip,
        endorsement: &WeightedEndorsement,
    ) -> Result<(), InMemoryConsensusStorageError> {
        if btc_block != endorsement.original.btc_block {
            return Err(InMemoryConsensusStorageError::BitcoinBlockMismatch);
        }
        let key = (
            endorsement.original.endorsement.auth().minter().p2wsh(),
            endorsement.original.block_height(),
        );
        insert_unique(&mut self.endorsements, key, endorsement.clone())
    }

    pub(super) fn get_endorsements_for_block(
        &self,
        tip: CoreBlockTip,
    ) -> Result<Vec<WeightedEndorsement>, InMemoryConsensusStorageError> {
        Ok(self
            .endorsements
            .values()
            .filter(|endorsement| endorsement.original.endorsement.block_tip() == tip)
            .cloned()
            .collect())
    }

    pub(super) fn get_minter_endorsement_for_height(
        &self,
        minter: &MinterP2wsh,
        flame_height: CoreFlameHeight,
    ) -> Result<Option<WeightedEndorsement>, InMemoryConsensusStorageError> {
        Ok(self.endorsements.get(&(*minter, flame_height)).cloned())
    }

    pub(super) fn remove_endorsement(
        &mut self,
        endorsement: &WeightedEndorsement,
    ) -> Result<(), InMemoryConsensusStorageError> {
        let key = (
            endorsement.original.endorsement.auth().minter().p2wsh(),
            endorsement.original.block_height(),
        );
        if let Entry::Occupied(entry) = self.endorsements.entry(key) {
            if entry.get().original != endorsement.original {
                return Err(InMemoryConsensusStorageError::ConflictingRecord);
            }
            entry.remove();
        }
        Ok(())
    }

    pub(super) fn store_pending_endorsement(
        &mut self,
        tip: CoreBlockTip,
        endorsement: &IncludedEndorsement,
    ) -> Result<(), InMemoryConsensusStorageError> {
        if tip != endorsement.endorsement.block_tip() {
            return Err(InMemoryConsensusStorageError::CoreBlockMismatch);
        }
        let key = (
            endorsement.endorsement.txid(),
            endorsement.endorsement.auth().input_index,
        );
        insert_unique(
            self.pending_endorsements.entry(tip).or_default(),
            key,
            endorsement.clone(),
        )
    }
}
