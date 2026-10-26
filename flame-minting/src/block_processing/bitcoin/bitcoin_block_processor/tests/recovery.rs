use super::*;
use flamevm::Predicate;

struct ArrivalProbe(Arc<tokio::sync::Notify>);

impl crate::FlameBlockArrivalHandler for ArrivalProbe {
    type Error = std::convert::Infallible;
    async fn on_flame_block_arrival(&self, _: BlockTip) -> Result<(), Self::Error> {
        self.0.notify_one();
        Ok(())
    }
}

#[tokio::test]
async fn resumes_before_and_after_every_apply_stage_marker_without_recomputing_undo() {
    for stage in apply_stages() {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            *p.journal.fault.lock().unwrap() = Some((Point::Apply(stage), after));
            assert!(matches!(
                p.apply(fixture.block()).await,
                Err(BlockProcessingError::Journal("journal interrupted"))
            ));
            let saved = fixture.entry().await;
            p.consensus_storage
                .reject_reads
                .store(true, Ordering::SeqCst);
            if saved.flame_transition.is_some() {
                fixture.no_replanning().await;
            }
            assert_eq!(p.apply(fixture.block()).await.unwrap(), btc_tip(101));
            fixture.assert_applied().await;
            assert_eq!(p.engine.calls.load(Ordering::SeqCst), 1);
            assert_eq!(p.canonical_storage.state.lock().unwrap().commits.len(), 4);
        }
    }
}

#[tokio::test]
async fn retries_plan_persistence_without_overwriting_saved_plans() {
    for point in [Point::Begin, Point::FlamePlan, Point::RewardsPlan] {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            *p.journal.fault.lock().unwrap() = Some((point, after));
            assert!(matches!(
                p.apply(fixture.block()).await,
                Err(BlockProcessingError::Journal("journal interrupted"))
            ));
            if let Some(entry) = p.journal.pending_entry().await.unwrap() {
                p.consensus_storage
                    .reject_reads
                    .store(true, Ordering::SeqCst);
                if entry.operation.entry.flame_transition.is_some() {
                    fixture.no_replanning().await;
                }
            }
            p.apply(fixture.block()).await.unwrap();
            fixture.assert_applied().await;
            assert_eq!(
                p.engine.calls.load(Ordering::SeqCst),
                if point == Point::Begin && !after {
                    2
                } else {
                    1
                }
            );
        }
    }
}

#[tokio::test]
async fn retries_a_partially_written_consensus_stage_using_the_original_undo() {
    for write in 1..=8 {
        let fixture = Fixture::new().await;
        let p = &fixture.processor;
        *p.consensus_storage.fail_after_write.lock().unwrap() = Some(write);
        assert!(matches!(
            p.apply(fixture.block()).await,
            Err(BlockProcessingError::Consensus(
                "interrupted after storage write"
            ))
        ));
        p.consensus_storage
            .reject_reads
            .store(true, Ordering::SeqCst);
        p.resume().await.unwrap();
        fixture.assert_applied().await;
        fixture.allow_planning();
        p.rollback(btc_tip(101)).await.unwrap();
        fixture.assert_restored().await;
    }
}

#[tokio::test]
async fn resumes_every_flame_commit_during_application_and_rollback() {
    for rollback in [false, true] {
        for step in 1..=4 {
            for after in [false, true] {
                let fixture = Fixture::new().await;
                let p = &fixture.processor;
                if rollback {
                    p.apply(fixture.block()).await.unwrap();
                }
                p.canonical_storage.state.lock().unwrap().failure =
                    Some((if rollback { 4 + step } else { step }, after));
                let result = if rollback {
                    p.rollback(btc_tip(101)).await
                } else {
                    p.apply(fixture.block()).await
                };
                assert!(matches!(result, Err(BlockProcessingError::Flame(_))));
                fixture.no_replanning().await;
                p.resume().await.unwrap();
                if rollback {
                    fixture.assert_restored().await;
                } else {
                    fixture.assert_applied().await;
                }
                assert_eq!(
                    p.canonical_storage.state.lock().unwrap().commits.len(),
                    if rollback { 8 } else { 4 }
                );
                assert_eq!(
                    p.chain.plans.load(Ordering::SeqCst),
                    if rollback { 2 } else { 1 }
                );
            }
        }
    }
}

