use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum InMemoryConsensusStorageError {
    LockPoisoned,
    BitcoinBlockMismatch,
    AcquisitionKeyMismatch,
    EndorsementKeyMismatch,
    PendingEndorsementKeyMismatch,
    CoreBlockMismatch,
    BlockTipMismatch,
    ConflictingRecord,
    DoubleSignKeyMismatch,
    InvalidDoubleSign,
}

impl fmt::Display for InMemoryConsensusStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LockPoisoned => write!(formatter, "consensus storage lock poisoned"),
            Self::BitcoinBlockMismatch => {
                write!(formatter, "Bitcoin block does not match the record")
            }
            Self::AcquisitionKeyMismatch => {
                write!(formatter, "acquisition key does not match the record")
            }
            Self::EndorsementKeyMismatch => {
                write!(formatter, "endorsement key does not match the record")
            }
            Self::PendingEndorsementKeyMismatch => {
                write!(
                    formatter,
                    "pending endorsement key does not match the record"
                )
            }
            Self::CoreBlockMismatch => {
                write!(formatter, "core block does not match the endorsement")
            }
            Self::BlockTipMismatch => {
                write!(formatter, "block tip does not match the weighted header")
            }
            Self::ConflictingRecord => write!(formatter, "conflicting consensus storage record"),
            Self::DoubleSignKeyMismatch => {
                write!(formatter, "double sign key does not match the record")
            }
            Self::InvalidDoubleSign => write!(
                formatter,
                "double sign must contain conflicting endorsements from the specified minter at the specified height"
            ),
        }
    }
}

impl std::error::Error for InMemoryConsensusStorageError {}
