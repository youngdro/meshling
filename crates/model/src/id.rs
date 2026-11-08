use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// A stable object id (a ULID). Written to project files; never changes when an object is renamed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjId(Ulid);

impl ObjId {
    /// A fresh id.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    /// An id from raw bits, for deterministic tests and fixtures.
    pub const fn from_u128(bits: u128) -> Self {
        Self(Ulid(bits))
    }
}

impl fmt::Debug for ObjId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjId({})", self.0)
    }
}

impl fmt::Display for ObjId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The text is not a valid object id.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid object id")]
pub struct ParseIdError;

impl FromStr for ObjId {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ulid::from_string(s).map(Self).map_err(|_| ParseIdError)
    }
}
