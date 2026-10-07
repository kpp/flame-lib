use std::future::Future;

use btc_integration::{AuthenticatedMintingEndorsement, MintingEndorsementData};

pub mod in_memory;

pub use in_memory::{InMemoryMinterJournal, InMemoryMinterJournalError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndorsementReservation {
    Reserved,
    AlreadyReserved,
}

pub trait MinterJournal {
    type Error;
    type TransactionId;

    fn write_reserve_endorsement(
        &self,
        endorsement: &MintingEndorsementData,
    ) -> impl Future<Output = Result<EndorsementReservation, Self::Error>> + Send;

    fn write_sent_endorsement(
        &self,
        endorsement: &MintingEndorsementData,
        transaction_id: &Self::TransactionId,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn write_committed_endorsement(
        &self,
        endorsement: &AuthenticatedMintingEndorsement,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
