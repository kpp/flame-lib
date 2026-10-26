#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlameApplyStage {
    Consensus,
    PrepareTransition,
    ApplyTransition,
    PrepareRewards,
    ApplyRewards,
    RemovePending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlameJournalStatus {
    Applying(FlameApplyStage),
    Applied,
}

impl FlameJournalStatus {
    pub fn next(self) -> Option<Self> {
        match self {
            Self::Applying(stage) => Some(match stage {
                FlameApplyStage::Consensus => Self::Applying(FlameApplyStage::PrepareTransition),
                FlameApplyStage::PrepareTransition => {
                    Self::Applying(FlameApplyStage::ApplyTransition)
                }
                FlameApplyStage::ApplyTransition => Self::Applying(FlameApplyStage::PrepareRewards),
                FlameApplyStage::PrepareRewards => Self::Applying(FlameApplyStage::ApplyRewards),
                FlameApplyStage::ApplyRewards => Self::Applying(FlameApplyStage::RemovePending),
                FlameApplyStage::RemovePending => Self::Applied,
            }),
            Self::Applied => None,
        }
    }
}
