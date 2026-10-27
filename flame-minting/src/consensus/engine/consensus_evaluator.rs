use super::{
    acquisition_provider::AcquisitionProvider,
    endorsement_validator::{EndorsementValidationError, EndorsementValidator},
    weight::weighter::Weighter,
};
use crate::consensus::{
    ConsensusOutcome, ConsensusStorage, EndorsementIndex, IncludedAcquisition, IncludedEndorsement,
    MintingEngineError, MintingProtocolParams,
};
use flame_storage::ChainStorage;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::consensus) struct ConsensusEvaluator<'a, C, H> {
    pub protocol_params: &'a MintingProtocolParams,
    pub consensus_storage: &'a C,
    pub chain_storage: &'a H,
}

impl<C: ConsensusStorage, H: ChainStorage> ConsensusEvaluator<'_, C, H> {
    pub(in crate::consensus) async fn evaluate(
        &self,
        endorsements: &[IncludedEndorsement],
        accepted_acquisitions: Vec<IncludedAcquisition>,
    ) -> Result<ConsensusOutcome, MintingEngineError<C::Error, H::Error>> {
        self.evaluate_with_removals(endorsements, accepted_acquisitions, &[])
            .await
    }

    pub(in crate::consensus) async fn evaluate_with_removals(
        &self,
        endorsements: &[IncludedEndorsement],
        accepted_acquisitions: Vec<IncludedAcquisition>,
        removed: &[crate::consensus::WeightedEndorsement],
    ) -> Result<ConsensusOutcome, MintingEngineError<C::Error, H::Error>> {
        let mut validated = EndorsementValidator {
            endorsements,
            protocol_params: self.protocol_params,
            consensus_storage: self.consensus_storage,
            chain_storage: self.chain_storage,
            new_acquisitions: &accepted_acquisitions,
        }
        .validate()
        .await
        .map_err(|error| match error {
            EndorsementValidationError::ConsensusStorage(error) => {
                MintingEngineError::ConsensusStorage(error)
            }
            EndorsementValidationError::ChainStorage(error) => {
                MintingEngineError::ChainStorage(error)
            }
        })?;
        validated.removed_endorsements.extend_from_slice(removed);
        let acquisition_provider = AcquisitionProvider {
            storage: self.consensus_storage,
            new_acquisitions: &accepted_acquisitions,
            params: self.protocol_params,
        };
        let weighted = Weighter {
            endorsements: &validated.valid_endorsements,
            removed_endorsements: &validated.removed_endorsements,
            acquisitions: &acquisition_provider,
            consensus_storage: self.consensus_storage,
            chain_storage: self.chain_storage,
            protocol_params: self.protocol_params,
        }
        .weigh()
        .await
        .map_err(MintingEngineError::Weighter)?;
        let mut removed_pending_endorsements: BTreeSet<_> = endorsements
            .iter()
            .map(|included| {
                (
                    included.endorsement.block_tip(),
                    EndorsementIndex::from(&included.endorsement),
                )
            })
            .collect();
        let mut pending_endorsements = BTreeMap::new();
        for endorsement in validated.pending_endorsements {
            removed_pending_endorsements.remove(&(
                endorsement.endorsement.block_tip(),
                EndorsementIndex::from(&endorsement.endorsement),
            ));
            pending_endorsements
                .entry(endorsement.endorsement.block_tip())
                .or_insert_with(Vec::new)
                .push(endorsement);
        }
        Ok(ConsensusOutcome {
            accepted_endorsements: weighted.weighted_endorsements,
            removed_endorsements: validated.removed_endorsements,
            weighted_blocks: weighted.weighted_blocks,
            accepted_acquisitions,
            double_signs: validated.double_signs,
            pending_endorsements,
            removed_pending_endorsements,
        })
    }
}
