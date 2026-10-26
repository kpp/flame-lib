use super::ConsensusUndo;
use crate::consensus::{AcquisitionIndex, ConsensusOutcome, ConsensusStorage, EndorsementIndex};
use std::collections::BTreeSet;

pub struct ConsensusUndoManager<'a, C: ConsensusStorage> {
    pub consensus_storage: &'a C,
}

impl<C: ConsensusStorage> ConsensusUndoManager<'_, C> {
    pub async fn prepare(&self, outcome: &ConsensusOutcome) -> Result<ConsensusUndo, C::Error> {
        let mut undo = ConsensusUndo::default();
        let acquisitions: BTreeSet<_> = outcome
            .accepted_acquisitions
            .iter()
            .map(|included| AcquisitionIndex::from(&included.acquisition))
            .collect();
        for index in acquisitions {
            undo.acquisitions
                .insert(index, self.consensus_storage.get_acquisition(index).await?);
        }

        let endorsements: BTreeSet<_> = outcome
            .removed_endorsements
            .iter()
            .chain(&outcome.accepted_endorsements)
            .map(|weighted| {
                (
                    weighted.original.endorsement.auth().minter().p2wsh(),
                    weighted.original.block_height(),
                )
            })
            .collect();
        for (minter, height) in endorsements {
            undo.endorsements.insert(
                (minter, height),
                self.consensus_storage
                    .get_minter_endorsement_for_height(&minter, height)
                    .await?,
            );
        }

        let pending_endorsements: BTreeSet<_> = outcome
            .pending_endorsements
            .iter()
            .flat_map(|(&tip, endorsements)| {
                endorsements
                    .iter()
                    .map(move |included| (tip, EndorsementIndex::from(&included.endorsement)))
            })
            .chain(outcome.removed_pending_endorsements.iter().copied())
            .collect();
        for (tip, index) in pending_endorsements {
            undo.pending_endorsements.insert(
                (tip, index),
                self.consensus_storage
                    .get_pending_endorsement(tip, index)
                    .await?,
            );
        }

        let weighted_blocks: BTreeSet<_> = outcome.weighted_blocks.keys().copied().collect();
        for tip in weighted_blocks {
            undo.weighted_blocks.insert(
                tip,
                self.consensus_storage.get_cumulative_weight(tip).await?,
            );
        }

        let double_signs: BTreeSet<_> = outcome
            .double_signs
            .iter()
            .map(|sign| (sign.minter, sign.target_flame_height))
            .collect();
        for (minter, height) in double_signs {
            undo.double_signs.insert(
                (minter, height),
                self.consensus_storage
                    .get_double_sign(&minter, height)
                    .await?,
            );
        }
        Ok(undo)
    }

    pub async fn restore(&self, undo: &ConsensusUndo) -> Result<(), C::Error> {
        for (&(tip, index), previous) in &undo.pending_endorsements {
            self.consensus_storage
                .restore_pending_endorsement(tip, index, previous.as_ref())
                .await?;
        }
        for (&(minter, height), previous) in &undo.double_signs {
            self.consensus_storage
                .restore_double_sign(&minter, height, previous.as_ref())
                .await?;
        }
        for (&tip, previous) in &undo.weighted_blocks {
            self.consensus_storage
                .restore_cumulative_weight(tip, previous.as_ref())
                .await?;
        }
        for (&index, previous) in &undo.acquisitions {
            self.consensus_storage
                .restore_acquisition(index, previous.as_ref())
                .await?;
        }
        for (&(minter, height), previous) in &undo.endorsements {
            self.consensus_storage
                .restore_endorsement(&minter, height, previous.as_ref())
                .await?;
        }
        Ok(())
    }
}
