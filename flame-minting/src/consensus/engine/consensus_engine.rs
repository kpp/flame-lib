use crate::consensus::{
    BitcoinRollbackInput, BitcoinRollbackPlan, ConsensusOutcome, IncludedEndorsement,
    MintingOutcome,
};
use btc_integration::IndexedBlock;
use std::future::Future;

pub trait ConsensusEngine: Send + Sync + 'static {
    type Error: Send + 'static;

    fn get_rollback_plan(
        &self,
        input: BitcoinRollbackInput,
    ) -> impl Future<Output = Result<BitcoinRollbackPlan, Self::Error>> + Send;

    fn get_endorsements_outcome(
        &self,
        endorsements: Vec<IncludedEndorsement>,
    ) -> impl Future<Output = Result<ConsensusOutcome, Self::Error>> + Send;

    fn get_minting_outcome(
        &self,
        _block: IndexedBlock,
    ) -> impl Future<Output = Result<MintingOutcome, Self::Error>> + Send {
        async { unimplemented!() }
    }
}
