use flamechain::BlockTip;

#[derive(Debug, PartialEq, Eq)]
pub enum BlockWeightCalculatorError<C> {
    ConsensusStorage(C),
    NotCoreBlock(BlockTip),
    EndorsementBeforeTarget {
        inclusion_height: u64,
        target_height: u64,
    },
}
