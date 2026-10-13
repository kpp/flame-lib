use btc_integration::BtcBlockTip;
use flame_storage::ChainStorage;
use flamechain::BlockHeader;

use super::journal::RewardsChanges;
use crate::{
    consensus::ConsensusStorage,
    rewards::{RewardsEngine, RewardsEngineError, RewardsStorage, RewardsUndo, RewardsUndoManager},
};

pub(super) async fn prepare<T: ChainStorage, C: ConsensusStorage, R: RewardsStorage>(
    engine: &RewardsEngine<T, C>,
    storage: &R,
    headers: &[BlockHeader],
    btc_tip: BtcBlockTip,
) -> Result<RewardsChanges, PrepareRewardsError<T::Error, C::Error, R::Error>> {
    let mut outcomes = Vec::new();
    for header in headers {
        if let Some(outcome) = engine
            .calculate(header, btc_tip)
            .await
            .map_err(PrepareRewardsError::Engine)?
        {
            outcomes.push(outcome);
        }
    }
    let mut undo = RewardsUndo::default();
    let manager = RewardsUndoManager {
        rewards_storage: storage,
    };
    for outcome in &outcomes {
        let previous = manager
            .prepare(outcome)
            .await
            .map_err(PrepareRewardsError::Storage)?;
        for (key, value) in previous.rewards {
            undo.rewards.entry(key).or_insert(value);
        }
    }
    Ok(RewardsChanges { outcomes, undo })
}

pub(super) async fn apply<R: RewardsStorage>(
    storage: &R,
    changes: &RewardsChanges,
) -> Result<(), R::Error> {
    for outcome in &changes.outcomes {
        crate::rewards::applier::apply_outcome(storage, outcome).await?;
    }
    Ok(())
}

#[derive(Debug)]
pub enum PrepareRewardsError<T, C, R> {
    Engine(RewardsEngineError<T, C>),
    Storage(R),
}
