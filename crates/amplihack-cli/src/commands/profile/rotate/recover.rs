//! Recovery of an interrupted swap (D10): settles it by where the recorded
//! inodes are. Only renames; never deletes or opens a login.

use super::ledger::{Phase, RotateJournal, RotateState};
use super::swap::{LOGINS_MISPLACED, drop_locks, finish, ino_at, show};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{CREDS, move_login};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverOutcome {
    None,
    RolledBack { from: ProfileName, to: ProfileName },
    RolledForward { from: ProfileName, to: ProfileName },
    Inconsistent(String),
}

/// Marks the journal `inconsistent`: no swap until a person resolves it.
fn freeze(
    rs: &RotateState,
    mut j: RotateJournal,
    reason: &str,
) -> Result<RecoverOutcome, ProfileError> {
    j.phase = Phase::Inconsistent;
    rs.save_journal(&j)?;
    Ok(RecoverOutcome::Inconsistent(reason.to_string()))
}

/// Settles an interrupted swap by where the recorded inodes are; the
/// phase never decides. Only renames; never deletes or opens a login.
pub fn recover(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
) -> Result<RecoverOutcome, ProfileError> {
    let Some(mut j) = rs.load_journal()? else {
        return Ok(RecoverOutcome::None);
    };
    if j.phase == Phase::Inconsistent {
        return Ok(RecoverOutcome::Inconsistent(
            "an earlier swap stopped half way".into(),
        ));
    }
    if home.to_str() != Some(j.home.as_str()) {
        return freeze(rs, j, "the journal is for another home directory");
    }
    // Names were checked when the journal was loaded.
    let (from, to) = (ProfileName::parse(&j.from)?, ProfileName::parse(&j.to)?);
    let (Ok(from_dir), Ok(to_dir)) = (
        store.verify_profile_dir(&from),
        store.verify_profile_dir(&to),
    ) else {
        return freeze(rs, j, "a profile directory failed the directory check");
    };
    let l = home.join(".claude").join(CREDS);
    if !fs::symlink_metadata(&l).is_ok_and(|m| m.is_file()) {
        return freeze(rs, j, "no login in ~/.claude");
    }
    let (t, s) = (to_dir.join(CREDS), from_dir.join(CREDS));
    let (at_t, at_s) = (ino_at(&t), ino_at(&s));
    if at_t == Some(j.target_ino) && at_s.is_none() {
        drop_locks(store, rs, home, &mut j)?;
        rs.delete_journal()?;
        return Ok(RecoverOutcome::RolledBack { from, to });
    }
    if at_t == Some(j.live_ino) && at_s.is_none() {
        if move_login(&t, &s, &show).is_err() {
            return freeze(
                rs,
                j,
                "the outgoing login could not be moved to its profile",
            );
        }
    } else if !(at_s == Some(j.live_ino) && at_t.is_none()) {
        return freeze(rs, j, LOGINS_MISPLACED);
    }
    drop_locks(store, rs, home, &mut j)?;
    finish(store, rs, home, &j)?;
    Ok(RecoverOutcome::RolledForward { from, to })
}
