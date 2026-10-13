use super::*;

const LIFECYCLE: [JournalStatus; 17] = [
    JournalStatus::Applying(ApplyStage::RemoveEndorsements),
    JournalStatus::Applying(ApplyStage::StoreEndorsements),
    JournalStatus::Applying(ApplyStage::StoreAcquisitions),
    JournalStatus::Applying(ApplyStage::StoreBlockWeights),
    JournalStatus::Applying(ApplyStage::StoreDoubleSigns),
    JournalStatus::Applying(ApplyStage::StorePendingEndorsements),
    JournalStatus::Applying(ApplyStage::PrepareFlameTransition),
    JournalStatus::Applying(ApplyStage::ApplyFlameTransition),
    JournalStatus::Applying(ApplyStage::PrepareRewards),
    JournalStatus::Applying(ApplyStage::ApplyRewards),
    JournalStatus::Applying(ApplyStage::StoreCursor),
    JournalStatus::Applied,
    JournalStatus::RollingBack(RollbackStage::RestoreRewards),
    JournalStatus::RollingBack(RollbackStage::RestoreFlameChain),
    JournalStatus::RollingBack(RollbackStage::RestoreConsensus),
    JournalStatus::RollingBack(RollbackStage::RestoreCursor),
    JournalStatus::RolledBack,
];

#[test]
fn applies_then_rolls_back_in_order() {
    let mut status = LIFECYCLE[0];
    for &next in &LIFECYCLE[1..] {
        assert_eq!(status.next(), Some(next));
        status.transition_to(next).unwrap();
        assert_eq!(status, next);
    }
    assert_eq!(status.next(), None);
}

#[test]
fn every_status_accepts_only_itself_or_its_immediate_successor() {
    for (current_index, &current) in LIFECYCLE.iter().enumerate() {
        for (requested_index, &requested) in LIFECYCLE.iter().enumerate() {
            let mut status = current;
            let result = status.transition_to(requested);
            if requested_index == current_index || requested_index == current_index + 1 {
                assert_eq!(result, Ok(()));
                assert_eq!(status, requested);
            } else {
                assert_eq!(result, Err(JournalTransitionError { current, requested }));
                assert_eq!(status, current);
            }
        }
    }
}
