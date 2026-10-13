use super::{
    ConsensusStorage, IncludedAcquisition, IncludedEndorsement, MintingOutcome,
    MintingProtocolParams,
};
use btc_integration::IndexedBlock;
pub(super) use consensus_evaluator::ConsensusEvaluator;
use flame_storage::{CanonicalStorage, ChainStorage};
use std::sync::Arc;
use weighter::WeighterError;

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
        let endorsements: Vec<_> = self
            .new_block
            .endorsements
            .iter()
            .cloned()
            .map(|endorsement| IncludedEndorsement {
                btc_block: self.new_block.btc_block_tip,
                endorsement,
            })
            .collect();
        let consensus = ConsensusEvaluator {
            protocol_params: &self.protocol_params,
            consensus_storage: self.consensus_storage.as_ref(),
            chain_storage: self.chain_storage.as_ref(),
        }
        .evaluate(&endorsements, self.validate_acquisitions())
        .await?;
        Ok(MintingOutcome {
            consensus,
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
}

#[derive(Debug, PartialEq, Eq)]
pub enum MintingEngineError<C, H> {
    ConsensusStorage(C),
    ChainStorage(H),
    Weighter(WeighterError<C, H>),
}

pub mod acquisition_provider;
mod consensus_evaluator;
mod endorsement_validation;
mod endorsement_weight;
#[cfg(test)]
pub(crate) mod tests;
pub mod weighter;
