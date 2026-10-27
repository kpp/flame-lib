use super::rollback_storage::RollbackStorage;
use crate::consensus::engine::consensus_evaluator::ConsensusEvaluator;
use crate::consensus::{
    AcquisitionIndex, ConsensusStorage, DoubleSign, EndorsementIndex, IncludedAcquisition,
    IncludedEndorsement, MinterEndorsementState, MintingEngineError, MintingProtocolParams,
    WeightedBlockHeader, WeightedEndorsement,
};
use btc_integration::MinterP2wsh;
use flame_storage::ChainStorage;
use flamechain::{BlockTip, CoreBlockTip, CoreFlameHeight};
use std::collections::{BTreeMap, HashMap};

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

pub(crate) async fn calculate<C: ConsensusStorage + Sync, H: ChainStorage>(
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
