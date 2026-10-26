use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use flame_storage::state::canonical::InMemoryCanonicalStorage;
use flamechain::{Block, BlockHash, Blockchain, ChainParams, CoreBlockHeader};

use super::*;

struct Chain {
    blocks: BTreeMap<BlockTip, Arc<Block>>,
}

impl ChainAccess for Chain {
    type Error = &'static str;

    async fn get_block(&self, tip: BlockTip) -> Result<Option<Arc<Block>>, Self::Error> {
        Ok(self.blocks.get(&tip).cloned())
    }
}

struct Fixture {
    chain: Chain,
    states: Vec<Blockchain>,
    path: ChainPath,
    core_headers: Vec<BlockHeader>,
}

fn fixture() -> Fixture {
    let ancestor = Blockchain::new(ChainParams::default()).unwrap();
    let mut old = ancestor.clone();
    let old_first = old.build_block([1; 32], Vec::new()).unwrap();
    old.connect(&old_first).unwrap();
    let old_intermediate = old.clone();
    let old_second = old.build_block([2; 32], Vec::new()).unwrap();
    old.connect(&old_second).unwrap();
    let mut new = ancestor.clone();
    let mut new_first = new.build_block([3; 32], Vec::new()).unwrap();
    new_first.header.core_block = Some(CoreBlockHeader {
        height: 1.into(),
        target_btc_height: 100,
    });
    new.connect(&new_first).unwrap();
    let new_intermediate = new.clone();
    let mut new_second = new.build_block([4; 32], Vec::new()).unwrap();
    new_second.header.core_block = Some(CoreBlockHeader {
        height: 2.into(),
        target_btc_height: 101,
    });
    new.connect(&new_second).unwrap();
    let path = ChainPath {
        detach: vec![old_second.header.block_tip(), old_first.header.block_tip()],
        attach: vec![new_first.header.block_tip(), new_second.header.block_tip()],
    };
    let core_headers = vec![new_first.header.clone(), new_second.header.clone()];
    let chain = Chain {
        blocks: [old_first, old_second, new_first, new_second]
            .into_iter()
            .map(|block| (block.header.block_tip(), Arc::new(block)))
            .collect(),
    };
    Fixture {
        chain,
        states: vec![old, old_intermediate, ancestor, new_intermediate, new],
        path,
        core_headers,
    }
}

fn tip(state: &Blockchain) -> BlockTip {
    BlockTip {
        hash: state.tip(),
        height: state.height().into(),
    }
}

struct Storage {
    inner: Mutex<StoredState>,
}

struct StoredState {
    state: Blockchain,
    committed_tips: Vec<BlockTip>,
    failure: Option<(usize, FailureTiming)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FailureTiming {
    BeforeCommit,
    AfterCommit,
}

impl Storage {
    fn new(state: &Blockchain, failure: Option<(usize, FailureTiming)>) -> Self {
        Self {
            inner: Mutex::new(StoredState {
                state: state.clone(),
                committed_tips: Vec::new(),
                failure,
            }),
        }
    }

    fn assert_state(&self, expected: &Blockchain) {
        let stored = self.inner.lock().unwrap();
        assert_eq!(tip(&stored.state), tip(expected));
        assert_eq!(stored.state.state_commitment(), expected.state_commitment());
    }

    fn committed_tips(&self) -> Vec<BlockTip> {
        self.inner.lock().unwrap().committed_tips.clone()
    }
}

impl CanonicalStorage for Storage {
    type Error = &'static str;

    async fn get_tip(&self) -> Result<Option<BlockTip>, Self::Error> {
        Ok(Some(tip(&self.inner.lock().unwrap().state)))
    }

    async fn get_state(&self) -> Result<Option<(BlockHash, Blockchain)>, Self::Error> {
        let stored = self.inner.lock().unwrap();
        Ok(Some((stored.state.tip(), stored.state.clone())))
    }

