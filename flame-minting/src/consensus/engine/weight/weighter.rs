use super::endorsement_weight::weight_endorsement;
use crate::consensus::engine::acquisition_provider::AcquisitionProvider;
use crate::consensus::{
    ConsensusStorage, IncludedEndorsement, MintingProtocolParams, WeightedBlockHeader,
    WeightedEndorsement,
};
use flame_storage::ChainStorage;
use flamechain::{BlockTip, CoreBlockTip};
use std::collections::{HashMap, hash_map::Entry};

pub use super::endorsement_weight::EndorsementWeightError;

pub struct Weighter<'a, C, H> {
    pub endorsements: &'a [IncludedEndorsement],
    pub removed_endorsements: &'a [WeightedEndorsement],
    pub acquisitions: &'a AcquisitionProvider<'a, C>,
    pub consensus_storage: &'a C,
    pub chain_storage: &'a H,
    pub protocol_params: &'a MintingProtocolParams,
}

impl<C: ConsensusStorage, H: ChainStorage> Weighter<'_, C, H> {
    /// Returns updated headers with raw effective power and parent chain weight,
    /// including all known descendants affected by the endorsements.
    pub async fn weigh(&self) -> Result<WeighterResult, WeighterError<C::Error, H::Error>> {
        let endorsements_by_block =
            group_endorsement_changes(self.endorsements, self.removed_endorsements);
        let mut pending_blocks = HashMap::new();
        let mut weighted_endorsements = Vec::with_capacity(self.endorsements.len());

        for (core_tip, endorsements) in endorsements_by_block {
            let (header, descendants) = self
                .chain_storage
                .get_core_block_header_with_core_descendants(core_tip)
                .await
                .map_err(WeighterError::ChainStorage)?
                .ok_or(WeighterError::MissingEndorsedBlock(core_tip))?;
            let tip = header.block_tip();
            let core = header
                .core_block
                .as_ref()
                .ok_or(WeighterError::MissingEndorsedBlock(core_tip))?;
            let target = u64::from(core.target_btc_height);

            let pending = self.load_pending_block(&mut pending_blocks, tip).await?;
            let added_endorsements = self
                .weigh_added_endorsements(&endorsements.added, target)
                .await?;

            let change = calculate_block_power_change(
                pending.weighted.effective_power,
                endorsements
                    .removed
                    .iter()
                    .map(|endorsement| endorsement.effective_minting_power),
                added_endorsements
                    .iter()
                    .map(|endorsement| endorsement.effective_minting_power),
            )
            .map_err(|error| match error {
                WeightArithmeticError::Underflow => WeighterError::EffectivePowerUnderflow(tip),
                WeightArithmeticError::Overflow => WeighterError::EffectivePowerOverflow(tip),
            })?;

            pending.weighted.effective_power = change.new_effective_power;
            weighted_endorsements.extend(added_endorsements);

            for descendant in descendants {
                let pending = self
                    .load_pending_block(&mut pending_blocks, descendant.block_tip())
                    .await?;
                pending.parent_weight_delta += change.descendant_weight_delta;
            }
        }

        let weighted_blocks = pending_blocks
            .into_iter()
            .map(|(tip, pending)| {
                pending
                    .finish()
                    .map(|weighted| (tip, weighted))
                    .map_err(|error| match error {
                        WeightArithmeticError::Underflow => {
                            WeighterError::CumulativeWeightUnderflow(tip)
                        }
                        WeightArithmeticError::Overflow => {
                            WeighterError::CumulativeWeightOverflow(tip)
                        }
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(WeighterResult {
            weighted_blocks,
            weighted_endorsements,
        })
    }

    async fn weigh_added_endorsements(
        &self,
        endorsements: &[&IncludedEndorsement],
        target_height: u64,
    ) -> Result<Vec<WeightedEndorsement>, WeighterError<C::Error, H::Error>> {
        if endorsements.is_empty() {
            return Ok(Vec::new());
        }

        let acquisitions = self
            .acquisitions
            .active_at(target_height)
            .await
            .map_err(WeighterError::ConsensusStorage)?;
        endorsements
            .iter()
            .map(|&endorsement| {
                let minter = endorsement.endorsement.auth().minter().p2wsh();
                let active = acquisitions
                    .get(&minter)
                    .filter(|minter| !minter.is_double_signed)
                    .map(|minter| minter.acquisitions.as_slice())
                    .unwrap_or_default();
                weight_endorsement(
                    endorsement.clone(),
                    target_height,
                    active,
                    self.protocol_params,
                )
                .map_err(WeighterError::EndorsementWeight)
            })
            .collect()
    }

    async fn load_pending_block<'a>(
        &self,
        blocks: &'a mut HashMap<BlockTip, PendingBlockUpdate>,
        tip: BlockTip,
    ) -> Result<&'a mut PendingBlockUpdate, WeighterError<C::Error, H::Error>> {
        match blocks.entry(tip) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => {
                let weighted = self
                    .consensus_storage
                    .get_cumulative_weight(tip)
                    .await
                    .map_err(WeighterError::ConsensusStorage)?
                    .ok_or(WeighterError::MissingWeight(tip))?;
                Ok(entry.insert(PendingBlockUpdate {
                    weighted,
                    parent_weight_delta: 0,
                }))
            }
        }
    }
}

