//! Persistent authentication episodes and conflict alerts, under the state lock.
use super::identify::Account;
use super::ledger::RotateState;
use super::log::{Sink, open_or_warn, timestamp};
use super::login_identity::Identity;
use crate::commands::profile::auto_switch::kit::{CREDS, State, abort_point, exists};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_direct::{self, Authentication, Endpoints, Request};
use crate::commands::profile::{ProfileError, default_live, kind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

type FileEvidence = (u64, u64, u64, i64, i64, i64, i64, u32, u32, u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    dir: PathBuf,
    directory: (u64, u64, u32, u32),
    file: Option<FileEvidence>,
}
fn evidence(dir: &Path) -> Result<Evidence, ProfileError> {
    let d = fs::symlink_metadata(dir)?;
    if !d.is_dir() || d.uid() != crate::commands::profile::auto_switch::kit::euid() {
        return Err(bad());
    }
    let file = match fs::symlink_metadata(dir.join(CREDS)) {
        Ok(m) if m.is_file() => Some((
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
            m.uid(),
            m.mode(),
            m.nlink(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        _ => return Err(bad()),
    };
    Ok(Evidence {
        dir: dir.to_path_buf(),
        directory: (d.dev(), d.ino(), d.uid(), d.mode()),
        file,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    first_seen: i64,
    evidence: Evidence,
    alert_id: String,
    alerted: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Conflict {
    live: Identity,
    occupied: Identity,
    live_file: super::login_identity::Stamp,
    occupied_file: super::login_identity::Stamp,
    alert_id: String,
    alerted: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Health {
    version: u32,
    profiles: BTreeMap<String, Entry>,
    #[serde(default)]
    conflicts: BTreeMap<String, Conflict>,
}
impl Default for Health {
    fn default() -> Self {
        Self {
            version: 1,
            profiles: BTreeMap::new(),
            conflicts: BTreeMap::new(),
        }
    }
}
fn bad() -> ProfileError {
    ProfileError::RotateInconsistent(
        "needs-login state unreadable or unsafe; preserved for repair".into(),
    )
}
fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok()
}
fn load(rs: &RotateState) -> Result<Health, ProfileError> {
    for dir in [rs.dir.parent().ok_or_else(bad)?, rs.dir.as_path()] {
        match fs::symlink_metadata(dir) {
            Ok(_) => crate::commands::profile::auto_switch::kit::check_root_private(dir)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let Some(raw) = rs.read(&rs.dir.join("login-state.json"), "needs-login state")? else {
        return Ok(Health::default());
    };
    let s: Health = serde_json::from_slice(&raw).map_err(|_| bad())?;
    if s.version != 1
        || s.profiles.len() > 128
        || s.conflicts.len() > 64
        || s.profiles.iter().any(|(n, e)| {
            ProfileName::parse(n).is_err() || e.first_seen <= 0 || !valid_id(&e.alert_id)
        })
        || s.conflicts.iter().any(|(n, c)| {
            ProfileName::parse(n).is_err()
                || !c.live.valid()
                || !c.occupied.valid()
                || !valid_id(&c.alert_id)
        })
    {
        return Err(bad());
    }
    Ok(s)
}
fn save(rs: &RotateState, s: &Health) -> Result<(), ProfileError> {
    let raw = serde_json::to_vec_pretty(s).map_err(|_| bad())?;
    if raw.len() > 64 * 1024 || s.profiles.len() > 128 || s.conflicts.len() > 64 {
        return Err(bad());
    }
    rs.write("login-state.json", &raw)
}
fn deliver(rs: &RotateState, id: &str, line: &str) -> Result<(), ProfileError> {
    let log = open_or_warn(&rs.log, &rs.log1).ok_or_else(bad)?;
    let mut sink = Sink::new(Some(log), false, true);
    sink.keyed(crate::commands::profile::auto_switch::kit::now(), id, line)?;
    Ok(())
}

/// Unchanged episodes are checked before any identity, usage or child request.
/// This function never creates ownership evidence or grants movement permission.
pub(in crate::commands::profile) fn suppressed(
    root: &Path,
    name: &ProfileName,
    dir: &Path,
) -> Result<bool, ProfileError> {
    let _ = dir;
    Ok(load(&RotateState::new(root))?
        .profiles
        .contains_key(name.as_str()))
}

pub(super) fn suppressed_dirs(root: &Path, home: &Path) -> Result<Vec<PathBuf>, ProfileError> {
    let s = load(&RotateState::new(root))?;
    let store = ProfileStore::new(root.to_path_buf());
    if s.profiles.iter().any(|(n, e)| {
        e.evidence.dir != root.join(n)
            && e.evidence.dir != home.join(".claude")
            && !ProfileName::parse(n)
                .ok()
                .and_then(|name| store.verify_profile_dir(&name).ok())
                .is_some_and(|dir| dir == e.evidence.dir)
    }) {
        return Err(bad());
    }
    Ok(s.profiles
        .values()
        .filter(|e| evidence(&e.evidence.dir).is_ok_and(|now| now == e.evidence))
        .map(|e| e.evidence.dir.clone())
        .collect())
}

/// Called with the existing state lock, after journal recovery and before reads.
pub(super) fn assess(store: &ProfileStore, home: &Path, t: i64) -> Result<(), ProfileError> {
    let budget = super::login_identity::Budget::new();
    let read = super::login_identity::default_read();
    assess_with(
        store,
        home,
        t,
        super::login_identity::Reader {
            read: &read,
            budget: &budget,
        },
    )
}

pub(super) fn assess_with(
    store: &ProfileStore,
    home: &Path,
    t: i64,
    reader: super::login_identity::Reader<'_>,
) -> Result<(), ProfileError> {
    let rs = RotateState::new(store.root());
    let mut s = load(&rs)?;
    let prior = s.clone();
    s.conflicts.retain(|name, _| {
        // Absence is a resolved destination conflict even on a stay cycle.
        match fs::symlink_metadata(store.root().join(name).join(CREDS)) {
            Ok(_) => true,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        }
    });
    let state = State::new(store.root())?;
    let ledger = state.load_ledger()?;
    let live = rs
        .load_ledger()?
        .and_then(|l| l.live)
        .and_then(|n| ProfileName::parse(&n).ok())
        .or_else(|| default_live::live_name(store, Some(home)));
    let excluded = crate::commands::profile::usage_cmd::exclude_list(store.root(), |_| {})?;
    let kinds = kind::Ctx::load(store, Some(home));
    let (ep, _) = Endpoints::from_env();
    let names = store.list()?;
    s.profiles.retain(|name, _| names.contains(name));
    for raw in names {
        let name = ProfileName::parse(&raw)?;
        let origin = match super::custody::origin(store, &name) {
            Ok(origin) => origin,
            Err(_) => continue,
        };
        // Respect effective custody. An empty live, leased or hosted slot is
        // not evidence of a missing login; no request uses those empty slots.
        if excluded.contains(&name)
            || origin.as_ref().is_some_and(|o| excluded.contains(o))
            || ledger.leased_to(&raw).is_some()
            || ledger.hosts.contains_key(&raw)
            || exists(&store.root().join(".auto-switch.d/parked").join(&raw))
            || kinds.resolve_kind(store, &name).kind == kind::Kind::Platform
        {
            continue;
        }
        let dir = if live.as_ref() == Some(&name) {
            home.join(".claude")
        } else {
            store.verify_profile_dir(&name)?
        };
        let before = match evidence(&dir) {
            Ok(evidence) => evidence,
            Err(error) => {
                eprintln!("profile {name}: authentication assessment skipped: {error}");
                continue;
            }
        };
        if s.profiles.get(&raw).is_some_and(|e| e.evidence == before) {
            continue;
        }
        let journals = [state.dir.join("journal.json"), rs.journal.clone()];
        let req = Request {
            creds_dir: &dir,
            journals: &journals,
            // Preserve the established unit-fixture rule: fake expired
            // credentials never reach a real endpoint. Command tests exercise
            // refresh against their explicitly isolated loopback server.
            allow_refresh: !cfg!(test),
            cancel: &crate::commands::profile::usage_cmd::PROBE_CANCEL,
            timeout: Duration::from_secs(10),
            now_ms: t.saturating_mul(1000),
        };
        match usage_direct::assess_authentication(&ep, &req) {
            Authentication::Usable { refreshed } => {
                if !refreshed && evidence(&dir)? != before {
                    return Err(bad());
                }
                if s.profiles.contains_key(&raw) {
                    let verified = reader.server_bound(&dir);
                    match verified {
                        Ok(bound)
                            if bound.current()
                                && super::swap_authorize::check_record(
                                    store,
                                    &name,
                                    bound.account(),
                                )
                                .is_ok() =>
                        {
                            s.profiles.remove(&raw);
                        }
                        Ok(bound) if bound.current() => {
                            // An authenticated different account cannot repair
                            // this profile's episode or replace its ownership.
                            s.profiles.get_mut(&raw).ok_or_else(bad)?.evidence = evidence(&dir)?;
                        }
                        Err(
                            usage_direct::reason::HTTP_401
                            | usage_direct::reason::HTTP_4XX
                            | usage_direct::reason::HTTP_403,
                        ) => {
                            // A definitive rejection suppresses this changed binding;
                            // the episode and its original alert remain intact.
                            if evidence(&dir)? != before && !refreshed {
                                return Err(bad());
                            }
                            s.profiles.get_mut(&raw).ok_or_else(bad)?.evidence = evidence(&dir)?;
                        }
                        _ => eprintln!(
                            "profile {name}: needs-login repair awaits trusted identity verification"
                        ),
                    }
                }
            }
            Authentication::NeedsLogin => {
                // No successful refresh: a substitution invalidates this conclusion.
                if evidence(&dir)? != before {
                    return Err(bad());
                }
                if let Some(entry) = s.profiles.get_mut(&raw) {
                    entry.evidence = before;
                } else {
                    s.profiles.insert(
                        raw,
                        Entry {
                            first_seen: t,
                            evidence: before,
                            alert_id: uuid::Uuid::new_v4().to_string(),
                            alerted: false,
                        },
                    );
                }
            }
            Authentication::Uncertain(cause) => {
                // Retain the last conclusively assessed binding, so changed
                // credentials remain pending on the next existing cycle.
                eprintln!("profile {name}: authentication reassessment uncertain ({cause})");
            }
        }
    }
    if s != prior {
        save(&rs, &s)?;
    }
    let pending: Vec<_> = s
        .profiles
        .iter()
        .filter(|(_, e)| !e.alerted)
        .map(|(n, e)| (n.clone(), e.alert_id.clone(), e.first_seen))
        .collect();
    for (name, id, first) in pending {
        abort_point("login-alert-pending");
        deliver(
            &rs,
            &id,
            &format!(
                "ALERT: needs-login {name} (first seen {}); repair with profile login, or claude /login for the live login",
                timestamp(first)
            ),
        )?;
        abort_point("login-alert-delivered");
        s.profiles.get_mut(&name).ok_or_else(bad)?.alerted = true;
        save(&rs, &s)?;
    }
    Ok(())
}

#[path = "login_alerts.rs"]
mod alerts;
pub(super) use alerts::{clear_conflict, conflict};

/// One sorted persisted summary, shared by both commands. No authentication.
pub(in crate::commands::profile) fn line(root: &Path) -> String {
    match load(&RotateState::new(root)) {
        Err(_) => "needs-login: unknown (state unreadable; preserved for repair)".into(),
        Ok(s) if s.profiles.is_empty() => "needs-login: none".into(),
        Ok(s) => format!(
            "needs-login: {}; stored login: amplihack profile login <name>; live login: claude /login",
            s.profiles
                .iter()
                .map(|(n, e)| format!("{n} (first seen {})", timestamp(e.first_seen)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// A usage command reassesses episodes once, before its parallel row readers.
pub(in crate::commands::profile) fn assess_usage(
    store: &ProfileStore,
    home: &Path,
) -> Result<(), ProfileError> {
    let state = State::new(store.root())?;
    let Some(_lock) = state.try_lock(Duration::ZERO)? else {
        return Ok(());
    };
    let rs = RotateState::new(store.root());
    if exists(&rs.journal) || exists(&state.dir.join("journal.json")) {
        return Ok(());
    }
    assess(
        store,
        home,
        crate::commands::profile::auto_switch::kit::now(),
    )
}