    async fn commit_state(&self, state: &Blockchain) -> Result<(), Self::Error> {
        let mut stored = self.inner.lock().unwrap();
        let failure = if stored
            .failure
            .is_some_and(|(step, _)| step == stored.committed_tips.len() + 1)
        {
            stored.failure.take().map(|(_, timing)| timing)
        } else {
            None
        };
        if failure == Some(FailureTiming::BeforeCommit) {
            return Err("commit interrupted");
        }
        stored.state = state.clone();
        stored.committed_tips.push(tip(state));
        if failure == Some(FailureTiming::AfterCommit) {
            return Err("commit interrupted");
        }
        Ok(())
    }
}

#[tokio::test]
async fn detaches_before_attaching_and_repeated_application_returns_all_core_headers() {
    let fixture = fixture();
    let storage = Storage::new(&fixture.states[0], None);
    let applier = ChainPathApplier {
        canonical_storage: &storage,
        chain: &fixture.chain,
    };
    for _ in 0..2 {
        let headers = applier
            .apply(
                tip(&fixture.states[0]),
                tip(&fixture.states[4]),
                &fixture.path,
            )
            .await
            .unwrap();
        assert_eq!(headers, fixture.core_headers);
        storage.assert_state(&fixture.states[4]);
        assert_eq!(
            storage.committed_tips(),
            fixture.states[1..].iter().map(tip).collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn resumes_after_failures_before_and_after_every_detach_and_attach_commit() {
    let fixture = fixture();
    for failed_step in 1..=4 {
        for timing in [FailureTiming::BeforeCommit, FailureTiming::AfterCommit] {
            let storage = Storage::new(&fixture.states[0], Some((failed_step, timing)));
            let applier = ChainPathApplier {
                canonical_storage: &storage,
                chain: &fixture.chain,
            };
            let result = applier
                .apply(
                    tip(&fixture.states[0]),
                    tip(&fixture.states[4]),
                    &fixture.path,
                )
                .await;
            assert!(matches!(
                result,
                Err(ChainPathApplierError::BlockDetacher(
                    BlockDetacherError::CanonicalStorage("commit interrupted")
                )) | Err(ChainPathApplierError::BlockAttacher(
                    BlockAttacherError::CanonicalStorage("commit interrupted")
                ))
            ));
            let completed = failed_step - usize::from(timing == FailureTiming::BeforeCommit);
            storage.assert_state(&fixture.states[completed]);
            let resumed = ChainPathApplier {
                canonical_storage: &storage,
                chain: &fixture.chain,
            };
            let headers = resumed
                .apply(
                    tip(&fixture.states[0]),
                    tip(&fixture.states[4]),
                    &fixture.path,
                )
                .await
                .unwrap();
            assert_eq!(headers, fixture.core_headers);
            storage.assert_state(&fixture.states[4]);
            assert_eq!(
                storage.committed_tips(),
                fixture.states[1..].iter().map(tip).collect::<Vec<_>>()
            );
        }
    }
}

#[tokio::test]
async fn resumes_detach_only_attach_only_and_empty_paths() {
    let fixture = fixture();
    for (start, end, path, expected_headers) in [
        (
            0,
            2,
            ChainPath {
                detach: fixture.path.detach.clone(),
                attach: Vec::new(),
            },
            Vec::new(),
        ),
        (
            2,
            4,
            ChainPath {
                detach: Vec::new(),
                attach: fixture.path.attach.clone(),
            },
            fixture.core_headers.clone(),
        ),
        (0, 0, ChainPath::default(), Vec::new()),
    ] {
        for completed in start..=end {
            let storage = Storage::new(&fixture.states[completed], None);
            let applier = ChainPathApplier {
                canonical_storage: &storage,
                chain: &fixture.chain,
            };
            let headers = applier
                .apply(
                    tip(&fixture.states[start]),
                    tip(&fixture.states[end]),
                    &path,
                )
                .await
                .unwrap();
            assert_eq!(headers, expected_headers);
            storage.assert_state(&fixture.states[end]);
            assert_eq!(
                storage.committed_tips(),
                fixture.states[completed + 1..end + 1]
                    .iter()
                    .map(tip)
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[tokio::test]
async fn rejects_a_tip_outside_the_path_without_writing_state() {
    let fixture = fixture();
    let mut other = fixture.states[2].clone();
    let block = other.build_block([9; 32], Vec::new()).unwrap();
    other.connect(&block).unwrap();
    let storage = Storage::new(&other, None);
    let applier = ChainPathApplier {
        canonical_storage: &storage,
        chain: &fixture.chain,
    };
    let result = applier
        .apply(
            tip(&fixture.states[0]),
            tip(&fixture.states[4]),
            &fixture.path,
        )
        .await;
    assert!(
        matches!(result, Err(ChainPathApplierError::UnexpectedCanonicalTip(actual)) if actual == tip(&other))
    );
    storage.assert_state(&other);
    assert!(storage.committed_tips().is_empty());
}

#[tokio::test]
async fn rejects_disconnected_paths_wrong_endpoints_and_repeated_tips_before_writes() {
    let fixture = fixture();
    let mut wrong_detach = fixture.path.clone();
    wrong_detach.detach.reverse();
    let mut wrong_attach = fixture.path.clone();
    wrong_attach.attach.reverse();
    let repeated = ChainPath {
        detach: fixture.path.detach.clone(),
        attach: fixture.path.detach.iter().rev().copied().collect(),
    };
    for (path, next) in [
        (wrong_detach, tip(&fixture.states[4])),
        (wrong_attach, tip(&fixture.states[4])),
        (fixture.path.clone(), tip(&fixture.states[0])),
        (repeated, tip(&fixture.states[0])),
        (ChainPath::default(), tip(&fixture.states[4])),
    ] {
        let storage = Storage::new(&fixture.states[0], None);
        let applier = ChainPathApplier {
            canonical_storage: &storage,
            chain: &fixture.chain,
        };
        assert!(matches!(
            applier.apply(tip(&fixture.states[0]), next, &path).await,
            Err(ChainPathApplierError::BlockTipMismatch { .. })
                | Err(ChainPathApplierError::ParentMismatch { .. })
                | Err(ChainPathApplierError::RepeatedTip(_))
        ));
        storage.assert_state(&fixture.states[0]);
        assert!(storage.committed_tips().is_empty());
    }
}

#[tokio::test]
async fn missing_or_mismatched_blocks_are_rejected_before_detaching() {
    for mismatched in [false, true] {
        let mut fixture = fixture();
        let requested = fixture.path.attach[1];
        if mismatched {
            fixture.chain.blocks.insert(
                requested,
                fixture.chain.blocks[&fixture.path.attach[0]].clone(),
            );
        } else {
            fixture.chain.blocks.remove(&requested);
        }
        let storage = Storage::new(&fixture.states[0], None);
        let applier = ChainPathApplier {
            canonical_storage: &storage,
            chain: &fixture.chain,
        };
        let result = applier
            .apply(
                tip(&fixture.states[0]),
                tip(&fixture.states[4]),
                &fixture.path,
            )
            .await;
        if mismatched {
            assert!(
                matches!(result, Err(ChainPathApplierError::BlockTipMismatch { expected, .. }) if expected == requested)
            );
        } else {
            assert!(
                matches!(result, Err(ChainPathApplierError::MissingBlock(actual)) if actual == requested)
            );
        }
        storage.assert_state(&fixture.states[0]);
        assert!(storage.committed_tips().is_empty());
    }
}

#[tokio::test]
async fn missing_canonical_state_is_reported() {
    let fixture = fixture();
    let storage = InMemoryCanonicalStorage::new();
    let applier = ChainPathApplier {
        canonical_storage: &storage,
        chain: &fixture.chain,
    };
    assert!(matches!(
        applier
            .apply(
                tip(&fixture.states[0]),
                tip(&fixture.states[4]),
                &fixture.path
            )
            .await,
        Err(ChainPathApplierError::MissingState)
    ));
}
