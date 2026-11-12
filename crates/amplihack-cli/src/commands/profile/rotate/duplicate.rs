//! Server-verified duplicate custody and move-only, journalled parking (#2242).
use super::ledger::{RotateJournal, RotateState};
use super::login_identity::{Bound, Identity, Reader, Stamp, UNAVAILABLE};
use super::swap::{Source, describe, take_lock};
use super::swap_authorize::{check_record, refused};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::{
    CREDS, abort_point, check_root_private, exists, fsync_dir, move_login, write_private_atomic,
};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

/// Only this module can construct permission to occupy an outgoing destination.
pub(super) struct DuplicateProof {
    live: Bound,
    duplicate: Bound,
    dir: PathBuf,
}
/// A different-account conflict grants custody of the live login only.
pub(super) struct ConflictProof {
    live: Bound,
    occupied: Bound,
    dir: PathBuf,
}
impl ConflictProof {
    pub(super) fn permits(&self, dir: &Path) -> bool {
        dir == self.dir && self.live.current() && self.occupied.current()
    }
    pub(super) fn live(&self) -> &Bound {
        &self.live
    }
    pub(super) fn occupied(&self) -> &Bound {
        &self.occupied
    }
}
pub(super) fn recovery_conflict(
    store: &ProfileStore,
    name: &ProfileName,
    outgoing: &Path,
    reader: Reader<'_>,
) -> Result<ConflictProof, ProfileError> {
    let dir = store.verify_profile_dir(name)?;
    let live = reader.server_bound(outgoing).map_err(refused)?;
    let occupied = reader.server_bound(&dir).map_err(refused)?;
    let (Some(a), Some(b)) = (live.account(), occupied.account()) else {
        return Err(refused(UNAVAILABLE));
    };
    if [a, b]
        .iter()
        .any(|a| a.block().get("accountUuid").is_none())
    {
        return Err(refused(
            "outgoing custody lacks stable server identity; retained for repair",
        ));
    }
    check_record(store, name, Some(a))?;
    let p = ConflictProof {
        live,
        occupied,
        dir,
    };
    if !p.permits(&p.dir) {
        return Err(refused(UNAVAILABLE));
    }
    Ok(p)
}
pub(super) enum Slot {
    Vacant,
    Duplicate(DuplicateProof),
    Conflict(ConflictProof),
}
impl DuplicateProof {
    pub fn permits(&self, dir: &Path) -> bool {
        dir == self.dir && self.live.current() && self.duplicate.current()
    }
}

/// The receipt is also the intent persisted before the first rename.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Parking {
    version: u32,
    pub origin: String,
    pub parked: String,
    ownership: Identity,
    live: Stamp,
    duplicate: Stamp,
    directory: (u64, u64),
    origin_directory: (u64, u64),
}

pub(super) fn verify(
    store: &ProfileStore,
    rs: &RotateState,
    home: &Path,
    from: &Source,
    reader: Reader<'_>,
) -> Result<Slot, ProfileError> {
    let Source::Profile(name) = from else {
        return Ok(Slot::Vacant);
    };
    let dir = store.verify_profile_dir(name)?;
    if !exists(&dir.join(CREDS)) {
        super::login_state::clear_conflict(rs, name)?;
        return Ok(Slot::Vacant);
    }
    let read = |d: &Path| {
        reader
            .server_bound(d)
            .map_err(|c| refused(format!("duplicate identity: {c}")))
    };
    let live = read(&home.join(".claude"))?;
    let duplicate = read(&dir)?;
    let (Some(a), Some(b)) = (live.account(), duplicate.account()) else {
        return Err(refused(UNAVAILABLE));
    };
    if [a, b]
        .iter()
        .any(|a| a.block().get("accountUuid").is_none())
    {
        return Err(refused(
            "duplicate identity lacks a stable server account identifier; both logins preserved",
        ));
    }
    if !a.same(b) || a.org() != b.org() {
        if !live.current() || !duplicate.current() {
            return Err(refused(UNAVAILABLE));
        }
        check_record(store, name, Some(a))?;
        super::login_state::conflict(rs, name, a, b, &live.stamp(), &duplicate.stamp())?;
        return Ok(Slot::Conflict(ConflictProof {
            live,
            occupied: duplicate,
            dir,
        }));
    }
    check_record(store, name, Some(a))?;
    super::login_state::clear_conflict(rs, name)?;
    let proof = DuplicateProof {
        live,
        duplicate,
        dir,
    };
    if !proof.permits(&proof.dir) {
        return Err(refused(UNAVAILABLE));
    }
    Ok(Slot::Duplicate(proof))
}

