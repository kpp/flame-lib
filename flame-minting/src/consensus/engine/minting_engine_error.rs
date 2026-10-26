use super::weight::weighter::WeighterError;

#[derive(Debug, PartialEq, Eq)]
pub enum MintingEngineError<C, H> {
    ConsensusStorage(C),
    ChainStorage(H),
    Weighter(WeighterError<C, H>),
}
