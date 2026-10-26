use super::{BlockAttacher, BlockAttacherError, BlockDetacher, BlockDetacherError};
use flame_chain_service::{ChainAccess, ChainPath};
use flame_storage::CanonicalStorage;
use flamechain::{BlockHeader, BlockTip};
use std::collections::BTreeSet;

pub struct ChainPathApplier<'a, S: CanonicalStorage, H: ChainAccess> {
    pub canonical_storage: &'a S,
    pub chain: &'a H,
}

struct ValidatedPath {
    tips: Vec<BlockTip>,
    attached_core_headers: Vec<BlockHeader>,
}

impl<S: CanonicalStorage, H: ChainAccess> ChainPathApplier<'_, S, H> {
    pub(super) async fn validate_path(
        &self,
        previous_tip: BlockTip,
        next_tip: BlockTip,
        path: &ChainPath,
    ) -> Result<Vec<BlockHeader>, ChainPathApplierError<S::Error, H::Error>> {
        Ok(self
            .validate(previous_tip, next_tip, path)
            .await?
            .attached_core_headers)
    }

    pub async fn apply(
        &self,
        previous_tip: BlockTip,
        next_tip: BlockTip,
        path: &ChainPath,
    ) -> Result<Vec<BlockHeader>, ChainPathApplierError<S::Error, H::Error>> {
        let validated = self.validate(previous_tip, next_tip, path).await?;
        let current = self
            .canonical_storage
            .get_tip()
            .await
            .map_err(ChainPathApplierError::CanonicalStorage)?
            .ok_or(ChainPathApplierError::MissingState)?;
        let completed = validated
            .tips
            .iter()
            .position(|tip| *tip == current)
            .ok_or(ChainPathApplierError::UnexpectedCanonicalTip(current))?;

        let detacher = BlockDetacher {
            canonical_storage: self.canonical_storage,
        };
        for &tip in path.detach.iter().skip(completed) {
            detacher
                .detach_block(tip)
                .await
                .map_err(ChainPathApplierError::BlockDetacher)?;
        }
        let attacher = BlockAttacher {
            canonical_storage: self.canonical_storage,
            chain: self.chain,
        };
        for &tip in path
            .attach
            .iter()
            .skip(completed.saturating_sub(path.detach.len()))
        {
            attacher
                .attach_block(tip)
                .await
                .map_err(ChainPathApplierError::BlockAttacher)?;
        }
        Ok(validated.attached_core_headers)
    }

    async fn validate(
        &self,
        previous_tip: BlockTip,
        next_tip: BlockTip,
        path: &ChainPath,
    ) -> Result<ValidatedPath, ChainPathApplierError<S::Error, H::Error>> {
        let mut current = previous_tip;
        let mut tips = vec![current];
        for &tip in &path.detach {
            check_tip(current, tip)?;
            let header = self.header(tip).await?;
            current = parent_tip(&header)?;
            tips.push(current);
        }
        let mut attached_core_headers = Vec::new();
        for &tip in &path.attach {
            let header = self.header(tip).await?;
            let parent = parent_tip(&header)?;
            if parent != current {
                return Err(ChainPathApplierError::ParentMismatch {
                    block: tip,
                    expected: current,
                    actual: parent,
                });
            }
            current = tip;
            tips.push(current);
            if header.core_block.is_some() {
                attached_core_headers.push(header);
            }
        }
        check_tip(next_tip, current)?;
        let mut unique = BTreeSet::new();
        for &tip in &tips {
            if !unique.insert(tip) {
                return Err(ChainPathApplierError::RepeatedTip(tip));
            }
        }
        Ok(ValidatedPath {
            tips,
            attached_core_headers,
        })
    }

    async fn header(
        &self,
        tip: BlockTip,
    ) -> Result<BlockHeader, ChainPathApplierError<S::Error, H::Error>> {
        let block = self
            .chain
            .get_block(tip)
            .await
            .map_err(ChainPathApplierError::ChainAccess)?
            .ok_or(ChainPathApplierError::MissingBlock(tip))?;
        check_tip(tip, block.header.block_tip())?;
        Ok(block.header.clone())
    }
}

fn parent_tip<S, H>(header: &BlockHeader) -> Result<BlockTip, ChainPathApplierError<S, H>> {
    let height = header
        .height
        .checked_sub(1)
        .ok_or(ChainPathApplierError::GenesisInPath)?;
    Ok(BlockTip {
        hash: header.parent,
        height: height.into(),
    })
}

fn check_tip<S, H>(
    expected: BlockTip,
    actual: BlockTip,
) -> Result<(), ChainPathApplierError<S, H>> {
    if expected != actual {
        return Err(ChainPathApplierError::BlockTipMismatch { expected, actual });
    }
    Ok(())
}

#[derive(Debug)]
pub enum ChainPathApplierError<S, H> {
    CanonicalStorage(S),
    ChainAccess(H),
    BlockAttacher(BlockAttacherError<S, H>),
    BlockDetacher(BlockDetacherError<S>),
    MissingState,
    MissingBlock(BlockTip),
    BlockTipMismatch {
        expected: BlockTip,
        actual: BlockTip,
    },
    ParentMismatch {
        block: BlockTip,
        expected: BlockTip,
        actual: BlockTip,
    },
    GenesisInPath,
    RepeatedTip(BlockTip),
    UnexpectedCanonicalTip(BlockTip),
}

#[cfg(test)]
mod tests;
