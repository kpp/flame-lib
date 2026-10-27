//! Flavors and quantities as people type and read them.
//!
//! Flame is written in flames with up to eight decimals (`1.5` is
//! 150 000 000 sparks). Any other flavor has no registry to say how many
//! decimals it has, so it is written in whole units.

use std::fmt;
use std::str::FromStr;

use flamechain::SPARKS_PER_FLAME;
use serde::{Serialize, Serializer};

use crate::error::Error;

const FLAME_DECIMALS: usize = 8;

/// A token flavor: the 32-byte canonical scalar every token carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Flavor(pub [u8; 32]);

impl Flavor {
    /// Native Flame, `FLAME_FLAVOR`.
    pub fn flame() -> Flavor {
        Flavor(flamevm::FLAME_FLAVOR.to_bytes())
    }

    pub fn is_flame(&self) -> bool {
        *self == Flavor::flame()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Flavor, Error> {
        <[u8; 32]>::try_from(bytes)
            .map(Flavor)
            .map_err(|_| Error::Invalid(format!("flavor of {} bytes, expected 32", bytes.len())))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

/// `FLM` for Flame, the 64-digit hex otherwise.
impl fmt::Display for Flavor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_flame() {
            f.write_str("FLM")
        } else {
            f.write_str(&self.to_hex())
        }
    }
}

/// `FLM` (any case) or 64 hex digits.
impl FromStr for Flavor {
    type Err = Error;

    fn from_str(text: &str) -> Result<Flavor, Error> {
        if text.eq_ignore_ascii_case("flm") {
            return Ok(Flavor::flame());
        }
        let bytes = hex::decode(text)
            .map_err(|_| Error::Invalid(format!("flavor {text:?} is neither FLM nor hex")))?;
        Flavor::from_bytes(&bytes)
    }
}

/// Always the hex, so a script never has to special-case `FLM`.
impl Serialize for Flavor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

/// Reads a quantity of `flavor`: flames with up to eight decimals for
/// Flame, whole units for anything else. Zero is refused.
pub fn parse_qty(text: &str, flavor: Flavor) -> Result<u64, Error> {
    let invalid = || Error::Invalid(format!("amount {text:?}"));
    let qty = if flavor.is_flame() {
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if fraction.len() > FLAME_DECIMALS
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
            || (whole.is_empty() && fraction.is_empty())
        {
            return Err(invalid());
        }
        let whole: u64 = if whole.is_empty() {
            0
        } else {
            whole.parse().map_err(|_| invalid())?
        };
        let fraction: u64 = format!("{fraction:0<FLAME_DECIMALS$}")
            .parse()
            .map_err(|_| invalid())?;
        whole
            .checked_mul(SPARKS_PER_FLAME)
            .and_then(|sparks| sparks.checked_add(fraction))
            .ok_or_else(invalid)?
    } else {
        text.parse().map_err(|_| invalid())?
    };
    if qty == 0 {
        return Err(Error::Invalid("amount must be more than zero".into()));
    }
    Ok(qty)
}

/// Writes a signed quantity of `flavor` the way [`parse_qty`] reads it,
/// without trailing zeros.
pub fn format_qty(qty: i128, flavor: Flavor) -> String {
    let sign = if qty < 0 { "-" } else { "" };
    let magnitude = qty.unsigned_abs();
    if !flavor.is_flame() {
        return format!("{sign}{magnitude}");
    }
    let per = u128::from(SPARKS_PER_FLAME);
    let fraction = format!("{:0FLAME_DECIMALS$}", magnitude % per);
    let fraction = fraction.trim_end_matches('0');
    if fraction.is_empty() {
        format!("{sign}{}", magnitude / per)
    } else {
        format!("{sign}{}.{fraction}", magnitude / per)
    }
}
