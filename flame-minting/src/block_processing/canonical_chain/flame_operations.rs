use super::chain_path_applier::ChainPathApplier;
use super::flame_transition::FlameTransition;
use crate::consensus::ConsensusStorage;
use flame_chain_service::ChainAccess;
use flame_storage::CanonicalStorage;

pub use super::chain_path_applier::ChainPathApplierError;

pub(in crate::block_processing) struct FlameOperations<
    'a,
    C: ConsensusStorage,
    H: ChainAccess,
    S: CanonicalStorage,
> {
    pub consensus_storage: &'a C,
    pub chain: &'a H,
    pub canonical_storage: &'a S,
}

impl<C: ConsensusStorage, H: ChainAccess, S: CanonicalStorage> FlameOperations<'_, C, H, S> {
    pub(in crate::block_processing) async fn prepare(
        &self,
    ) -> Result<FlameTransition, PrepareFlameError<C::Error, S::Error, H::Error>> {
        let previous_tip = self
            .canonical_storage
            .get_tip()
            .await
            .map_err(PrepareFlameError::Canonical)?
            .ok_or(PrepareFlameError::MissingCanonicalTip)?;
        let next_tip = self
            .consensus_storage
            .get_block_tip_with_most_weight()
            .await
            .map_err(PrepareFlameError::Consensus)?
            .ok_or(PrepareFlameError::MissingConsensusTip)?;
        let path = self
            .chain
            .get_chain_path(previous_tip, next_tip)
            .await
            .map_err(PrepareFlameError::Chain)?;
        let attached_core_headers = ChainPathApplier {
            canonical_storage: self.canonical_storage,
            chain: self.chain,
        }
        .validate_path(previous_tip, next_tip, &path)
        .await
        .map_err(PrepareFlameError::Path)?;
        Ok(FlameTransition {
            previous_tip,
            next_tip,
            path,
            attached_core_headers,
        })
    }

    pub(in crate::block_processing) async fn apply(
        &self,
        transition: &FlameTransition,
    ) -> Result<(), ChainPathApplierError<S::Error, H::Error>> {
        ChainPathApplier {
            canonical_storage: self.canonical_storage,
            chain: self.chain,
        }
        .apply(
            transition.previous_tip,
            transition.next_tip,
            &transition.path,
        )
        .await?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum PrepareFlameError<C, S, H> {
    Consensus(C),
    Canonical(S),
    Chain(H),
    Path(ChainPathApplierError<S, H>),
    MissingCanonicalTip,
    MissingConsensusTip,
}
