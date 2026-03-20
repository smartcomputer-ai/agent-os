use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{JournalHeight, model::PersistError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UniverseId(Uuid);

impl UniverseId {
    pub fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for UniverseId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for UniverseId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for UniverseId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorldId(Uuid);

impl WorldId {
    pub fn new(value: Uuid) -> Self {
        Self(value)
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl From<Uuid> for WorldId {
    fn from(value: Uuid) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for WorldId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for WorldId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// Opaque, serializable, totally ordered cursor token.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboxSeq(#[serde(with = "serde_bytes")] Vec<u8>);

impl InboxSeq {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn from_u64(value: u64) -> Self {
        Self(value.to_be_bytes().to_vec())
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for InboxSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InboxSeq({})", hex::encode(&self.0))
    }
}

impl fmt::Display for InboxSeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(&self.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SegmentId {
    pub start: JournalHeight,
    pub end: JournalHeight,
}

impl SegmentId {
    pub fn new(start: JournalHeight, end: JournalHeight) -> Result<Self, PersistError> {
        if end < start {
            return Err(PersistError::validation(format!(
                "segment end {end} must be >= start {start}"
            )));
        }
        Ok(Self { start, end })
    }
}
