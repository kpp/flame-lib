//! The history the chain does not keep.
//!
//! A `Blockchain` keeps roots and an accumulator; it forgets which
//! transaction created which contract, and it never knew which predicate
//! locked it. A wallet needs both. These two maps are that memory, and they
//! live in RAM, except for the bulky bytes they point to in `cold.bin`: an
//! archival node rebuilds both by replaying `blocks.bin`.

pub mod outputindex;
pub mod txindex;

pub use outputindex::{OutputIndex, OutputRecord, Position, SpendRecord, Touch, TxContracts};
pub use txindex::{TxIndex, TxRecord};
