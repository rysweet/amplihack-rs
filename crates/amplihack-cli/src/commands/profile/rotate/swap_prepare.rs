//! Durable ownership of refresh locks before bounded final identity requests.
use super::*;

/// Journal preflight before acquiring locks; no display/profile/login moves occur.
#[allow(clippy::too_many_arguments)]
pub(super) fn begin(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: &Source,
    to: &ProfileName,
    now: i64,
    live: &Path,
    target: &Path,
) -> Result<RotateJournal, ProfileError> {
    let name = match from {
        Source::Profile(name) => name.clone(),
        Source::Adopt => adoption_name(store)?,
    };
    let mut j = RotateJournal {
        warning_transaction: None,
        version: 1,
        from: name.to_string(),
        to: to.to_string(),
        // No adoption directory has been created. Cleanup must not delete one
        // another writer might create while identity reads are in progress.
        adopt: false,
        phase: Phase::Checking,
        home: home
            .to_str()
            .ok_or_else(|| refused("HOME is not UTF-8"))?
            .into(),
        live_ino: ino_at(live).ok_or_else(|| refused(LOGIN_NOT_MOVABLE))?,
        target_ino: ino_at(target).ok_or_else(|| refused(LOGIN_NOT_MOVABLE))?,
        home_lock_ino: None,
        to_lock_ino: None,
        started_at: now,
        pid: std::process::id(),
        identity: None,
    };
    let held = (|| {
        rs.save_journal(&j)?;
        j.home_lock_ino = Some(take_lock(&home.join(".claude"))?);
        rs.save_journal(&j)?;
        j.to_lock_ino = Some(take_lock(&store.root().join(to.as_str()))?);
        step(rs, &mut j, Phase::Checking)
    })();
    if let Err(error) = held {
        release_locks(store, home, &j);
        if let Err(cleanup) = rs.delete_journal() {
            eprintln!("warning: preflight lock journal retained: {cleanup}");
        }
        return Err(error);
    }
    Ok(j)
}
