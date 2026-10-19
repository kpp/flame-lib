use std::collections::{BTreeMap, BTreeSet, HashMap};

use btc_integration::MinterP2wsh;
use flame_storage::ChainStorage;
use flamechain::{BlockTip, CoreBlockTip, CoreFlameHeight};

use super::engine::ConsensusEvaluator;
use super::{
    AcquisitionIndex, ConsensusStorage, DoubleSign, EndorsementIndex, IncludedAcquisition,
    IncludedEndorsement, MinterAcquisitions, MinterEndorsementState, MintingEngineError,
    MintingProtocolParams, WeightedBlockHeader, WeightedEndorsement,
};

#[derive(Clone, Debug, Default)]
pub struct BitcoinRollbackInput {
    pub included_endorsements: Vec<IncludedEndorsement>,
    pub remaining_endorsements: Vec<IncludedEndorsement>,
    pub minters: BTreeMap<MinterP2wsh, MinterEndorsementState>,
    pub acquisition_undo: BTreeMap<AcquisitionIndex, Option<IncludedAcquisition>>,
}

#[derive(Clone, Debug, Default)]
pub struct BitcoinRollbackPlan {
    pub included_endorsements: Vec<IncludedEndorsement>,
    pub acquisition_undo: BTreeMap<AcquisitionIndex, Option<IncludedAcquisition>>,
    pub endorsements: BTreeMap<(MinterP2wsh, CoreFlameHeight), Option<WeightedEndorsement>>,
    pub pending_endorsements:
        BTreeMap<(CoreBlockTip, EndorsementIndex), Option<IncludedEndorsement>>,
    pub double_signs: BTreeMap<(MinterP2wsh, CoreFlameHeight), Option<DoubleSign>>,
    pub weighted_blocks: HashMap<BlockTip, WeightedBlockHeader>,
}

pub(super) async fn calculate<C: ConsensusStorage + Sync, H: ChainStorage>(
    storage: &C,
    chain: &H,
    params: &MintingProtocolParams,
    input: BitcoinRollbackInput,
) -> Result<BitcoinRollbackPlan, MintingEngineError<C::Error, H::Error>> {
    let view = RollbackStorage {
        storage,
        input: &input,
    };
    let removed: Vec<_> = input
        .minters
        .values()
        .flat_map(|state| state.endorsements.iter())
        .cloned()
        .collect();
    let outcome = ConsensusEvaluator {
        protocol_params: params,
        consensus_storage: &view,
        chain_storage: chain,
    }
    .evaluate_with_removals(&input.remaining_endorsements, Vec::new(), &removed)
    .await?;
    let mut plan = BitcoinRollbackPlan {
        included_endorsements: input.included_endorsements.clone(),
        acquisition_undo: input.acquisition_undo.clone(),
        weighted_blocks: outcome.weighted_blocks,
        ..BitcoinRollbackPlan::default()
    };
    for (&minter, state) in &input.minters {
        for endorsement in &state.endorsements {
            plan.endorsements
                .insert((minter, endorsement.original.block_height()), None);
        }
        for endorsement in &state.pending_endorsements {
            plan.pending_endorsements.insert(
                (
                    endorsement.endorsement.block_tip(),
                    EndorsementIndex::from(&endorsement.endorsement),
                ),
                None,
            );
        }
        for sign in &state.double_signs {
            plan.double_signs
                .insert((minter, sign.target_flame_height), None);
        }
    }
    for endorsement in outcome.accepted_endorsements {
        plan.endorsements.insert(
            (
                endorsement.original.endorsement.auth().minter().p2wsh(),
                endorsement.original.block_height(),
            ),
            Some(endorsement),
        );
    }
    for (tip, endorsements) in outcome.pending_endorsements {
        for endorsement in endorsements {
            plan.pending_endorsements.insert(
                (tip, EndorsementIndex::from(&endorsement.endorsement)),
                Some(endorsement),
            );
        }
    }
    for endorsement in &input.remaining_endorsements {
        let tip = endorsement.endorsement.block_tip();
        if chain
            .get_core_block_header(tip)
            .await
            .map_err(MintingEngineError::ChainStorage)?
            .is_none()
        {
            plan.pending_endorsements.insert(
                (tip, EndorsementIndex::from(&endorsement.endorsement)),
                Some(endorsement.clone()),
            );
        }
    }
    for sign in outcome.double_signs {
        plan.double_signs
            .insert((sign.minter, sign.target_flame_height), Some(sign));
    }
    Ok(plan)
}

struct RollbackStorage<'a, C> {
    storage: &'a C,
    input: &'a BitcoinRollbackInput,
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
