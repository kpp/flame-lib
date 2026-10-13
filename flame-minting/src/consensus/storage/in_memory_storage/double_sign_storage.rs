use std::collections::{BTreeMap, btree_map::Entry};

use btc_integration::MinterP2wsh;
use flamechain::CoreFlameHeight;

use crate::consensus::DoubleSign;

use super::InMemoryConsensusStorageError;

#[derive(Default)]
pub(super) struct DoubleSignStorage {
    signs: BTreeMap<MinterP2wsh, BTreeMap<CoreFlameHeight, DoubleSign>>,
}

impl DoubleSignStorage {
    pub(super) fn add_double_sign(
        &mut self,
        double_sign: &DoubleSign,
    ) -> Result<(), InMemoryConsensusStorageError> {
        validate_double_sign(double_sign)?;
        let stored = self
            .signs
            .entry(double_sign.minter)
            .or_default()
            .entry(double_sign.target_flame_height)
            .or_insert_with(|| DoubleSign {
                minter: double_sign.minter,
                target_flame_height: double_sign.target_flame_height,
                endorsements: Vec::new(),
            });
        for endorsement in &double_sign.endorsements {
            if !stored.endorsements.contains(endorsement) {
                stored.endorsements.push(endorsement.clone());
            }
        }
        Ok(())
    }

    pub(super) fn get_double_sign(
        &self,
        minter: &MinterP2wsh,
        flame_height: CoreFlameHeight,
    ) -> Option<DoubleSign> {
        self.signs.get(minter)?.get(&flame_height).cloned()
    }

    pub(super) fn is_minter_double_signed(&self, minter: &MinterP2wsh) -> bool {
        self.signs.contains_key(minter)
    }

    pub(super) fn restore_double_sign(
        &mut self,
        minter: &MinterP2wsh,
        flame_height: CoreFlameHeight,
        previous: Option<&DoubleSign>,
    ) -> Result<(), InMemoryConsensusStorageError> {
        match previous {
            Some(previous) => {
                if previous.minter != *minter || previous.target_flame_height != flame_height {
                    return Err(InMemoryConsensusStorageError::DoubleSignKeyMismatch);
                }
                validate_double_sign(previous)?;
                self.signs
                    .entry(*minter)
                    .or_default()
                    .insert(flame_height, previous.clone());
            }
            None => {
                if let Entry::Occupied(mut entry) = self.signs.entry(*minter) {
                    entry.get_mut().remove(&flame_height);
                    if entry.get().is_empty() {
                        entry.remove();
                    }
                }
            }
        }
        Ok(())
    }
}

fn validate_double_sign(double_sign: &DoubleSign) -> Result<(), InMemoryConsensusStorageError> {
    let Some(first) = double_sign.endorsements.first() else {
        return Err(InMemoryConsensusStorageError::InvalidDoubleSign);
    };
    let matching_endorsements = double_sign.endorsements.iter().all(|endorsement| {
        endorsement.endorsement.auth().minter().p2wsh() == double_sign.minter
            && endorsement.block_height() == double_sign.target_flame_height
    });
    let conflicting_hashes = double_sign
        .endorsements
        .iter()
        .any(|endorsement| endorsement.block_hash() != first.block_hash());
    if !matching_endorsements || !conflicting_hashes {
        return Err(InMemoryConsensusStorageError::InvalidDoubleSign);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
