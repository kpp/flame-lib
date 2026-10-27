use corepc_client::client_sync::Error as RpcError;

use super::types::MAX_HISTORY_BLOCKS;
use crate::{btc::rpc::BtcBlockTip, protocol::MintingEndorsementProcessingError};

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("protocol indexer is not running")]
    NotRunning,
    #[error("history page size must not exceed {MAX_HISTORY_BLOCKS} blocks")]
    InvalidLimit,
    #[error("cursor height does not match its Bitcoin header: {0:?}")]
    InvalidCursor(BtcBlockTip),
    #[error("Bitcoin Core does not know cursor {0:?}")]
    UnknownCursor(BtcBlockTip),
    #[error("Bitcoin history is unavailable: {0}")]
    HistoryUnavailable(String),
    #[error("Bitcoin Core RPC error: {0}")]
    Rpc(#[from] RpcError),
    #[error("failed to process minting endorsements: {0}")]
    EndorsementProcessing(#[source] MintingEndorsementProcessingError),
    #[error("Bitcoin target tip was reorganized during the history request")]
    ChainChanged,
}

impl From<MintingEndorsementProcessingError> for HistoryError {
    fn from(error: MintingEndorsementProcessingError) -> Self {
        match error {
            MintingEndorsementProcessingError::MissingPrevout { .. } => {
                Self::HistoryUnavailable(error.to_string())
            }
            _ => Self::EndorsementProcessing(error),
        }
    }
}

pub(super) fn history_rpc_error(error: RpcError) -> HistoryError {
    match &error {
        RpcError::JsonRpc(jsonrpc::Error::Rpc(response))
            if response.code == -5
                || (response.code == -1
                    && (response.message.contains("not available")
                        || response.message.contains("pruned"))) =>
        {
            HistoryError::HistoryUnavailable(error.to_string())
        }
        _ => HistoryError::Rpc(error),
    }
}
