use std::collections::{BTreeMap, BTreeSet, HashMap};

use flamechain::{BlockTip, CoreBlockTip};

use crate::consensus::{
    ConsensusStorage, DoubleSign, EndorsementIndex, IncludedAcquisition, PendingEndorsements,
    WeightedBlockHeader, WeightedEndorsement,
};

pub(super) struct ConsensusOperations<'a, C: ConsensusStorage> {
    pub storage: &'a C,
}

impl<C: ConsensusStorage> ConsensusOperations<'_, C> {
    pub(super) async fn remove_endorsements(
        &self,
        endorsements: &[WeightedEndorsement],
    ) -> Result<(), C::Error> {
        for endorsement in endorsements {
            self.storage.remove_endorsement(endorsement).await?;
        }

        Ok(())
    }

    pub(super) async fn store_endorsements(
        &self,
        endorsements: &[WeightedEndorsement],
    ) -> Result<(), C::Error> {
        for endorsement in endorsements {
            self.storage
                .store_endorsement(endorsement.original.btc_block, endorsement)
                .await?;
        }

        Ok(())
    }

    pub(super) async fn store_acquisitions(
        &self,
        acquisitions: &[IncludedAcquisition],
    ) -> Result<(), C::Error> {
        for acquisition in acquisitions {
            self.storage
                .store_acquisition(acquisition.btc_block, acquisition)
                .await?;
        }

        Ok(())
    }

    pub(super) async fn store_block_weights(
        &self,
        weighted_blocks: &HashMap<BlockTip, WeightedBlockHeader>,
    ) -> Result<(), C::Error> {
        for (&tip, weighted_block) in weighted_blocks {
            self.storage
                .store_cumulative_weight(tip, weighted_block)
                .await?;
        }

        Ok(())
    }

    pub(super) async fn store_double_signs(
        &self,
        double_signs: &[DoubleSign],
    ) -> Result<(), C::Error> {
        for double_sign in double_signs {
            self.storage.add_double_sign(double_sign).await?;
        }

        Ok(())
    }

    pub(super) async fn remove_pending_endorsements(
        &self,
        endorsements: &BTreeSet<(CoreBlockTip, EndorsementIndex)>,
    ) -> Result<(), C::Error> {
        for &(tip, index) in endorsements {
            self.storage
                .restore_pending_endorsement(tip, index, None)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn store_pending_endorsements(
        &self,
        pending_endorsements: &BTreeMap<CoreBlockTip, PendingEndorsements>,
    ) -> Result<(), C::Error> {
        for (&tip, endorsements) in pending_endorsements {
            for endorsement in endorsements {
                self.storage
                    .store_pending_endorsement(tip, endorsement)
                    .await?;
            }
        }

        Ok(())
    }
}