#[tokio::test]
async fn resumes_every_rollback_marker_and_the_begin_record() {
    for point in [Point::BeginRollback, Point::FlamePlan, Point::RewardsPlan]
        .into_iter()
        .chain(
            [
                RollbackStage::RemoveEndorsements,
                RollbackStage::RestoreAcquisitions,
                RollbackStage::StoreDoubleSigns,
                RollbackStage::StoreEndorsements,
                RollbackStage::StorePendingEndorsements,
                RollbackStage::StoreBlockWeights,
                RollbackStage::PrepareFlameTransition,
                RollbackStage::ApplyFlameTransition,
                RollbackStage::PrepareRewards,
                RollbackStage::ApplyRewards,
                RollbackStage::StoreCursor,
            ]
            .into_iter()
            .map(Point::Restore),
        )
    {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            p.apply(fixture.block()).await.unwrap();
            *p.journal.fault.lock().unwrap() = Some((point, after));
            assert!(matches!(
                p.rollback(btc_tip(101)).await,
                Err(BlockProcessingError::Journal("journal interrupted"))
            ));
            let saved = p.journal.pending_entry().await.unwrap().is_some();
            if saved {
                fixture.no_replanning().await;
            }
            p.rollback(btc_tip(101)).await.unwrap();
            assert_eq!(
                p.engine.rollback_calls.load(Ordering::SeqCst),
                if point == Point::BeginRollback && !after {
                    2
                } else {
                    1
                }
            );
            fixture.assert_restored().await;
        }
    }
}

#[tokio::test]
async fn resumes_a_partially_restored_consensus_undo() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    p.apply(fixture.block()).await.unwrap();
    *p.consensus_storage.fail_after_write.lock().unwrap() = Some(3);
    assert!(matches!(
        p.rollback(btc_tip(101)).await,
        Err(BlockProcessingError::Consensus(_))
    ));
    fixture.no_replanning().await;
    assert_eq!(p.resume().await.unwrap(), btc_tip(100));
    fixture.assert_restored().await;
}

#[tokio::test]
async fn manager_serializes_bitcoin_calculation_and_the_entire_application() {
    use crate::block_processing::{ConsensusManager, tests::support::ArrivalHandlerProcessor};
    let fixture = Fixture::new().await;
    let block = fixture.block();
    let done = Arc::new(tokio::sync::Notify::new());
    let manager = Arc::new(ConsensusManager::new(
        fixture.processor,
        ArrivalHandlerProcessor::new(ArrivalProbe(done.clone())),
        fixture.journal,
    ));
    manager.startup().await.unwrap();
    manager.new_bitcoin_block(block.clone()).unwrap();
    manager.new_bitcoin_block(block).unwrap();
    manager.new_flame_block(tip(&fixture.states[0])).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), done.notified())
        .await
        .unwrap();
    manager.shutdown().await.unwrap();
    let processor = manager.bitcoin_processor();
    assert_eq!(processor.engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(processor.consensus_storage.writes.lock().unwrap().len(), 8);
    assert_eq!(
        processor.cursor_storage.get_cursor().await.unwrap(),
        btc_tip(101)
    );
    assert!(processor.journal.pending_entry().await.unwrap().is_none());
}

