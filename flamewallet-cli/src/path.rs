//! Key paths below the account, as people type them: `branch/index`.

use std::fmt;
use std::str::FromStr;

use flamewallet_ffi::KeyPath;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::Error;

/// `m/…/branch/index`, both normal. Branch 0 is receiving and 1 change;
/// any other branch is the user's own, and the wallet watches it only once
/// `address --path` has handed it out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DerivationPath {
    pub branch: u32,
    pub index: u32,
}

impl DerivationPath {
    pub fn key_path(self) -> KeyPath {
        KeyPath {
            branch: self.branch,
            index: self.index,
        }
    }
}

impl From<KeyPath> for DerivationPath {
    fn from(path: KeyPath) -> DerivationPath {
        DerivationPath {
            branch: path.branch,
            index: path.index,
        }
    }
}

impl fmt::Display for DerivationPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.branch, self.index)
    }
}

/// Two decimal numbers below 2³¹, joined by `/`.
impl FromStr for DerivationPath {
    type Err = Error;

    fn from_str(text: &str) -> Result<DerivationPath, Error> {
        let invalid = || Error::Invalid(format!("path {text:?}: branch/index, as 2/0"));
        let (branch, index) = text.split_once('/').ok_or_else(invalid)?;
        let normal = |part: &str| {
            part.parse::<u32>()
                .ok()
                .filter(|n| *n < 1 << 31 && part.bytes().all(|b| b.is_ascii_digit()))
                .ok_or_else(invalid)
        };
        Ok(DerivationPath {
            branch: normal(branch)?,
            index: normal(index)?,
        })
    }
}

impl Serialize for DerivationPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for DerivationPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<DerivationPath, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
