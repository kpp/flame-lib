use super::{ConsensusStorage, IncludedAcquisition, MintingOutcome, MintingProtocolParams};
use acquisition_provider::AcquisitionProvider;
use btc_integration::IndexedBlock;
use endorsement_validation::{
    EndorsementValidationError, EndorsementValidationResult, EndorsementValidator,
};
use flame_storage::{CanonicalStorage, ChainStorage};
use std::collections::BTreeMap;
use std::sync::Arc;
use weighter::{Weighter, WeighterError};

pub struct MintingEngine<C: ConsensusStorage, H: ChainStorage, S: CanonicalStorage> {
    pub new_block: Arc<IndexedBlock>,
    pub protocol_params: Arc<MintingProtocolParams>,
    pub consensus_storage: Arc<C>,
    pub chain_storage: Arc<H>,
    pub canonical_storage: Arc<S>,
}

impl<C: ConsensusStorage, H: ChainStorage, S: CanonicalStorage> MintingEngine<C, H, S> {
    pub fn new(
        new_block: Arc<IndexedBlock>,
        protocol_params: Arc<MintingProtocolParams>,
        consensus_storage: Arc<C>,
        chain_storage: Arc<H>,
        canonical_storage: Arc<S>,
    ) -> Self {
        Self {
            new_block,
            protocol_params,
            consensus_storage,
            chain_storage,
            canonical_storage,
        }
    }

    pub async fn get_minting_outcome(
        &self,
    ) -> Result<MintingOutcome, MintingEngineError<C::Error, H::Error>> {
        let accepted_acquisitions = self.validate_acquisitions();
        let acquisition_provider = AcquisitionProvider {
            storage: self.consensus_storage.as_ref(),
            new_acquisitions: &accepted_acquisitions,
            params: &self.protocol_params,
        };
        let validated = self
            .validate_endorsements(&accepted_acquisitions)
            .await
            .map_err(|error| match error {
                EndorsementValidationError::ConsensusStorage(error) => {
                    MintingEngineError::ConsensusStorage(error)
                }
                EndorsementValidationError::ChainStorage(error) => {
                    MintingEngineError::ChainStorage(error)
                }
            })?;
        let weighted = Weighter {
            endorsements: &validated.valid_endorsements,
            removed_endorsements: &validated.removed_endorsements,
            acquisitions: &acquisition_provider,
            consensus_storage: self.consensus_storage.as_ref(),
            chain_storage: self.chain_storage.as_ref(),
            protocol_params: &self.protocol_params,
        }
        .weigh()
        .await
        .map_err(MintingEngineError::Weighter)?;
        let mut pending_endorsements = BTreeMap::new();
        for endorsement in validated.pending_endorsements {
            pending_endorsements
                .entry(endorsement.endorsement.block_tip())
                .or_insert_with(Vec::new)
                .push(endorsement);
        }
        Ok(MintingOutcome {
            accepted_endorsements: weighted.weighted_endorsements,
            removed_endorsements: validated.removed_endorsements,
            weighted_blocks: weighted.weighted_blocks,
            accepted_acquisitions,
            double_signs: validated.double_signs,
            pending_endorsements,
            next_btc_cursor: self.new_block.btc_block_tip,
        })
    }

    fn validate_acquisitions(&self) -> Vec<IncludedAcquisition> {
        self.new_block
            .acquisitions
            .iter()
            .filter(|acquisition| {
                let duration = acquisition
                    .data()
                    .duration
                    .unwrap_or(self.protocol_params.default_acquisition_duration.get());
                duration >= self.protocol_params.min_acquisition_duration.get()
            })
            .map(|acquisition| IncludedAcquisition {
                btc_block: self.new_block.btc_block_tip,
                acquisition: acquisition.clone(),
            })
            .collect()
    }

    async fn validate_endorsements(
        &self,
        new_acquisitions: &[IncludedAcquisition],
    ) -> Result<EndorsementValidationResult, EndorsementValidationError<C::Error, H::Error>> {
        EndorsementValidator {
            new_block: &self.new_block,
            protocol_params: &self.protocol_params,
            consensus_storage: self.consensus_storage.as_ref(),
            chain_storage: self.chain_storage.as_ref(),
            new_acquisitions,
        }
        .validate()
        .await
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum MintingEngineError<C, H> {
    ConsensusStorage(C),
    ChainStorage(H),
    Weighter(WeighterError<C, H>),
}

pub mod acquisition_provider;
mod endorsement_validation;
mod endorsement_weight;
#[cfg(test)]
pub(crate) mod tests;
pub mod weighter;
