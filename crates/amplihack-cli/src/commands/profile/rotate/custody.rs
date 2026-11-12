//! Private, discoverable outgoing custody. Receipts describe provenance;
//! only fresh server evidence can authorise movement.
use super::duplicate::ConflictProof;
use super::ledger::{RotateJournal, RotateState};
use super::login_identity::{Identity, Stamp, UNAVAILABLE};
use super::swap_authorize::refused;
use crate::commands::profile::auto_switch::kit::{
    CREDS, abort_point, check_root_private, exists, fsync_dir, write_private_atomic,
};
use crate::commands::profile::{ProfileError, name::ProfileName, store::ProfileStore};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::{fs, path::PathBuf};

const PREFIX: &str = "recovered-";
const MARKER: &str = ".rotate-custody.json";

fn reserved(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with(PREFIX)
}

/// Selection state is not movement authority; every attempt verifies afresh.
pub(super) enum Strategy {
    Original,
    AfterConflictRefusal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Custody {
    version: u32,
    pub origin: String,
    pub destination: String,
    ownership: Identity,
    live: Stamp,
    directory: Option<(u64, u64)>,
}
impl Custody {
    pub(super) fn valid(&self) -> bool {
        self.version == 1
            && ProfileName::parse(&self.origin).is_ok()
            && ProfileName::parse(&self.destination).is_ok()
            && self
                .destination
                .strip_prefix(PREFIX)
                .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
            && !reserved(&self.origin)
            && self.ownership.valid()
    }
    fn receipt(&self) -> String {
        format!("custody-{}.json", self.destination)
    }
    pub(super) fn dir(&self, store: &ProfileStore) -> Result<PathBuf, ProfileError> {
        if !self.valid() {
            return Err(refused("invalid outgoing custody provenance"));
        }
        let dir = store.verify_profile_dir(&ProfileName::parse(&self.destination)?)?;
        let m = fs::symlink_metadata(&dir)?;
        if self.directory != Some((m.dev(), m.ino())) {
            return Err(refused(UNAVAILABLE));
        }
        Ok(dir)
    }
    pub(super) fn permits(
        &self,
        store: &ProfileStore,
        outgoing: &super::login_identity::Bound,
    ) -> bool {
        self.dir(store).is_ok()
            && outgoing.current()
            && self.live.moved(outgoing.directory())
            && outgoing.account().is_some_and(|a| self.ownership.names(a))
    }
    pub(super) fn commit(
        &self,
        store: &ProfileStore,
        rs: &RotateState,
    ) -> Result<(), ProfileError> {
        let dir = self.dir(store)?;
        if !self.live.moved(&dir) {
            return Err(refused("outgoing custody file changed"));
        }
        let raw = serde_json::to_vec_pretty(self)
            .map_err(|_| refused("outgoing receipt serialisation failed"))?;
        for (parent, name) in [(&rs.dir, self.receipt()), (&dir, MARKER.to_owned())] {
            match rs.read(&parent.join(&name), "outgoing custody receipt")? {
                Some(old) if old != raw => {
                    return Err(refused("outgoing custody receipt conflicts"));
                }
                Some(_) => {}
                None => write_private_atomic(parent, &name, &raw)?,
            }
        }
        Ok(())
    }
    pub(super) fn record_account(
        &self,
        store: &ProfileStore,
        account: &super::identify::Account,
    ) -> Result<(), ProfileError> {
        if !self.ownership.names(account) {
            return Err(refused(UNAVAILABLE));
        }
        let dir = self.dir(store)?;
        write_private_atomic(
            &dir,
            ".claude.json",
            &serde_json::to_vec(&serde_json::json!({"oauthAccount":account.block()}))
                .map_err(|e| ProfileError::Io(std::io::Error::other(e)))?,
        )?;
        let state = crate::commands::profile::auto_switch::kit::State::new(store.root())?;
        let name = ProfileName::parse(&self.destination)?;
        super::swap_authorize::check_record(store, &name, Some(account))?;
        crate::commands::profile::expected::record(&state, &name, account)
    }
    /// Remove only our still-empty reservation, never another writer's files.
    pub(super) fn rollback(&self, store: &ProfileStore) -> Result<(), ProfileError> {
        let dir = self.dir(store)?;
        if fs::read_dir(&dir)?.next().is_none() {
            fs::remove_dir(&dir)?;
            fsync_dir(store.root())?;
        } else {
            return Err(refused(
                "outgoing reservation is no longer empty; retained for recovery",
            ));
        }
        Ok(())
    }
}

pub(super) fn reserve(
    store: &ProfileStore,
    rs: &RotateState,
    j: &mut RotateJournal,
    proof: &ConflictProof,
) -> Result<(), ProfileError> {
    let account = proof.live().account().ok_or_else(|| refused(UNAVAILABLE))?;
    let custody = Custody {
        version: 1,
        origin: j.from.clone(),
        destination: format!("{PREFIX}{}", uuid::Uuid::new_v4()),
        ownership: Identity::new(account),
        live: proof.live().stamp(),
        directory: None,
    };
    if !custody.valid() {
        return Err(refused("cannot reserve nested recovery custody"));
    }
    j.version = 2;
    j.outgoing = Some(custody);
    rs.save_journal(j)?; // exclusive name is recorded before creation
    let c = j.outgoing.as_mut().unwrap();
    let dir = store.reserve_recovery(&ProfileName::parse(&c.destination)?)?;
    let m = fs::symlink_metadata(&dir)?;
    c.directory = Some((m.dev(), m.ino()));
    rs.save_journal(j)?;
    j.outgoing_lock_ino = Some(super::swap::take_lock(&dir)?);
    rs.save_journal(j)?;
    abort_point("outgoing-reserved");
    Ok(())
}

/// The prefix is reserved: absent or inconsistent provenance fails closed.
/// Discovery itself never proves that a credential belongs to an account.
pub(in crate::commands::profile) fn origin(
    store: &ProfileStore,
    name: &ProfileName,
) -> Result<Option<ProfileName>, ProfileError> {
    if !reserved(name.as_str()) {
        match fs::symlink_metadata(store.root().join(name.as_str()).join(MARKER)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
            Ok(_) => {} // A renamed recovery profile must still validate provenance.
        }
    }
    let rs = RotateState::new(store.root());
    check_root_private(
        rs.dir
            .parent()
            .ok_or_else(|| refused("invalid custody state directory"))?,
    )?;
    check_root_private(&rs.dir)?;
    let dir = store.verify_profile_dir(name)?;
    let raw = rs
        .read(&dir.join(MARKER), "outgoing custody provenance")?
        .ok_or_else(|| refused("recovery profile has no provenance"))?;
    let c: Custody =
        serde_json::from_slice(&raw).map_err(|_| refused("invalid recovery profile provenance"))?;
    if !c.valid()
        || c.destination != name.as_str()
        || c.dir(store)? != dir
        || rs
            .read(&rs.dir.join(c.receipt()), "outgoing custody receipt")?
            .as_deref()
            != Some(raw.as_slice())
    {
        return Err(refused("inconsistent recovery profile provenance"));
    }
    Ok(Some(ProfileName::parse(&c.origin)?))
}

/// Apply origin exclusions before any reader refreshes or probes an alias,
/// including an alias that is already live. Unknown provenance fails closed.
pub(in crate::commands::profile) fn extend_exclusions(
    root: &std::path::Path,
    mut names: Vec<ProfileName>,
) -> Result<Vec<ProfileName>, ProfileError> {
    let store = ProfileStore::new(root.to_path_buf());
    for raw in store.list()? {
        let name = ProfileName::parse(&raw)?;
        let excluded = match origin(&store, &name) {
            Ok(Some(origin)) => names.contains(&origin),
            Ok(None) => false,
            Err(_) => true,
        };
        if excluded && !names.contains(&name) {
            names.push(name);
        }
    }
    Ok(names)
}

pub(super) fn destination(
    store: &ProfileStore,
    j: &RotateJournal,
) -> Result<PathBuf, ProfileError> {
    match &j.outgoing {
        Some(c) => c.dir(store),
        None => store.verify_profile_dir(&ProfileName::parse(&j.from)?),
    }
}

pub(super) fn has_login(store: &ProfileStore, j: &RotateJournal) -> Result<bool, ProfileError> {
    Ok(exists(&destination(store, j)?.join(CREDS)))
}
