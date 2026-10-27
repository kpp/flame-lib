//! A Flame wallet that keeps its state in one file and asks `flamed` for
//! everything else.
//!
//! The node does the scanning: [`Session::coins`] is one `unspent` call per
//! watched address, and [`Session::history`] one `history` page. Nothing is
//! cached between runs but the secret and the receiving counter, so a run
//! costs the same on a fresh devnet and on a long chain.
//!
//! Every amount is per flavor. Fees are always Flame, so a send of any
//! other flavor also spends a Flame coin.

mod amount;
mod error;
mod path;
mod session;
mod store;

pub use amount::{format_qty, parse_qty, Flavor};
pub use error::Error;
pub use path::DerivationPath;
pub use session::{
    Balance, Built, Coin, CoinValue, Coins, FlavorDelta, HistoryItem, HistoryPage, IssueRequest,
    Issued, Payment, SendRequest, Sent, Session, DEFAULT_FEE, DEFAULT_GAS,
};
pub use store::{NetworkName, Secret, WalletFile, DEFAULT_NODE};

#[cfg(test)]
mod tests;
