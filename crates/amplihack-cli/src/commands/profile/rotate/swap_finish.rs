//! Completion writes display and ledger identity from the incoming evidence.
use super::identify::{Account, LoginKind, login_kind};
use super::ledger::{LIVE_KIND_PLATFORM, Phase, RotateJournal, RotateState};
use super::swap::{HOME_JSON_UNREADABLE, step};
use super::swap_authorize::refused;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CLAUDE_JSON, CREDS, exists, read_claude_json, write_atomic_keep_mode,
};
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::path::Path;

/// Write `s3` and `s4`; the caller releases its locks before deleting the journal.
pub(super) fn finish(
    rs: &RotateState,
    home: &Path,
    j: &RotateJournal,
    incoming: &super::login_identity::Bound,
) -> Result<(), ProfileError> {
    let mut j = j.clone();
    if !incoming.current() {
        return Err(ProfileError::RotateInconsistent(
            super::login_identity::UNAVAILABLE.into(),
        ));
    }
    let block = incoming.account().map(Account::block);
    patch_home_account(home, block.as_ref())?;
    step(rs, &mut j, Phase::S3)?;
    let mut l = rs.load_ledger()?.unwrap_or_default();
    l.live = Some(j.to.clone());
    l.previous = Some(j.from.clone());
    l.account = incoming.account().map(|a| a.key().to_string());
    l.login_identity = incoming.account().map(super::login_identity::Identity::new);
    // Issue #1700: the incoming login is now in `~/.claude`; one without
    // OAuth is only ever an API-key profile's (`swap` checks that).
    l.live_kind = (login_kind(&home.join(".claude").join(CREDS)) != LoginKind::OAuth)
        .then(|| LIVE_KIND_PLATFORM.to_string());
    l.since = Some(j.started_at);
    l.swapped_at = Some(j.started_at);
    super::alert::clear_alert(&mut l);
    rs.save_ledger(&l)?;
    step(rs, &mut j, Phase::S4)?;
    if let Some(transaction) = &j.warning_transaction {
        super::super::swap_warning::complete(
            &rs.root,
            transaction,
            "rotator",
            "home",
            &j.from,
            &j.to,
        )?;
    }
    Ok(())
}

/// `s3`: replaces only `oauthAccount` in `$HOME/.claude.json`, keeping
/// every other key and the file's mode. A missing file is created. With no
/// block (an API-key profile without `oauthAccount`) the file is left
/// byte for byte as it was.
fn patch_home_account(home: &Path, block: Option<&serde_json::Value>) -> Result<(), ProfileError> {
    let Some(block) = block else {
        return Ok(());
    };
    let mut v = match read_claude_json(home) {
        Some((v, _)) => v,
        None if !exists(&home.join(CLAUDE_JSON)) => serde_json::json!({}),
        // `swap` refuses this before J0; only a file damaged since then,
        // during recovery, gets here.
        None => {
            eprintln!("warning: {HOME_JSON_UNREADABLE}; its account was not updated");
            return Ok(());
        }
    };
    if v.get("oauthAccount") == Some(block) {
        return Ok(());
    }
    v["oauthAccount"] = block.clone();
    let bytes = serde_json::to_vec_pretty(&v).map_err(|e| refused(e.to_string()))?;
    write_atomic_keep_mode(home, CLAUDE_JSON, &bytes).map_err(ProfileError::Io)
}

/// Removes a profile the swap adopted that still holds no login: nothing
/// but the copied `oauthAccount` block is in it.
pub(super) fn remove_unused_adoption(store: &ProfileStore, j: &RotateJournal) {
    let dir = store.root().join(&j.from);
    if j.adopt
        && !exists(&dir.join(CREDS))
        && let Err(e) = fs::remove_dir_all(&dir)
    {
        eprintln!("warning: unused profile {} was not removed: {e}", j.from);
    }
}
