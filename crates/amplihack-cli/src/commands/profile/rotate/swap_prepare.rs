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
        version: 2,
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
        from_lock_ino: None,
        parking: None,
        completion: None,
        outgoing: None,
        outgoing_lock_ino: None,
        from_directory: match from {
            Source::Profile(name) => {
                let m = fs::symlink_metadata(store.verify_profile_dir(name)?)?;
                Some((m.dev(), m.ino()))
            }
            Source::Adopt => None,
        },
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

/// Takes the refresh lock; an existing lock always refuses.
pub(in crate::commands::profile::rotate) fn take_lock(dir: &Path) -> Result<u64, ProfileError> {
    take_refresh_lock_at(dir).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => refused(REFRESHING),
        _ => refused(format!("Claude's refresh lock could not be taken: {e}")),
    })
}

/// Undoes a swap that stopped before any login moved: releases the locks
/// it took, deletes the journal and removes a profile it adopted, which
/// still holds no login. Returns `err`, or the error that stopped the undo.
pub(in crate::commands::profile::rotate) fn abandon(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    j: &mut RotateJournal,
    err: ProfileError,
) -> ProfileError {
    // The journal forgets the locks before they are released, so a journal
    // left behind cannot later remove a lock Claude made with a reused inode.
    // If it cannot be saved (a full disk), the locks are released anyway:
    // the journal may not record them, and nothing else would.
    let held = j.clone();
    if let Err(e) = drop_locks(store, rs, home, j) {
        eprintln!("warning: the abandoned swap's journal was not updated: {e}");
        release_locks(store, home, &held);
    }
    if let Some(c) = &j.outgoing
        && let Err(e) = c.rollback(store)
    {
        return ProfileError::RotateInconsistent(format!(
            "outgoing reservation retained with journal: {e}"
        ));
    }
    if let Err(e) = rs.delete_journal() {
        eprintln!("warning: the abandoned swap's journal was not removed: {e}");
        return err;
    }
    remove_unused_adoption(store, j);
    err
}

/// Clears the lock inodes from the journal, saves it, then releases the
/// locks. A lock is released at most once, so a later recovery cannot remove
/// a lock Claude made since, even if it was given the same inode.
pub(in crate::commands::profile::rotate) fn drop_locks(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    j: &mut RotateJournal,
) -> Result<(), ProfileError> {
    let held = j.clone();
    if held.home_lock_ino.is_none()
        && held.to_lock_ino.is_none()
        && held.from_lock_ino.is_none()
        && held.outgoing_lock_ino.is_none()
    {
        return Ok(());
    }
    (j.home_lock_ino, j.to_lock_ino, j.from_lock_ino) = (None, None, None);
    j.outgoing_lock_ino = None;
    rs.save_journal(j)?;
    release_locks(store, home, &held);
    Ok(())
}

pub(in crate::commands::profile::rotate) fn release_locks(
    store: &ProfileStore,
    home: &Path,
    j: &RotateJournal,
) {
    if let (Some(c), Some(ino)) = (&j.outgoing, j.outgoing_lock_ino)
        && let Ok(dir) = c.dir(store)
    {
        release_refresh_lock_at(&dir, ino);
    }
    if let Some(ino) = j.home_lock_ino {
        release_refresh_lock_at(&home.join(".claude"), ino);
    }
    if let Some(ino) = j.from_lock_ino
        && source_current(store, j)
    {
        release_refresh_lock_at(&store.root().join(&j.from), ino);
    }
    if let Some(ino) = j.to_lock_ino {
        release_refresh_lock_at(&store.root().join(&j.to), ino);
    }
}

pub(in crate::commands::profile::rotate) fn source_current(
    store: &ProfileStore,
    j: &RotateJournal,
) -> bool {
    j.from_directory.is_none_or(|expected| {
        ProfileName::parse(&j.from)
            .ok()
            .and_then(|name| store.verify_profile_dir(&name).ok())
            .and_then(|dir| fs::symlink_metadata(dir).ok())
            .is_some_and(|m| expected == (m.dev(), m.ino()))
    })
}