fn group_endorsement_changes<'a>(
    added_endorsements: &'a [IncludedEndorsement],
    removed_endorsements: &'a [WeightedEndorsement],
) -> HashMap<CoreBlockTip, BlockEndorsements<'a>> {
    let mut endorsements_by_block: HashMap<CoreBlockTip, BlockEndorsements<'a>> = HashMap::new();
    for endorsement in added_endorsements {
        endorsements_by_block
            .entry(endorsement.endorsement.block_tip())
            .or_default()
            .added
            .push(endorsement);
    }
    for endorsement in removed_endorsements {
        endorsements_by_block
            .entry(endorsement.original.endorsement.block_tip())
            .or_default()
            .removed
            .push(endorsement);
    }
    endorsements_by_block
}

fn block_weight(effective_power: u64) -> u32 {
    effective_power.checked_ilog2().unwrap_or(0)
}

fn calculate_block_power_change(
    old_power: u64,
    removed_powers: impl IntoIterator<Item = u64>,
    added_powers: impl IntoIterator<Item = u64>,
) -> Result<BlockWeightUpdate, WeightArithmeticError> {
    let mut effective_power = old_power;
    for power in removed_powers {
        effective_power = effective_power
            .checked_sub(power)
            .ok_or(WeightArithmeticError::Underflow)?;
    }
    for power in added_powers {
        effective_power = effective_power
            .checked_add(power)
            .ok_or(WeightArithmeticError::Overflow)?;
    }
    Ok(BlockWeightUpdate {
        new_effective_power: effective_power,
        descendant_weight_delta: i128::from(block_weight(effective_power))
            - i128::from(block_weight(old_power)),
    })
}

#[derive(Debug, PartialEq, Eq)]
struct BlockWeightUpdate {
    new_effective_power: u64,
    descendant_weight_delta: i128,
}

struct PendingBlockUpdate {
    weighted: WeightedBlockHeader,
    parent_weight_delta: i128,
}

impl PendingBlockUpdate {
    fn finish(mut self) -> Result<WeightedBlockHeader, WeightArithmeticError> {
        let parent_weight = i128::from(self.weighted.parent_weight) + self.parent_weight_delta;
        self.weighted.parent_weight = u64::try_from(parent_weight).map_err(|_| {
            if parent_weight < 0 {
                WeightArithmeticError::Underflow
            } else {
                WeightArithmeticError::Overflow
            }
        })?;
        Ok(self.weighted)
    }
}

#[derive(Debug, PartialEq, Eq)]
enum WeightArithmeticError {
    Underflow,
    Overflow,
}

#[derive(Default)]
struct BlockEndorsements<'a> {
    added: Vec<&'a IncludedEndorsement>,
    removed: Vec<&'a WeightedEndorsement>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct WeighterResult {
    pub weighted_blocks: HashMap<BlockTip, WeightedBlockHeader>,
    pub weighted_endorsements: Vec<WeightedEndorsement>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum WeighterError<C, H> {
    ConsensusStorage(C),
    ChainStorage(H),
    MissingEndorsedBlock(CoreBlockTip),
    MissingWeight(BlockTip),
    EffectivePowerOverflow(BlockTip),
    EffectivePowerUnderflow(BlockTip),
    CumulativeWeightOverflow(BlockTip),
    CumulativeWeightUnderflow(BlockTip),
    EndorsementWeight(EndorsementWeightError),
}
