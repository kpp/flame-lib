//! The wallet file: the secret, the network, the node, the receiving
//! counter and any other paths handed out. That is the whole of a wallet's
//! own state; coins and history are the node's to answer.
//!
//! The secret is stored in the clear. This is a development wallet: the
//! file is created readable by its owner only, and nothing more is done.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use flamewallet_ffi::{Network, Wallet};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::error::Error;
use crate::path::DerivationPath;

const FORMAT_VERSION: u32 = 1;

/// Where `flamed` serves JSON-RPC by default.
pub const DEFAULT_NODE: &str = "http://127.0.0.1:8545";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletFile {
    pub version: u32,
    pub network: NetworkName,
    /// The node's JSON-RPC URL.
    pub node: String,
    pub secret: Secret,
    /// The next receiving index not yet issued. Every index below it, and
    /// index 0 always, is watched.
    pub next_index: u32,
    /// Other paths `address --path` has handed out, watched as well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<DerivationPath>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkName {
    Mainnet,
    Testnet,
}

impl FromStr for NetworkName {
    type Err = Error;

    fn from_str(text: &str) -> Result<NetworkName, Error> {
        match text {
            "mainnet" => Ok(NetworkName::Mainnet),
            "testnet" => Ok(NetworkName::Testnet),
            _ => Err(Error::Invalid(format!(
                "network {text:?}: mainnet or testnet"
            ))),
        }
    }
}

impl From<NetworkName> for Network {
    fn from(name: NetworkName) -> Network {
        match name {
            NetworkName::Mainnet => Network::Mainnet,
            NetworkName::Testnet => Network::Testnet,
        }
    }
}

/// What the account is derived from.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Secret {
    /// A BIP-39 phrase, as Safely and `flamed address` take it.
    Mnemonic {
        phrase: String,
        #[serde(default)]
        passphrase: String,
    },
    /// A raw 64-byte seed in hex, as the node's tests hold their accounts:
    /// the devnet's genesis allocation is under the seed `a1` × 64.
    Seed { hex: String },
}

impl Drop for Secret {
    fn drop(&mut self) {
        match self {
            Secret::Mnemonic { phrase, passphrase } => {
                phrase.zeroize();
                passphrase.zeroize();
            }
            Secret::Seed { hex } => hex.zeroize(),
        }
    }
}

impl WalletFile {
    pub fn load(path: &Path) -> Result<WalletFile, Error> {
        let text = Zeroizing::new(fs::read_to_string(path).map_err(|source| Error::File {
            path: path.display().to_string(),
            source,
        })?);
        let file: WalletFile = serde_json::from_str(&text).map_err(|error| Error::Format {
            path: path.display().to_string(),
            reason: error.to_string(),
        })?;
        if file.version != FORMAT_VERSION {
            return Err(Error::Format {
                path: path.display().to_string(),
                reason: format!(
                    "version {}, this wallet reads {FORMAT_VERSION}",
                    file.version
                ),
            });
        }
        Ok(file)
    }

    /// Writes a new file; an existing one is never overwritten.
    pub fn create(&self, path: &Path) -> Result<(), Error> {
        if path.exists() {
            return Err(Error::Exists {
                path: path.display().to_string(),
            });
        }
        self.write(path)
    }

    /// Replaces the file with this one, through a rename, so a crash leaves
    /// either the old file or the new one.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        self.write(path)
    }

    fn write(&self, path: &Path) -> Result<(), Error> {
        let io = |source| Error::File {
            path: path.display().to_string(),
            source,
        };
        let text = Zeroizing::new(
            serde_json::to_string_pretty(self).expect("a wallet file serializes") + "\n",
        );
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");
        let temporary = Path::new(&temporary);

        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut out = options.open(temporary).map_err(io)?;
        out.write_all(text.as_bytes()).map_err(io)?;
        out.sync_all().map_err(io)?;
        fs::rename(temporary, path).map_err(io)
    }

    /// The account this file describes, with its counter where the file
    /// left it.
    pub fn open(&self) -> Result<Arc<Wallet>, Error> {
        let network = self.network.into();
        Ok(match &self.secret {
            Secret::Mnemonic { phrase, passphrase } => {
                Wallet::from_mnemonic(phrase.clone(), passphrase.clone(), network, self.next_index)?
            }
            Secret::Seed { hex } => {
                let seed = Zeroizing::new(
                    hex::decode(hex.trim())
                        .map_err(|_| Error::Invalid("the seed is not hex".into()))?,
                );
                Wallet::new(seed.to_vec(), network, self.next_index)?
            }
        })
    }
}
