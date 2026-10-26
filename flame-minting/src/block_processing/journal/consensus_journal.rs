use super::{BitcoinJournal, ConsensusJournalEntry, FlameJournal};
use std::future::Future;

pub trait ConsensusJournal: Send + Sync + 'static {
    type Error: Send + 'static;
    type Bitcoin: BitcoinJournal<Error = Self::Error>;
    type Flame: FlameJournal<Error = Self::Error>;

    fn bitcoin_journal(&self) -> Self::Bitcoin;

    fn flame_journal(&self) -> Self::Flame;

    fn latest_entry(
        &self,
    ) -> impl Future<Output = Result<Option<ConsensusJournalEntry>, Self::Error>> + Send;
}
