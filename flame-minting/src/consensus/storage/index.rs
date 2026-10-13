use btc_integration::{Acquisition, AuthenticatedMintingEndorsement};
use corepc_client::bitcoin::Txid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AcquisitionIndex {
    pub txid: Txid,
    pub output_index: usize,
}

impl From<&Acquisition> for AcquisitionIndex {
    fn from(acquisition: &Acquisition) -> Self {
        Self {
            txid: acquisition.txid(),
            output_index: acquisition.output_index(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndorsementIndex {
    pub txid: Txid,
    pub input_index: usize,
}

impl From<&AuthenticatedMintingEndorsement> for EndorsementIndex {
    fn from(endorsement: &AuthenticatedMintingEndorsement) -> Self {
        Self {
            txid: endorsement.txid(),
            input_index: endorsement.auth().input_index,
        }
    }
}
