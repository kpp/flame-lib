use super::ConsensusOutcome;
use btc_integration::BtcBlockTip;

/// Outcome for 1 new BTC block
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MintingOutcome {
    pub consensus: ConsensusOutcome,
    pub next_btc_cursor: BtcBlockTip,
}
