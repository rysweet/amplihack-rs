//! The snapshot file's format (D2): a versioned header and one scalar
//! record per same-user process. No paths, command lines or environments.
//! Unknown fields are ignored so a later version can add some.

use super::{Collected, ProcRecord};
use crate::commands::profile::name::ProfileName;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub uid: u32,
    pub root_hash: String,
    pub taken_at: i64,
    pub writer_pid: u32,
    pub outcome: Outcome,
    pub procs: Vec<ProcRecord>,
}

impl Snapshot {
    /// The snapshot a writer stores for `collected`. A profile name no
    /// reader would accept is dropped from its record: it can never match a
    /// profile, and a reader rejects the whole file for one.
    pub fn from_collected(
        collected: &Collected,
        uid: u32,
        root_hash: &str,
        taken_at: i64,
        writer_pid: u32,
    ) -> Self {
        let (outcome, procs) = match collected {
            Collected::Unsupported => (Outcome::Unsupported, Vec::new()),
            Collected::Procs(procs) => (
                Outcome::Ok,
                procs
                    .iter()
                    .cloned()
                    .map(|mut r| {
                        r.profile = r.profile.filter(|p| ProfileName::parse(p).is_ok());
                        r
                    })
                    .collect(),
            ),
        };
        Self {
            version: VERSION,
            uid,
            root_hash: root_hash.to_string(),
            taken_at,
            writer_pid,
            outcome,
            procs,
        }
    }

    /// The records, moved out: a served snapshot is not used again.
    pub fn into_collected(self) -> Collected {
        match self.outcome {
            Outcome::Unsupported => Collected::Unsupported,
            Outcome::Ok => Collected::Procs(self.procs),
        }
    }

    /// Whether this file belongs to `uid` and `root_hash`, is of this
    /// version, and names only valid profiles (SR12).
    pub fn matches(&self, uid: u32, root_hash: &str) -> bool {
        self.version == VERSION
            && self.uid == uid
            && self.root_hash == root_hash
            && self
                .procs
                .iter()
                .filter_map(|r| r.profile.as_deref())
                .all(|p| ProfileName::parse(p).is_ok())
    }
}
