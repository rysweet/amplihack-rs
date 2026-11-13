//! Execution identity, independent of optional issue tracking.
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkstreamId(uuid::Uuid);

impl WorkstreamId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for WorkstreamId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WorkstreamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for WorkstreamId {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let id = uuid::Uuid::parse_str(s)?;
        anyhow::ensure!(!id.is_nil(), "workstream identity must not be nil");
        Ok(Self(id))
    }
}

impl Serialize for WorkstreamId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for WorkstreamId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}
