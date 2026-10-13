use std::collections::HashMap;

use btc_integration::MinterIdentity;
use flame_storage::ChainStorage;
use flamechain::CoreFlameHeight;

use crate::consensus::{
    ConsensusStorage, DoubleSign, IncludedAcquisition, IncludedEndorsement, MintingProtocolParams,
    WeightedEndorsement,
};

pub struct EndorsementValidator<'a, C, H> {
    pub endorsements: &'a [IncludedEndorsement],
    pub protocol_params: &'a MintingProtocolParams,
    pub consensus_storage: &'a C,
    pub chain_storage: &'a H,
    pub new_acquisitions: &'a [IncludedAcquisition],
}

impl<C: ConsensusStorage, H: ChainStorage> EndorsementValidator<'_, C, H> {
    pub async fn validate(
        &self,
    ) -> Result<EndorsementValidationResult, EndorsementValidationError<C::Error, H::Error>> {
        let mut minters: HashMap<MinterIdentity, MinterEndorsements> = HashMap::new();

        for endorsement in self.endorsements {
            match self.check_eligibility(endorsement).await? {
                EndorsementEligibility::Rejected => {}
                EndorsementEligibility::Pending(endorsement) => {
                    minters
                        .entry(endorsement.endorsement.auth().minter().clone())
                        .or_default()
                        .pending
                        .push(endorsement);
                }
                EndorsementEligibility::Eligible(endorsement) => {
                    let minter = endorsement.endorsement.auth().minter();
                    let endorsements = minters.entry(minter.clone()).or_default();
                    let stored = if endorsements
                        .by_height
                        .contains_key(&endorsement.block_height())
                    {
                        None
                    } else {
                        self.consensus_storage
                            .get_minter_endorsement_for_height(
                                &minter.p2wsh(),
                                endorsement.block_height(),
                            )
                            .await
                            .map_err(EndorsementValidationError::ConsensusStorage)?
                    };
                    endorsements.record_endorsement(endorsement, stored);
                }
            }
        }

        let mut result = EndorsementValidationResult::default();
        for endorsements in minters.into_values() {
            endorsements.append_to(&mut result);
        }
        Ok(result)
    }

    async fn check_eligibility(
        &self,
        included: &IncludedEndorsement,
    ) -> Result<EndorsementEligibility, EndorsementValidationError<C::Error, H::Error>> {
        let endorsement = &included.endorsement;
        let minter = endorsement.auth().minter().p2wsh();
        let Some(core) = self
            .chain_storage
            .get_core_block_header(endorsement.block_tip())
            .await
            .map_err(EndorsementValidationError::ChainStorage)?
        else {
            if self
                .consensus_storage
                .is_minter_double_signed(&minter)
                .await
                .map_err(EndorsementValidationError::ConsensusStorage)?
            {
                return Ok(EndorsementEligibility::Rejected);
            }
            return Ok(EndorsementEligibility::Pending(included.clone()));
        };
        let target = u64::from(core.target_btc_height);
        if included.btc_block.height < target {
            return Ok(EndorsementEligibility::Rejected);
        }
        let active = self
            .consensus_storage
            .get_active_minter_acquisitions_at_height(&minter, target, self.protocol_params)
            .await
            .map_err(EndorsementValidationError::ConsensusStorage)?;
        if active.is_double_signed {
            return Ok(EndorsementEligibility::Rejected);
        }
        let has_new_active_acquisition = self.new_acquisitions.iter().any(|acquisition| {
            acquisition.acquisition.data().minter_p2wsh == minter
                && acquisition.is_active_at(target, self.protocol_params)
        });
        if active.acquisitions.is_empty() && !has_new_active_acquisition {
            return Ok(EndorsementEligibility::Rejected);
        }

        Ok(EndorsementEligibility::Eligible(included.clone()))
    }
}

enum EndorsementEligibility {
    Rejected,
    Pending(IncludedEndorsement),
    Eligible(IncludedEndorsement),
}

#[derive(Default)]
struct MinterEndorsements {
    by_height: HashMap<CoreFlameHeight, HeightEndorsements>,
    pending: Vec<IncludedEndorsement>,
    removed_endorsements: Vec<WeightedEndorsement>,
}

impl MinterEndorsements {
    fn record_endorsement(
        &mut self,
        endorsement: IncludedEndorsement,
        stored: Option<WeightedEndorsement>,
    ) {
        let height = endorsement.block_height();
        let previous = self.by_height.remove(&height).or_else(|| {
            stored.map(|endorsement| {
                HeightEndorsements::Single(EndorsementOrigin::Stored(endorsement))
            })
        });
        let state = match previous {
            Some(state) => state.record_endorsement(endorsement, &mut self.removed_endorsements),
            None => HeightEndorsements::Single(EndorsementOrigin::Current(endorsement)),
        };
        self.by_height.insert(height, state);
    }

    fn append_to(self, result: &mut EndorsementValidationResult) {
        let disqualified = self
            .by_height
            .values()
            .any(|state| matches!(state, HeightEndorsements::DoubleSigned(_)));

        for state in self.by_height.into_values() {
            match state {
                HeightEndorsements::Single(EndorsementOrigin::Current(endorsement))
                    if !disqualified =>
                {
                    result.valid_endorsements.push(endorsement);
                }
                HeightEndorsements::DoubleSigned(sign) => result.double_signs.push(sign),
                _ => {}
            }
        }
        if !disqualified {
            result.pending_endorsements.extend(self.pending);
        }
        result
            .removed_endorsements
            .extend(self.removed_endorsements);
    }
}

enum HeightEndorsements {
    Single(EndorsementOrigin),
    DoubleSigned(DoubleSign),
}

impl HeightEndorsements {
    fn record_endorsement(
        self,
        endorsement: IncludedEndorsement,
        removed_endorsements: &mut Vec<WeightedEndorsement>,
    ) -> Self {
        match self {
            Self::Single(previous)
                if previous.original().block_hash() == endorsement.block_hash() =>
            {
                Self::Single(previous)
            }
            Self::Single(previous) => {
                let first = match previous {
                    EndorsementOrigin::Current(first) => first,
                    EndorsementOrigin::Stored(stored) => {
                        let first = stored.original.clone();
                        removed_endorsements.push(stored);
                        first
                    }
                };
                Self::DoubleSigned(DoubleSign {
                    minter: endorsement.endorsement.auth().minter().p2wsh(),
                    target_flame_height: endorsement.block_height(),
                    endorsements: vec![first, endorsement],
                })
            }
            Self::DoubleSigned(mut sign) => {
                sign.endorsements.push(endorsement);
                Self::DoubleSigned(sign)
            }
        }
    }
}

enum EndorsementOrigin {
    Current(IncludedEndorsement),
    Stored(WeightedEndorsement),
}

impl EndorsementOrigin {
    fn original(&self) -> &IncludedEndorsement {
        match self {
            Self::Current(endorsement) => endorsement,
            Self::Stored(endorsement) => &endorsement.original,
        }
    }
}

#[derive(Default)]
pub(in crate::consensus) struct EndorsementValidationResult {
    pub valid_endorsements: Vec<IncludedEndorsement>,
    pub double_signs: Vec<DoubleSign>,
    pub removed_endorsements: Vec<WeightedEndorsement>,
    pub pending_endorsements: Vec<IncludedEndorsement>,
}

#[derive(Debug)]
pub(in crate::consensus) enum EndorsementValidationError<C, H> {
    ConsensusStorage(C),
    ChainStorage(H),
}
