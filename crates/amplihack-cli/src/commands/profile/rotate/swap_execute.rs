//! Execute one authorised custody strategy under journalled refresh locks.
use crate::commands::profile::rotate::swap::*;

#[allow(clippy::too_many_arguments)]
pub(in crate::commands::profile::rotate) fn swap_operation_custody(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: Source,
    to: &ProfileName,
    now: i64,
    reader: Reader<'_>,
    basis: Option<&crate::commands::profile::rotate::observe::Snap>,
    completion: Option<crate::commands::profile::rotate::swap_record::Completion>,
    clock: fn() -> i64,
    strategy: &mut crate::commands::profile::rotate::custody::Strategy,
) -> Result<ProfileName, ProfileError> {
    check_root_private(store.root()).map_err(|e| refused(format!("profile root {e}")))?;
    let dot = check_home(home)?;
    if exists(&rs.journal) {
        return Err(refused("a swap journal is pending; run recovery first"));
    }
    let to_dir = store
        .verify_profile_dir(to)
        .map_err(|_| refused(format!("profile {to} failed the directory check")))?;
    let live_dir = super::super::verified_dirs::Directory::open(&dot, None)?;
    let target_dir =
        super::super::verified_dirs::Directory::open(&store.root().join(to.as_str()), None)?;
    let (l, t) = (dot.join(CREDS), to_dir.join(CREDS));
    if !movable_login(&l) || !movable_login(&t) {
        return Err(refused(LOGIN_NOT_MOVABLE));
    }
    // Issue #1700: `~/.claude` is never left without an OAuth login unless
    // the incoming profile is an API-key profile; checked before J0, so a
    // refusal writes no journal, and again once J0 records `T`'s inode.
    let platform = kind::kind_of(store, to, home) == Kind::Platform;
    if !may_go_live(&t, platform) {
        return Err(no_oauth(to));
    }
    // `s3` could not record the new account; nothing has moved yet.
    if exists(&home.join(CLAUDE_JSON)) && read_claude_json(home).is_none() {
        return Err(refused(HOME_JSON_UNREADABLE));
    }
    let dev = |p: &Path| fs::metadata(p).map(|m| m.dev()).ok();
    if dev(&dot).is_none() || dev(&dot) != dev(&to_dir) {
        return Err(refused(DIFFERENT_DEVICES));
    }
    #[cfg(test)]
    let test_exchange = crate::commands::profile::auto_switch::kit::test_exchange::active();
    #[cfg(not(test))]
    let test_exchange = false;
    if !cfg!(target_os = "linux") && !test_exchange {
        return Err(refused(NO_EXCHANGE));
    }
    // Refuses before anything is written; the locks are taken below.
    if exists(&dot.join(REFRESH_LOCK)) || exists(&to_dir.join(REFRESH_LOCK)) {
        return Err(refused(REFRESHING));
    }
    if let Source::Profile(name) = &from {
        let dir = store
            .verify_profile_dir(name)
            .map_err(|_| refused(format!("profile {name} failed the directory check")))?;
        if name == to || (completion.is_none() && exists(&dir.join(CREDS))) {
            return Err(refused(format!("profile {name} {ALREADY_LOGGED_IN}")));
        }
    }
    let platform_out = login_kind(&l) == LoginKind::NoOAuth
        && matches!(&from, Source::Profile(n) if kind::kind_of(store, n, home) == Kind::Platform);
    let slot = if completion.is_some() {
        crate::commands::profile::rotate::duplicate::verify(store, rs, home, &from, reader)?
    } else {
        crate::commands::profile::rotate::duplicate::Slot::Vacant
    };
    let proof = match &slot {
        crate::commands::profile::rotate::duplicate::Slot::Vacant => Proof::Live,
        crate::commands::profile::rotate::duplicate::Slot::Duplicate(p) => Proof::Duplicate(p),
        crate::commands::profile::rotate::duplicate::Slot::Conflict(p)
            if matches!(
                strategy,
                crate::commands::profile::rotate::custody::Strategy::AfterConflictRefusal
            ) =>
        {
            Proof::Conflict(p)
        }
        crate::commands::profile::rotate::duplicate::Slot::Conflict(_) => {
            *strategy = crate::commands::profile::rotate::custody::Strategy::AfterConflictRefusal;
            return Err(refused(
                "different-account outgoing conflict; both logins preserved; trying the next eligible candidate",
            ));
        }
    };
    let first = authorize(
        store,
        rs,
        &from,
        to,
        &dot,
        &to_dir,
        platform,
        platform_out,
        reader,
        proof,
    )?;
    crate::commands::profile::rotate::swap_authorize::check_snapshot(&first, basis, to)?;
    // Persist owned locks before final identity requests can block or terminate.
    let mut j = prepare::begin(store, rs, home, &from, to, now, &l, &t)?;
    j.movement_directories = Some([live_dir.id(), target_dir.id()]);
    rs.save_journal(&j)?;
    let checked = authorize(
        store,
        rs,
        &from,
        to,
        &dot,
        &to_dir,
        platform,
        platform_out,
        reader,
        proof,
    );
    let mut auth = match checked {
        Ok(auth) if first.same(&auth) && auth.current(store) => auth,
        other => {
            let error = other.err().unwrap_or_else(|| refused(UNAVAILABLE));
            return Err(abandon(store, rs, home, &mut j, error));
        }
    };
    if completion.is_some() {
        j.version = 2;
        j.completion = completion;
        rs.save_journal(&j)?;
    }
    if let crate::commands::profile::rotate::duplicate::Slot::Duplicate(proof) = &slot {
        if let Err(e) = crate::commands::profile::rotate::duplicate::park(rs, &mut j, proof) {
            // A parking intent can already own a moved credential: recovery decides.
            if j.parking.is_some() {
                return Err(e);
            }
            return Err(abandon(store, rs, home, &mut j, e));
        }
        // Claim observations made before parking may include the moved file.
        auth = authorize(
            store,
            rs,
            &from,
            to,
            &dot,
            &to_dir,
            platform,
            platform_out,
            reader,
            Proof::Live,
        )?;
        crate::commands::profile::rotate::swap_authorize::check_snapshot(&auth, basis, to)?;
    }
    if let crate::commands::profile::rotate::duplicate::Slot::Conflict(proof) = &slot {
        // Non-blocking locks always follow live, target, original outgoing.
        let Source::Profile(origin) = &from else {
            return Err(refused(UNAVAILABLE));
        };
        j.from_lock_ino = Some(take_lock(&store.verify_profile_dir(origin)?)?);
        rs.save_journal(&j)?;
        if !proof.permits(&store.verify_profile_dir(origin)?) || !auth.current(store) {
            return Err(abandon(store, rs, home, &mut j, refused(UNAVAILABLE)));
        }
        crate::commands::profile::rotate::custody::reserve(store, rs, &mut j, proof)?;
    }
    let pending = std::mem::replace(
        &mut auth.pending,
        crate::commands::profile::rotate::swap_expect::Pending::Nothing,
    );
    let prepared = (|| {
        let home_str = home.to_str().ok_or_else(|| refused("HOME is not UTF-8"))?;
        let (Some(live_ino), Some(target_ino)) = (ino_at(&l), ino_at(&t)) else {
            return Err(refused(LOGIN_NOT_MOVABLE));
        };
        // The one write before J0, after the last check that refuses with
        // nothing written. `Profile` and `Adopt` exclude each other, so the
        // adoption below never follows a relabel.
        if j.outgoing.is_none() {
            relabel(store, &from, &pending)?;
        }
        let (from, adopt) = match from {
            Source::Profile(name) => (name, false),
            Source::Adopt => (adopt(store, auth.outgoing.account())?, true),
        };
        let from_directory = if adopt {
            let m = fs::symlink_metadata(store.verify_profile_dir(&from)?)?;
            Some((m.dev(), m.ino()))
        } else {
            j.from_directory
        };
        let j = RotateJournal {
            from: from.to_string(),
            to: to.to_string(),
            adopt,
            from_directory,
            phase: Phase::J0,
            home: home_str.to_string(),
            live_ino,
            target_ino,
            identity: Some(
                crate::commands::profile::rotate::login_identity::SwapIdentity::new(
                    auth.outgoing.account(),
                    auth.incoming.account(),
                ),
            ),
            ..j.clone()
        };
        Ok((from, j))
    })();
    let (from, mut j) = match prepared {
        Ok(v) => v,
        Err(e) => {
            return Err(abandon(store, rs, home, &mut j, e));
        }
    };
    if let Err(e) = rs.save_journal(&j) {
        release_locks(store, home, &j);
        remove_unused_adoption(store, &j);
        return Err(e);
    }
    let (live_ino, target_ino) = (j.live_ino, j.target_ino);
    let introduced = j
        .outgoing
        .as_ref()
        .map(|c| ProfileName::parse(&c.destination))
        .transpose()?;
    // Recheck every material ownership dependency immediately before movement.
    if ino_at(&t) != Some(target_ino)
        || !source_current(store, &j)
        || ino_at(&l) != Some(live_ino)
        || !auth.current_ignoring(store, introduced.as_ref().or(j.adopt.then_some(&from)))
        || crate::commands::profile::rotate::custody::has_login(store, &j)?
        || j.outgoing
            .as_ref()
            .is_some_and(|c| !c.permits(store, &auth.outgoing))
    {
        return Err(abandon(store, rs, home, &mut j, refused(LOGIN_NOT_MOVABLE)));
    }
    if !may_go_live(&t, platform) {
        return Err(abandon(store, rs, home, &mut j, no_oauth(to)));
    }
    abort_point("rotate-J0");
    if j.outgoing.is_some() {
        abort_point("outgoing-intent");
    }

    crate::commands::profile::rotate::custody::destination(store, &j)?;
    let destination = store.root().join(
        j.outgoing
            .as_ref()
            .map_or(j.from.as_str(), |c| c.destination.as_str()),
    );
    let saved_dir = super::super::verified_dirs::Directory::open(
        &destination,
        j.outgoing
            .as_ref()
            .map_or(j.from_directory, |c| c.directory()),
    )?;
    // An error after the syscall can mean movement occurred. Preserve J0;
    // recovery inspects placement and never blindly repeats an exchange.
    target_dir.exchange(&live_dir, target_ino, live_ino)?;
    super::super::verified_dirs::durable(&[&live_dir, &target_dir, &saved_dir])?;
    step(rs, &mut j, Phase::S1)?;
    if let Err(e) = target_dir.move_to(&saved_dir, live_ino) {
        return Err(ProfileError::RotateInconsistent(format!(
            "the outgoing login could not be moved from {to} to {from}; journal retained: {e}"
        )));
    }
    super::super::verified_dirs::durable(&[&live_dir, &target_dir, &saved_dir])?;
    if let Some(c) = &j.outgoing {
        abort_point("outgoing-moved");
        c.commit(store, rs)?;
        c.record_account(
            store,
            auth.outgoing
                .account()
                .ok_or_else(|| refused(UNAVAILABLE))?,
        )?;
    }
    step(rs, &mut j, Phase::S2)?;
    let incoming = auth
        .incoming
        .moved_to(&dot)
        .map_err(|_| ProfileError::RotateInconsistent(UNAVAILABLE.into()))?;
    crate::commands::profile::rotate::swap_authorize::check_record(store, to, incoming.account())
        .map_err(|_| ProfileError::RotateInconsistent(UNAVAILABLE.into()))?;
    finish(rs, home, &mut j, &incoming)?;
    if j.outgoing.is_none() && j.completion.is_some() {
        crate::commands::profile::rotate::swap_expect::settle_durable(store, &from, pending)?;
    } else if j.outgoing.is_none() {
        settle(store, &from, pending);
    }
    if let Some(completion) = &mut j.completion {
        completion.placed(clock());
        rs.save_journal(&j)?;
    }
    drop_locks(store, rs, home, &mut j)?;
    if j.completion.is_none() {
        rs.delete_journal()?;
    }
    Ok(from)
}
