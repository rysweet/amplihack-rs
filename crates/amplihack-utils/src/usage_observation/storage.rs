use super::types::*;
use crate::secure_files::invalid;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
#[path = "directory.rs"]
pub(super) mod directory;
use directory::Directory;

const MAX_METADATA: usize = 4 * 1024 * 1024;
const MAX_CAPTURES: usize = 4096;
const MAX_SESSIONS: usize = 4096;

pub struct ObservationStore {
    root: PathBuf,
    state: Directory,
    records: Directory,
}
pub struct ObservationGuard<'a> {
    store: &'a ObservationStore,
    _lock: Option<File>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Stamp {
    dev: u64,
    ino: u64,
    size: u64,
    modified: i64,
    nanos: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginLocation {
    path: PathBuf,
    stamp: Option<Stamp>,
    #[serde(default)]
    movement: u64,
}
pub(super) fn stamp(path: &Path) -> io::Result<Option<Stamp>> {
    use std::os::unix::fs::MetadataExt;
    let m = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    crate::secure_files::check_private_file(&m)?;
    Ok(Some(Stamp {
        dev: m.dev(),
        ino: m.ino(),
        size: m.len(),
        modified: m.mtime(),
        nanos: m.mtime_nsec(),
    }))
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Login {
    binding: Option<Binding>,
    next: u64,
    accepted: Option<Record>,
    captures: BTreeMap<String, Capture>,
    #[serde(default)]
    location: Option<LoginLocation>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    schema_version: u32,
    logins: BTreeMap<String, Login>,
    sessions: BTreeMap<String, Binding>,
}

impl ObservationStore {
    pub fn open(root: &Path) -> io::Result<Self> {
        let dir = Directory::open(root)?;
        let state = dir.managed(".auto-switch.d")?;
        let records = state.managed("observations")?;
        Ok(Self {
            root: root.to_path_buf(),
            state,
            records,
        })
    }
    pub fn directory(&self) -> PathBuf {
        self.root.join(".auto-switch.d/observations")
    }
    pub fn record_path(&self, binding: &Binding) -> io::Result<PathBuf> {
        if !valid_binding(binding) {
            return Err(invalid("invalid observation binding"));
        }
        Ok(self.directory().join(format!("{}.json", binding.profile)))
    }
    pub fn lock(&self) -> io::Result<ObservationGuard<'_>> {
        let f = self.state.file("lock", true)?;
        loop {
            // SAFETY: f owns a live descriptor. Blocking flock has no timer.
            if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        Ok(ObservationGuard {
            store: self,
            _lock: Some(f),
        })
    }
    /// The caller must hold the existing `.auto-switch.d/lock` for this root
    /// until the guard is dropped. Never acquire a nested copy of that lock.
    pub fn under_state_lock(&self) -> ObservationGuard<'_> {
        ObservationGuard {
            store: self,
            _lock: None,
        }
    }
}
impl ObservationGuard<'_> {
    fn metadata(&self) -> io::Result<Metadata> {
        let Some(raw) = self
            .store
            .state
            .read("observation-order.json", MAX_METADATA)?
        else {
            return Ok(Metadata {
                schema_version: 1,
                ..Metadata::default()
            });
        };
        let mut meta: Metadata = serde_json::from_slice(&raw)
            .map_err(|_| invalid("invalid observation ordering metadata"))?;
        if meta.schema_version != 1
            || meta.logins.len() > MAX_SESSIONS
            || meta.sessions.len() > MAX_SESSIONS
            || meta.logins.iter().any(|(p, l)| {
                !profile(p)
                    || l.captures.len() > MAX_CAPTURES
                    || l.binding
                        .as_ref()
                        .is_some_and(|b| !valid_binding(b) || &b.profile != p)
            })
            || meta
                .sessions
                .iter()
                .any(|(id, b)| !identifier(id) || !valid_binding(b))
        {
            return Err(invalid(
                "unsupported or invalid observation ordering metadata",
            ));
        }
        for login in meta.logins.values_mut() {
            if let Some(record) = &mut login.accepted
                && record.schema_version == 1
                && record.evidence.session_used.is_some()
                && record.evidence.session_window == SessionWindow::Unknown
            {
                record.evidence.session_window = SessionWindow::Present;
            }
        }
        Ok(meta)
    }
    fn save(&self, meta: &Metadata) -> io::Result<()> {
        let raw = serde_json::to_vec(meta)?;
        if raw.len() > MAX_METADATA {
            return Err(invalid("observation metadata capacity exceeded"));
        }
        self.store.state.replace("observation-order.json", &raw)
    }
    pub fn authorised_login(&mut self, name: &str, account: &str) -> io::Result<Binding> {
        if !profile(name) || !identifier(account) {
            return Err(invalid("invalid login identity"));
        }
        let mut meta = self.metadata()?;
        if !meta.logins.contains_key(name) && meta.logins.len() >= MAX_SESSIONS {
            return Err(invalid("observation login capacity exceeded"));
        }
        let mut random = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let b = Binding {
            profile: name.into(),
            account_id: account.into(),
            generation: format!("{:032x}", u128::from_ne_bytes(random)),
        };
        meta.logins.insert(
            name.into(),
            Login {
                binding: Some(b.clone()),
                location: Some(LoginLocation {
                    path: self.store.root.join(name).join(".credentials.json"),
                    movement: 0,
                    stamp: stamp(&self.store.root.join(name).join(".credentials.json"))?,
                }),
                ..Login::default()
            },
        );
        self.save(&meta)?;
        Ok(b)
    }
    pub fn binding(&self, profile: &str) -> io::Result<Option<Binding>> {
        if !super::types::profile(profile) {
            return Err(invalid("invalid profile name"));
        }
        Ok(self
            .metadata()?
            .logins
            .get(profile)
            .and_then(|l| l.binding.clone()))
    }
    /// Resolve ownership only at session start from an authorised location.
    /// Canonicalise the parent directory, never open the credential contents.
    /// Ambiguous ownership remains unattributed rather than choosing a login.
    pub fn binding_at(&mut self, path: &Path) -> io::Result<Option<Binding>> {
        let parent = path.parent().map(Path::canonicalize).transpose()?;
        let candidates: Vec<_> = self
            .metadata()?
            .logins
            .into_values()
            .filter_map(|login| {
                let location = login.location?;
                let same = location.path == path
                    || (parent.is_some()
                        && location.path.file_name() == path.file_name()
                        && location.path.parent().and_then(|p| p.canonicalize().ok()) == parent);
                if same { login.binding } else { None }
            })
            .collect();
        let [binding] = candidates.as_slice() else {
            return Ok(None);
        };
        if self.retire_if_changed(binding)? {
            return Ok(None);
        }
        Ok(Some(binding.clone()))
    }
    /// A retired generation is distinct from pre-upgrade state. Recovery
    /// must never recreate a retired binding from an old expected record.
    pub fn registered(&self, profile: &str) -> io::Result<bool> {
        Ok(self.metadata()?.logins.contains_key(profile))
    }
    pub fn bindings(&self) -> io::Result<Vec<Binding>> {
        Ok(self
            .metadata()?
            .logins
            .values()
            .filter_map(|l| l.binding.clone())
            .collect())
    }
    /// Reconcile non-secret locations from one metadata snapshot. Unchanged
    /// inputs do not reparse the whole registry for each profile. Changed
    /// locations still use the same retirement/refresh validation under lock.
    pub fn reconcile_bindings(&mut self) -> io::Result<Vec<(Binding, Option<u64>)>> {
        let mut bindings = Vec::new();
        for login in self.metadata()?.logins.into_values() {
            let Some(binding) = login.binding.as_ref() else {
                continue;
            };
            let unchanged = match &login.location {
                None => true,
                Some(location) => match stamp(&location.path) {
                    Ok(current) => current == location.stamp,
                    Err(error) if error.kind() == io::ErrorKind::InvalidData => false,
                    Err(error) => return Err(error),
                },
            };
            if !unchanged && self.retire_if_changed(binding)? {
                continue;
            }
            let accepted = if unchanged {
                login.accepted.as_ref().map(|record| record.capture.ticket)
            } else {
                self.accepted_ticket(binding)?
            };
            bindings.push((binding.clone(), accepted));
        }
        Ok(bindings)
    }
    pub fn retire(&mut self, binding: &Binding) -> io::Result<()> {
        let mut meta = self.metadata()?;
        if let Some(login) = meta.logins.get_mut(&binding.profile)
            && login.binding.as_ref() == Some(binding)
        {
            *login = Login::default();
            self.save(&meta)?;
        }
        Ok(())
    }
    /// Authorised movement/refresh records the new metadata without changing
    /// identity or generation. This never opens the credential contents.
    pub fn set_location(&mut self, binding: &Binding, path: &Path) -> io::Result<()> {
        let mut meta = self.metadata()?;
        let login = meta
            .logins
            .get_mut(&binding.profile)
            .filter(|l| l.binding.as_ref() == Some(binding))
            .ok_or_else(|| invalid("cannot move retired usage generation"))?;
        let movement = match &login.location {
            Some(old) if old.path != path => old
                .movement
                .checked_add(1)
                .ok_or_else(|| invalid("login movement counter exhausted"))?,
            Some(old) => old.movement,
            None => 0,
        };
        let location = LoginLocation {
            path: path.to_path_buf(),
            stamp: stamp(path)?,
            movement,
        };
        if login
            .location
            .as_ref()
            .is_some_and(|l| l.path == location.path && l.stamp == location.stamp)
        {
            return Ok(());
        }
        login.location = Some(location);
        self.save(&meta)
    }
    /// Unexplained replacements invalidate evidence; metadata cannot prove
    /// the new credential's identity and can never authorise a replacement.
    pub fn retire_if_changed(&mut self, binding: &Binding) -> io::Result<bool> {
        let meta = self.metadata()?;
        let Some(login) = meta
            .logins
            .get(&binding.profile)
            .filter(|l| l.binding.as_ref() == Some(binding))
        else {
            return Ok(true);
        };
        if let Some(location) = &login.location {
            let current = match stamp(&location.path) {
                Ok(current) => current,
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                    self.retire(binding)?;
                    return Ok(true);
                }
                Err(e) => return Err(e),
            };
            if current != location.stamp {
                // A refresh in progress is unavailable, never stale evidence
                // for a movement. Its lock removal provides the next event.
                if location
                    .path
                    .parent()
                    .is_some_and(|p| p.join(".oauth_refresh.lock").exists())
                {
                    return Ok(true);
                }
                if super::refresh::confirms(&location.path, &location.stamp, &current)? {
                    self.set_location(binding, &location.path)?;
                } else {
                    self.retire(binding)?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    /// Preserve an established generation across an authorised rename.
    pub fn moved(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        let mut meta = self.metadata()?;
        let current = stamp(destination)?;
        let mut changed = false;
        for login in meta.logins.values_mut() {
            if let Some(location) = &mut login.location
                && location.path == source
                && location.stamp == current
            {
                location.movement = location
                    .movement
                    .checked_add(1)
                    .ok_or_else(|| invalid("login movement counter exhausted"))?;
                location.path = destination.to_path_buf();
                changed = true;
            }
        }
        if changed {
            self.save(&meta)?;
        }
        Ok(())
    }
    /// Capture non-secret filesystem identity for an unlocked probe. A
    /// measurement from a shared path is refused if a swap moved its owner
    /// while the probe was queued or collecting.
    pub fn probe_location(
        &mut self,
        binding: &Binding,
        path: &Path,
    ) -> io::Result<Option<LoginLocation>> {
        if self.retire_if_changed(binding)? {
            return Ok(None);
        }
        Ok(self
            .metadata()?
            .logins
            .get(&binding.profile)
            .filter(|l| l.binding.as_ref() == Some(binding))
            .and_then(|l| l.location.as_ref())
            .filter(|l| l.path == path)
            .cloned())
    }
    pub fn probe_location_unchanged(
        &mut self,
        binding: &Binding,
        location: &LoginLocation,
    ) -> io::Result<bool> {
        let Some(current) = self.probe_location(binding, &location.path)? else {
            return Ok(false);
        };
        if current.movement != location.movement {
            return Ok(false);
        }
        if current.stamp == location.stamp {
            return Ok(true);
        }
        super::refresh::confirms(&location.path, &location.stamp, &current.stamp)
    }
    /// Compare a newer publication by effective evidence while the caller
    /// holds the state lock. Ticket allocation alone is not a policy input.
    pub fn accepted_evidence(&self, binding: &Binding) -> io::Result<Option<Evidence>> {
        let meta = self.metadata()?;
        let record = meta
            .logins
            .get(&binding.profile)
            .filter(|login| login.binding.as_ref() == Some(binding))
            .and_then(|login| login.accepted.as_ref());
        match record {
            Some(record)
                if valid_capture(&record.capture)
                    && &record.capture.binding == binding
                    && record.revision > 0
                    && record.evidence.valid()
                    && [1, SCHEMA_VERSION].contains(&record.schema_version) =>
            {
                Ok(Some(record.evidence.clone()))
            }
            Some(_) => Err(invalid("invalid accepted observation evidence")),
            None => Ok(None),
        }
    }
    pub fn accepted_ticket(&self, binding: &Binding) -> io::Result<Option<u64>> {
        Ok(self
            .metadata()?
            .logins
            .get(&binding.profile)
            .filter(|l| l.binding.as_ref() == Some(binding))
            .and_then(|l| l.accepted.as_ref())
            .map(|r| r.capture.ticket))
    }
    pub fn capture(&mut self, binding: &Binding, id: &str, at: i64) -> io::Result<Capture> {
        if !valid_binding(binding)
            || !identifier(id)
            || id.starts_with("captured-")
            || !(0..=MAX_TIME).contains(&at)
        {
            return Err(invalid("invalid observation capture"));
        }
        let mut meta = self.metadata()?;
        let login = meta
            .logins
            .get_mut(&binding.profile)
            .filter(|l| l.binding.as_ref() == Some(binding))
            .ok_or_else(|| invalid("observation login generation is missing or retired"))?;
        if login.captures.contains_key(id) {
            return Err(invalid("observation identifier already captured"));
        }
        if login.captures.len() >= MAX_CAPTURES {
            return Err(invalid(
                "observation capture capacity exceeded; re-login required",
            ));
        }
        login.next = login
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("observation ticket exhausted"))?;
        let capture = Capture {
            binding: binding.clone(),
            observation_id: id.into(),
            ticket: login.next,
            captured_at: at,
        };
        login.captures.insert(id.into(), capture.clone());
        self.save(&meta)?;
        Ok(capture)
    }
    /// Redelivery reuses its original capture; this never allocates a new
    /// ticket for an existing observation identifier.
    /// Allocate a self-identifying capture for a new trusted event. Older
    /// generated captures can be compacted because their ticket is encoded
    /// in the reserved identifier namespace and can never be reallocated.
    pub fn capture_new(&mut self, binding: &Binding, at: i64) -> io::Result<Capture> {
        if !valid_binding(binding) || !(0..=MAX_TIME).contains(&at) {
            return Err(invalid("invalid capture"));
        }
        let mut meta = self.metadata()?;
        let login = meta
            .logins
            .get_mut(&binding.profile)
            .filter(|l| l.binding.as_ref() == Some(binding))
            .ok_or_else(|| invalid("observation generation is missing or retired"))?;
        if login.captures.len() >= MAX_CAPTURES {
            return Err(invalid("too many pending observation captures"));
        }
        login.next = login
            .next
            .checked_add(1)
            .ok_or_else(|| invalid("observation ticket exhausted"))?;
        let c = Capture {
            binding: binding.clone(),
            observation_id: format!("captured-{}-{}", binding.generation, login.next),
            ticket: login.next,
            captured_at: at,
        };
        login.captures.insert(c.observation_id.clone(), c.clone());
        self.save(&meta)?;
        Ok(c)
    }
    /// Release an unused generated capture. Its reserved ticket identifier
    /// remains non-reallocatable even after the pending entry is removed.
    pub fn abandon(&mut self, capture: &Capture) -> io::Result<()> {
        if !capture.observation_id.starts_with("captured-") {
            return Ok(());
        }
        let mut meta = self.metadata()?;
        if let Some(login) = meta.logins.get_mut(&capture.binding.profile)
            && login.binding.as_ref() == Some(&capture.binding)
            && !login
                .accepted
                .as_ref()
                .is_some_and(|r| r.capture == *capture)
            && login.captures.remove(&capture.observation_id).is_some()
        {
            self.save(&meta)?;
        }
        Ok(())
    }
    pub fn capture_once(&mut self, binding: &Binding, id: &str, at: i64) -> io::Result<Capture> {
        if let Some(capture) = self
            .metadata()?
            .logins
            .get(&binding.profile)
            .filter(|l| l.binding.as_ref() == Some(binding))
            .and_then(|l| l.captures.get(id))
            .cloned()
        {
            return Ok(capture);
        }
        self.capture(binding, id, at)
    }
    pub fn publish(
        &mut self,
        capture: &Capture,
        evidence: &Evidence,
    ) -> io::Result<PublishOutcome> {
        use PublishOutcome::*;
        if !valid_capture(capture) {
            return Ok(InvalidAttribution);
        }
        if !evidence.valid() {
            return Ok(InvalidEvidence);
        }
        let identity = self.binding(&capture.binding.profile)?;
        if let Some(identity) = identity {
            if identity.account_id != capture.binding.account_id {
                return Ok(InvalidAttribution);
            }
            if identity != capture.binding {
                return Ok(RetiredGeneration);
            }
            if self.retire_if_changed(&capture.binding)? {
                return Ok(RetiredGeneration);
            }
        }
        let mut meta = self.metadata()?;
        let Some(login) = meta.logins.get_mut(&capture.binding.profile) else {
            return Ok(InvalidAttribution);
        };
        let Some(binding) = &login.binding else {
            return Ok(RetiredGeneration);
        };
        if binding.account_id != capture.binding.account_id {
            return Ok(InvalidAttribution);
        }
        if binding != &capture.binding {
            return Ok(RetiredGeneration);
        }
        if login
            .accepted
            .as_ref()
            .is_some_and(|r| capture.ticket < r.capture.ticket)
        {
            return Ok(Superseded);
        }
        if login.captures.get(&capture.observation_id) != Some(capture) {
            return Ok(InvalidAttribution);
        }
        let name = format!("{}.json", binding.profile);
        if let Err(e) = self.store.records.file(&name, false)
            && e.kind() != io::ErrorKind::NotFound
        {
            return Ok(Unsafe);
        }
        if let Some(accepted) = &login.accepted {
            if capture.ticket < accepted.capture.ticket {
                return Ok(Superseded);
            }
            if capture.ticket == accepted.capture.ticket {
                if evidence != &accepted.evidence {
                    return Ok(InvalidEvidence);
                }
                let raw = serde_json::to_vec(accepted)?;
                if self.store.records.read(&name, MAX_RECORD_BYTES)?.as_deref()
                    != Some(raw.as_slice())
                {
                    self.store.records.replace(&name, &raw)?;
                    return Ok(Accepted {
                        revision: accepted.revision,
                    });
                }
                return Ok(Duplicate);
            }
        }
        let revision = login.accepted.as_ref().map_or(Ok(1), |r| {
            r.revision
                .checked_add(1)
                .ok_or_else(|| invalid("observation revision exhausted"))
        })?;
        let record = Record {
            schema_version: SCHEMA_VERSION,
            capture: capture.clone(),
            evidence: evidence.clone(),
            revision,
        };
        let raw = serde_json::to_vec(&record)?;
        if raw.len() > MAX_RECORD_BYTES {
            return Ok(InvalidEvidence);
        }
        login.accepted = Some(record);
        login
            .captures
            .retain(|id, c| !id.starts_with("captured-") || c.ticket >= capture.ticket);
        // Commit ordering first. Recovery never exposes an older record if
        // interruption happens between these two durable atomic replacements.
        self.save(&meta)?;
        self.store.records.replace(&name, &raw)?;
        Ok(Accepted { revision })
    }
    pub fn read(&self, binding: &Binding) -> io::Result<ReadOutcome> {
        if !valid_binding(binding) {
            return Ok(ReadOutcome::Invalid);
        }
        let meta = self.metadata()?;
        let Some(login) = meta.logins.get(&binding.profile) else {
            return Ok(ReadOutcome::Missing);
        };
        if login.binding.as_ref() != Some(binding) {
            return Ok(ReadOutcome::StaleGeneration);
        }
        let file = match self
            .store
            .records
            .file(&format!("{}.json", binding.profile), false)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(ReadOutcome::Missing),
            Err(e)
                if e.kind() == io::ErrorKind::InvalidData
                    || matches!(e.raw_os_error(), Some(libc::ELOOP)) =>
            {
                return Ok(ReadOutcome::Unsafe);
            }
            Err(e) => return Err(e),
        };
        let outcome = read_bounded_record(file, binding)?;
        if let ReadOutcome::Usable(record) = &outcome
            && login.accepted.as_ref() != Some(record)
        {
            return Ok(ReadOutcome::Invalid);
        }
        Ok(outcome)
    }
    pub fn bind_session(&mut self, session: &str, binding: &Binding) -> io::Result<()> {
        if !identifier(session) || self.binding(&binding.profile)?.as_ref() != Some(binding) {
            return Err(invalid("cannot bind session to missing or retired login"));
        }
        let mut meta = self.metadata()?;
        if let Some(old) = meta.sessions.get(session) {
            return if old == binding {
                Ok(())
            } else {
                Err(invalid("session identity already captured"))
            };
        }
        if meta.sessions.len() >= MAX_SESSIONS {
            return Err(invalid("session attribution capacity exceeded"));
        }
        meta.sessions.insert(session.into(), binding.clone());
        self.save(&meta)
    }
    pub fn sessions(&self) -> io::Result<Vec<crate::session_attribution::SessionBinding>> {
        Ok(self
            .metadata()?
            .sessions
            .into_iter()
            .map(
                |(session_id, binding)| crate::session_attribution::SessionBinding {
                    session_id,
                    binding,
                },
            )
            .collect())
    }
}
pub fn read_bounded_record(reader: impl Read, binding: &Binding) -> io::Result<ReadOutcome> {
    let mut raw = Vec::new();
    reader
        .take(MAX_RECORD_BYTES as u64 + 1)
        .read_to_end(&mut raw)?;
    if raw.len() > MAX_RECORD_BYTES {
        return Ok(ReadOutcome::Invalid);
    }
    let value: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(v) => v,
        Err(_) => return Ok(ReadOutcome::Invalid),
    };
    if value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .is_some_and(|v| v != 1 && v != u64::from(SCHEMA_VERSION))
    {
        return Ok(ReadOutcome::UnsupportedSchema);
    }
    // Version 1 had no discriminator. Only a measured session can establish
    // presence; a missing session must never migrate to an absent limit.
    let mut value = value;
    if value.get("schema_version").and_then(|v| v.as_u64()) == Some(1)
        && let Some(evidence) = value.get_mut("evidence").and_then(|v| v.as_object_mut())
        && !evidence.contains_key("session_window")
        && evidence.get("session_used").is_some_and(|v| v.is_number())
    {
        evidence.insert("session_window".into(), serde_json::json!("present"));
    }
    let record: Record = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(_) => return Ok(ReadOutcome::Invalid),
    };
    if ![1, SCHEMA_VERSION].contains(&record.schema_version)
        || !valid_capture(&record.capture)
        || !record.evidence.valid()
        || record.revision == 0
    {
        return Ok(ReadOutcome::Invalid);
    }
    if &record.capture.binding != binding {
        return Ok(ReadOutcome::StaleGeneration);
    }
    Ok(ReadOutcome::Usable(record))
}
