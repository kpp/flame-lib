use flame_chain_service::ChainPath;
use flamechain::{BlockHeader, BlockTip};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlameTransition {
    pub previous_tip: BlockTip,
    pub next_tip: BlockTip,
    pub path: ChainPath,
    pub attached_core_headers: Vec<BlockHeader>,
}
