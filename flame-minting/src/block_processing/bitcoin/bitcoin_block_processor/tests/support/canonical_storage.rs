use super::*;

impl CanonicalStorage for Storage {
    type Error = &'static str;

    async fn get_tip(&self) -> Result<Option<BlockTip>, Self::Error> {
        Ok(Some(tip(&self.state.lock().unwrap().chain)))
    }

    async fn get_state(&self) -> Result<Option<(BlockHash, Blockchain)>, Self::Error> {
        let state = self.state.lock().unwrap();
        Ok(Some((state.chain.tip(), state.chain.clone())))
    }

    async fn commit_state(&self, chain: &Blockchain) -> Result<(), Self::Error> {
        let entry = self
            .journal
            .pending_entry()
            .await
            .unwrap()
            .unwrap()
            .operation
            .entry;
        assert!(matches!(
            entry.status,
            JournalStatus::Applying(ApplyStage::ApplyFlameTransition)
                | JournalStatus::RollingBack(RollbackStage::ApplyFlameTransition)
        ));
        let expected = if matches!(entry.status, JournalStatus::RollingBack(_)) {
            FlameTransition {
                previous_tip: self.transition.next_tip,
                next_tip: self.transition.previous_tip,
                path: ChainPath {
                    detach: self.transition.path.attach.iter().rev().copied().collect(),
                    attach: self.transition.path.detach.iter().rev().copied().collect(),
                },
                attached_core_headers: Vec::new(),
            }
        } else {
            self.transition.clone()
        };
        assert_eq!(entry.flame_transition.as_ref(), Some(&expected));
        let mut state = self.state.lock().unwrap();
        let failure = if state
            .failure
            .is_some_and(|(step, _)| step == state.commits.len() + 1)
        {
            state.failure.take().map(|(_, after)| after)
        } else {
            None
        };
        if failure == Some(false) {
            return Err("commit interrupted");
        }
        state.chain = chain.clone();
        state.commits.push(tip(chain));
        if failure == Some(true) {
            return Err("commit interrupted");
        }
        Ok(())
    }
}

pub(in super::super) struct Storage {
    pub(in super::super) state: Mutex<StoredState>,
    pub(in super::super) journal: InMemoryBitcoinJournal,
    pub(in super::super) transition: FlameTransition,
}

pub(in super::super) struct StoredState {
    pub(in super::super) chain: Blockchain,
    pub(in super::super) commits: Vec<BlockTip>,
    pub(in super::super) failure: Option<(usize, bool)>,
}
