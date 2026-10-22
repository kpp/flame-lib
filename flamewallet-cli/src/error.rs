//! Why a wallet command failed.

use crate::amount::Flavor;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("wallet file {path}: {source}")]
    File {
        path: String,
        source: std::io::Error,
    },
    #[error("wallet file {path}: {reason}")]
    Format { path: String, reason: String },
    #[error("wallet file {path} already exists; refusing to overwrite it")]
    Exists { path: String },
    #[error(transparent)]
    Wallet(#[from] flamewallet_ffi::FlameError),
    #[error("node: {0}")]
    Rpc(#[from] flamed_rpc::ClientError),
    #[error("{0}")]
    Invalid(String),
    #[error("not enough {flavor}: {have} spendable, {need} needed")]
    Insufficient {
        flavor: Flavor,
        have: String,
        need: String,
    },
    #[error("transaction {txid} is not confirmed after {secs}s")]
    Timeout { txid: String, secs: u64 },
}
