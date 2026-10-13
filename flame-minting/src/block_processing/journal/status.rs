use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyStage {
    RemoveEndorsements,
    StoreEndorsements,
    StoreAcquisitions,
    StoreBlockWeights,
    StoreDoubleSigns,
    StorePendingEndorsements,
    PrepareFlameTransition,
    ApplyFlameTransition,
    PrepareRewards,
    ApplyRewards,
    StoreCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollbackStage {
    RestoreRewards,
    RestoreFlameChain,
    RestoreConsensus,
    RestoreCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalStatus {
    Applying(ApplyStage),
    Applied,
    RollingBack(RollbackStage),
    RolledBack,
}

impl JournalStatus {
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Applying(stage) => Some(match stage {
                ApplyStage::RemoveEndorsements => Self::Applying(ApplyStage::StoreEndorsements),
                ApplyStage::StoreEndorsements => Self::Applying(ApplyStage::StoreAcquisitions),
                ApplyStage::StoreAcquisitions => Self::Applying(ApplyStage::StoreBlockWeights),
                ApplyStage::StoreBlockWeights => Self::Applying(ApplyStage::StoreDoubleSigns),
                ApplyStage::StoreDoubleSigns => {
                    Self::Applying(ApplyStage::StorePendingEndorsements)
                }
                ApplyStage::StorePendingEndorsements => {
                    Self::Applying(ApplyStage::PrepareFlameTransition)
                }
                ApplyStage::PrepareFlameTransition => {
                    Self::Applying(ApplyStage::ApplyFlameTransition)
                }
                ApplyStage::ApplyFlameTransition => Self::Applying(ApplyStage::PrepareRewards),
                ApplyStage::PrepareRewards => Self::Applying(ApplyStage::ApplyRewards),
                ApplyStage::ApplyRewards => Self::Applying(ApplyStage::StoreCursor),
                ApplyStage::StoreCursor => Self::Applied,
            }),
            Self::Applied => Some(Self::RollingBack(RollbackStage::RestoreRewards)),
            Self::RollingBack(stage) => Some(match stage {
                RollbackStage::RestoreRewards => {
                    Self::RollingBack(RollbackStage::RestoreFlameChain)
                }
                RollbackStage::RestoreFlameChain => {
                    Self::RollingBack(RollbackStage::RestoreConsensus)
                }
                RollbackStage::RestoreConsensus => Self::RollingBack(RollbackStage::RestoreCursor),
                RollbackStage::RestoreCursor => Self::RolledBack,
            }),
            Self::RolledBack => None,
        }
    }

    pub fn transition_to(&mut self, next: Self) -> Result<(), JournalTransitionError> {
        if next != *self && self.next() != Some(next) {
            return Err(JournalTransitionError {
                current: *self,
                requested: next,
            });
        }
        *self = next;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalTransitionError {
    pub current: JournalStatus,
    pub requested: JournalStatus,
}

impl fmt::Display for JournalTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid journal transition from {:?} to {:?}",
            self.current, self.requested
        )
    }
}

impl std::error::Error for JournalTransitionError {}

#[cfg(test)]
mod tests;
