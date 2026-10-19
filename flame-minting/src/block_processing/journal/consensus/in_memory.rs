use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use corepc_client::bitcoin::BlockHash;

use super::*;

mod bitcoin;
mod error;
mod flame;

use InMemoryConsensusJournalError as Error;
pub use error::InMemoryConsensusJournalError;

#[derive(Clone)]
pub struct InMemoryConsensusJournal {
    state: Arc<RwLock<State>>,
}

#[derive(Clone)]
pub struct InMemoryBitcoinJournal {
    journal: InMemoryConsensusJournal,
}

#[derive(Clone)]
pub struct InMemoryFlameJournal {
    journal: InMemoryConsensusJournal,
}

struct State {
    next_id: u64,
    applied_btc_tip: BtcBlockTip,
    pending: Option<OperationId>,
    entries: BTreeMap<OperationId, ConsensusJournalEntry>,
    bitcoin_entries: BTreeMap<BlockHash, OperationId>,
}

impl InMemoryConsensusJournal {
    pub fn new(initial_btc_tip: BtcBlockTip) -> Self {
        Self {
            state: Arc::new(RwLock::new(State {
                next_id: 0,
                applied_btc_tip: initial_btc_tip,
                pending: None,
                entries: BTreeMap::new(),
                bitcoin_entries: BTreeMap::new(),
            })),
        }
    }

    fn read(&self) -> Result<RwLockReadGuard<'_, State>, Error> {
        self.state.read().map_err(|_| Error::LockPoisoned)
    }

    fn write(&self) -> Result<RwLockWriteGuard<'_, State>, Error> {
        self.state.write().map_err(|_| Error::LockPoisoned)
    }
}

impl ConsensusJournal for InMemoryConsensusJournal {
    type Error = Error;
    type Bitcoin = InMemoryBitcoinJournal;
    type Flame = InMemoryFlameJournal;

    fn bitcoin_journal(&self) -> Self::Bitcoin {
        InMemoryBitcoinJournal {
            journal: self.clone(),
        }
    }

    fn flame_journal(&self) -> Self::Flame {
        InMemoryFlameJournal {
            journal: self.clone(),
        }
    }

    async fn latest_entry(&self) -> Result<Option<ConsensusJournalEntry>, Error> {
        Ok(self
            .read()?
            .entries
            .last_key_value()
            .map(|(_, entry)| entry.clone()))
    }
}

impl State {
    fn require_idle(&self) -> Result<(), Error> {
        if let Some(id) = self.pending {
            return Err(Error::OperationInProgress(id));
        }
        Ok(())
    }

    fn require_pending(&self, id: OperationId) -> Result<(), Error> {
        if self.pending != Some(id) {
            return Err(Error::NotPending(id));
        }
        Ok(())
    }

    fn begin(&mut self, operation: ConsensusOperation) -> Result<OperationId, Error> {
        self.require_idle()?;
        let id = OperationId(self.next_id);
        let next_id = self.next_id.checked_add(1).ok_or(Error::IdOverflow)?;
        if let ConsensusOperation::Bitcoin(operation) = &operation {
            self.bitcoin_entries
                .insert(operation.entry.btc_tip.hash, id);
        }
        self.entries
            .insert(id, ConsensusJournalEntry { id, operation });
        self.next_id = next_id;
        self.pending = Some(id);
        Ok(id)
    }

    fn bitcoin(&self, id: OperationId) -> Result<&BitcoinJournalOperation, Error> {
        match &self
            .entries
            .get(&id)
            .ok_or(Error::MissingEntry(id))?
            .operation
        {
            ConsensusOperation::Bitcoin(operation) => Ok(operation),
            ConsensusOperation::Flame(_) => Err(Error::WrongOperationKind(id)),
        }
    }

    fn bitcoin_mut(&mut self, id: OperationId) -> Result<&mut BitcoinJournalOperation, Error> {
        match &mut self
            .entries
            .get_mut(&id)
            .ok_or(Error::MissingEntry(id))?
            .operation
        {
            ConsensusOperation::Bitcoin(operation) => Ok(operation),
            ConsensusOperation::Flame(_) => Err(Error::WrongOperationKind(id)),
        }
    }

    fn flame(&self, id: OperationId) -> Result<&FlameJournalOperation, Error> {
        match &self
            .entries
            .get(&id)
            .ok_or(Error::MissingEntry(id))?
            .operation
        {
            ConsensusOperation::Flame(entry) => Ok(entry),
            ConsensusOperation::Bitcoin(_) => Err(Error::WrongOperationKind(id)),
        }
    }

    fn flame_mut(&mut self, id: OperationId) -> Result<&mut FlameJournalOperation, Error> {
        match &mut self
            .entries
            .get_mut(&id)
            .ok_or(Error::MissingEntry(id))?
            .operation
        {
            ConsensusOperation::Flame(entry) => Ok(entry),
            ConsensusOperation::Bitcoin(_) => Err(Error::WrongOperationKind(id)),
        }
    }

    fn bitcoin_id(&self, tip: BtcBlockTip) -> Result<Option<OperationId>, Error> {
        let Some(&id) = self.bitcoin_entries.get(&tip.hash) else {
            return Ok(None);
        };
        check_tip(self.bitcoin(id)?.entry.btc_tip, tip)?;
        Ok(Some(id))
    }
}

fn check_tip(expected: BtcBlockTip, actual: BtcBlockTip) -> Result<(), Error> {
    if expected != actual {
        return Err(Error::BlockTipMismatch { expected, actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
