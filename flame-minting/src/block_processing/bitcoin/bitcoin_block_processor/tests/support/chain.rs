use super::*;

impl ChainAccess for Chain {
    type Error = &'static str;

    async fn get_block(&self, tip: BlockTip) -> Result<Option<Arc<Block>>, Self::Error> {
        Ok(self.blocks.get(&tip).cloned())
    }

    async fn get_chain_path(&self, from: BlockTip, to: BlockTip) -> Result<ChainPath, Self::Error> {
        self.plans.fetch_add(1, Ordering::SeqCst);
        if self.reject_plans.load(Ordering::SeqCst) {
            return Err("path planning rejected");
        }
        if from == to {
            return Ok(ChainPath::default());
        }
        if from == self.transition.next_tip && to == self.transition.previous_tip {
            return Ok(ChainPath {
                detach: self.transition.path.attach.iter().rev().copied().collect(),
                attach: self.transition.path.detach.iter().rev().copied().collect(),
            });
        }
        assert_eq!(from, self.transition.previous_tip);
        assert_eq!(to, self.transition.next_tip);
        Ok(self.transition.path.clone())
    }
}

pub(in super::super) struct Chain {
    pub(in super::super) blocks: BTreeMap<BlockTip, Arc<Block>>,
    pub(in super::super) transition: FlameTransition,
    pub(in super::super) plans: AtomicUsize,
    pub(in super::super) reject_plans: AtomicBool,
}
