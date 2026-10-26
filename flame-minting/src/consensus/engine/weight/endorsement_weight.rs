use crate::consensus::{
    IncludedAcquisition, IncludedEndorsement, MintingProtocolParams, WeightedEndorsement,
};

pub fn weight_endorsement(
    endorsement: IncludedEndorsement,
    target_btc_height: u64,
    acquisitions: &[IncludedAcquisition],
    params: &MintingProtocolParams,
) -> Result<WeightedEndorsement, EndorsementWeightError> {
    let delay = endorsement
        .btc_block
        .height
        .checked_sub(target_btc_height)
        .ok_or(EndorsementWeightError::EndorsementBeforeTarget {
            inclusion_height: endorsement.btc_block.height,
            target_height: target_btc_height,
        })?;

    let effective_minting_power = if delay > u64::from(params.max_endorsement_delay) {
        0
    } else {
        let minter = endorsement.endorsement.auth().minter().p2wsh();
        let base_power = acquisitions
            .iter()
            .filter(|acquisition| {
                acquisition.acquisition.data().minter_p2wsh == minter
                    && acquisition.is_active_at(target_btc_height, params)
            })
            .try_fold(0u64, |total, acquisition| {
                let power = acquisition.acquisition.amount().to_sat()
                    / u64::from(acquisition.duration(params));
                total
                    .checked_add(power)
                    .ok_or(EndorsementWeightError::MintingPowerOverflow)
            })?;
        if delay >= u64::from(u64::BITS) {
            0
        } else {
            base_power >> delay
        }
    };

    Ok(WeightedEndorsement {
        original: endorsement,
        effective_minting_power,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndorsementWeightError {
    EndorsementBeforeTarget {
        inclusion_height: u64,
        target_height: u64,
    },
    MintingPowerOverflow,
}