#[tokio::test]
async fn manager_startup_resumes_the_saved_bitcoin_operation_without_recalculation() {
    use crate::block_processing::{ConsensusManager, tests::support::ArrivalHandlerProcessor};
    let fixture = Fixture::new().await;
    *fixture.processor.journal.fault.lock().unwrap() =
        Some((Point::Apply(ApplyStage::ApplyFlameTransition), true));
    assert!(fixture.processor.apply(fixture.block()).await.is_err());
    fixture.no_replanning().await;
    let original_undo = fixture.entry().await.consensus_undo;
    let manager = Arc::new(ConsensusManager::new(
        fixture.processor,
        ArrivalHandlerProcessor::new(ArrivalProbe(Arc::new(tokio::sync::Notify::new()))),
        fixture.journal.clone(),
    ));
    assert_eq!(manager.startup().await.unwrap(), btc_tip(101));
    manager.shutdown().await.unwrap();
    assert_eq!(
        manager
            .bitcoin_processor()
            .engine
            .calls
            .load(Ordering::SeqCst),
        1
    );
    let latest = fixture.journal.latest_entry().await.unwrap().unwrap();
    assert!(latest.is_complete());
    let crate::block_processing::journal::ConsensusOperation::Bitcoin(operation) = latest.operation
    else {
        panic!()
    };
    assert_eq!(operation.entry.consensus_undo, original_undo);
}

#[tokio::test]
async fn resumes_cursor_writes_before_and_after_persistence_in_both_directions() {
    for rollback in [false, true] {
        for after in [false, true] {
            let fixture = Fixture::new().await;
            let p = &fixture.processor;
            if rollback {
                p.apply(fixture.block()).await.unwrap();
            }
            *p.cursor_storage.fault.lock().unwrap() = Some(after);
            let result = if rollback {
                p.rollback(btc_tip(101)).await
            } else {
                p.apply(fixture.block()).await
            };
            assert!(matches!(
                result,
                Err(BlockProcessingError::Cursor("cursor interrupted"))
            ));
            fixture.no_replanning().await;
            p.resume().await.unwrap();
            if rollback {
                fixture.assert_restored().await;
            } else {
                fixture.assert_applied().await;
            }
        }
    }
}

#[tokio::test]
async fn saved_rewards_are_replayed_and_restored_after_interrupted_writes() {
    let fixture = Fixture::new().await;
    let p = &fixture.processor;
    *p.journal.fault.lock().unwrap() = Some((Point::Apply(ApplyStage::ApplyFlameTransition), true));
    assert!(p.apply(fixture.block()).await.is_err());
    let reward = PendingReward {
        flame_predicate: Predicate::opaque(Predicate::unspendable_key()),
        access_predicate: Predicate::opaque(Predicate::unspendable_key()),
        amount: 42,
    };
    let index = RewardIndex::from(&reward);
    let changes = RewardsChanges {
        outcomes: vec![RewardsOutcome {
            target_block_height: 200.into(),
            rewards: vec![reward],
        }],
        undo: RewardsUndo {
            rewards: BTreeMap::from([((200.into(), index), None)]),
        },
    };
    p.journal
        .store_rewards_changes(
            p.journal.pending_entry().await.unwrap().unwrap().id,
            &changes,
        )
        .await
        .unwrap();
    p.rewards_storage
        .fail_after_write
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        p.resume().await,
        Err(BlockProcessingError::Rewards("reward interrupted"))
    ));
    assert_eq!(p.cursor_storage.get_cursor().await.unwrap(), btc_tip(100));
    fixture.no_replanning().await;
    p.resume().await.unwrap();
    assert_eq!(
        p.rewards_storage
            .get_pending_reward(200.into(), index)
            .await
            .unwrap()
            .unwrap()
            .amount,
        42
    );
    fixture.allow_planning();
    p.rewards_storage
        .fail_after_write
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        p.rollback(btc_tip(101)).await,
        Err(BlockProcessingError::Rewards("reward interrupted"))
    ));
    p.resume().await.unwrap();
    assert!(
        p.rewards_storage
            .get_pending_reward(200.into(), index)
            .await
            .unwrap()
            .is_none()
    );
    fixture.assert_restored().await;
}