pub(super) fn park(
    rs: &RotateState,
    j: &mut RotateJournal,
    proof: &DuplicateProof,
) -> Result<(), ProfileError> {
    // The outgoing slot has its own refresh lock, independent of live and target.
    j.from_lock_ino = Some(take_lock(&proof.dir)?);
    j.version = 2;
    rs.save_journal(j)?;
    if !proof.permits(&proof.dir) {
        return Err(refused(UNAVAILABLE));
    }
    rs.ensure()?;
    let id = format!("duplicate-{}", uuid::Uuid::new_v4());
    let parked = rs.dir.join(&id);
    // Exclusive creation: an existing path is never reused or replaced.
    fs::DirBuilder::new().mode(0o700).create(&parked)?;
    fsync_dir(&rs.dir)?;
    let m = fs::symlink_metadata(&parked)?;
    let origin = fs::symlink_metadata(&proof.dir)?;
    if m.dev() != origin.dev() {
        return Err(refused(super::swap::DIFFERENT_DEVICES));
    }
    j.parking = Some(Parking {
        version: 1,
        origin: j.from.clone(),
        parked: id,
        ownership: Identity::new(
            proof
                .duplicate
                .account()
                .ok_or_else(|| refused(UNAVAILABLE))?,
        ),
        live: proof.live.stamp(),
        duplicate: proof.duplicate.stamp(),
        directory: (m.dev(), m.ino()),
        origin_directory: (origin.dev(), origin.ino()),
    });
    rs.save_journal(j)?;
    abort_point("duplicate-intent");
    let p = j.parking.as_ref().unwrap();
    p.complete(rs, &proof.dir, true, Some(proof))
}

impl Parking {
    pub(super) fn valid(&self) -> bool {
        self.version == 1
            && ProfileName::parse(&self.origin).is_ok()
            && self.ownership.valid()
            && self.parked.starts_with("duplicate-")
            && self.parked.len() <= 80
            && self
                .parked
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }
    fn complete(
        &self,
        rs: &RotateState,
        origin: &Path,
        may_move: bool,
        authority: Option<&DuplicateProof>,
    ) -> Result<(), ProfileError> {
        let fail = || {
            ProfileError::RotateInconsistent(
                "duplicate parking custody is uncertain; retained journal and credentials".into(),
            )
        };
        if !self.valid() {
            return Err(fail());
        }
        check_root_private(&rs.dir)?;
        check_root_private(origin)?;
        let parked = rs.dir.join(&self.parked);
        check_root_private(&parked)?;
        let m = fs::symlink_metadata(&parked)?;
        let o = fs::symlink_metadata(origin)?;
        if self.directory != (m.dev(), m.ino())
            || self.origin_directory != (o.dev(), o.ino())
            || m.mode() & 0o077 != 0
        {
            return Err(fail());
        }
        let (src, dst) = (origin.join(CREDS), parked.join(CREDS));
        if may_move && exists(&dst) && exists(&src) {
            return Err(fail());
        }
        if !exists(&dst) {
            if !may_move
                || !self.duplicate.current(origin)
                || !authority.is_some_and(|p| p.permits(origin))
            {
                return Err(fail());
            }
            move_login(&src, &dst, &super::swap::show).map_err(|e| {
                ProfileError::RotateInconsistent(format!("duplicate parking: {}", describe(e)))
            })?;
            abort_point("duplicate-moved");
        }
        if !self.duplicate.moved(&parked) {
            return Err(fail());
        }
        fsync_dir(origin)?;
        fsync_dir(&parked)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| fail())?;
        let receipt = parked.join("receipt.json");
        match rs.read(&receipt, "duplicate receipt")? {
            Some(raw) if raw != bytes => return Err(fail()),
            Some(_) => {}
            None => write_private_atomic(&parked, "receipt.json", &bytes)?,
        }
        abort_point("duplicate-receipt");
        Ok(())
    }
    /// Inspect parking before checking-phase rollback. A rename may have completed
    /// even when its directory sync failed; placement, never the error, decides.
    pub(super) fn recover(
        &self,
        rs: &RotateState,
        store: &ProfileStore,
        home: &Path,
        checking: bool,
        reader: Reader<'_>,
    ) -> Result<(), ProfileError> {
        let name = ProfileName::parse(&self.origin)?;
        let origin = store.verify_profile_dir(&name)?;
        if checking && !self.live.current(&home.join(".claude")) {
            return Err(ProfileError::RotateInconsistent(UNAVAILABLE.into()));
        }
        let authority = if checking && !exists(&rs.dir.join(&self.parked).join(CREDS)) {
            let live = reader
                .server_bound(&home.join(".claude"))
                .map_err(refused)?;
            let duplicate = reader.server_bound(&origin).map_err(refused)?;
            if !live.current()
                || !duplicate.current()
                || !self.live.current(&home.join(".claude"))
                || !self.duplicate.current(&origin)
                || !live.account().is_some_and(|a| self.ownership.names(a))
                || !duplicate.account().is_some_and(|a| self.ownership.names(a))
            {
                return Err(refused(UNAVAILABLE));
            }
            Some(DuplicateProof {
                live,
                duplicate,
                dir: origin.clone(),
            })
        } else {
            None
        };
        self.complete(rs, &origin, checking, authority.as_ref())
    }
}
