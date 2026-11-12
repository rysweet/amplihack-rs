//! Read-only named destinations for a default directory hosting another login.
//! Selection uses the nested selector's policy. Credentials are neither moved
//! nor refreshed. Authenticated continuity evidence remains process-local.

use super::state::{CREDS, Ledger, State};
use super::{candidate, config, fsx, overflow_inputs::OverflowInputs, per_token, rerun};
use crate::commands::profile::launch_target::{JoinPolicy, LaunchTarget, SelectionOrigin};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::{ProfileError, expected, kind, rotate};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(super) fn invalid(reason: impl Into<String>) -> ProfileError {
    ProfileError::LaunchDestination(reason.into())
}

/// File metadata only: never a token, hash of a token or credential contents.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    owner: u32,
    group: u32,
    mode: u32,
    file: Option<(u64, i64, i64, i64, i64)>,
}

impl Stamp {
    fn read(path: &Path) -> Result<Option<Self>, ProfileError> {
        let m = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if m.file_type().is_symlink() || (!m.is_file() && !m.is_dir()) {
            return Err(invalid(
                "destination evidence is not a regular file or directory",
            ));
        }
        if m.uid() != fsx::euid() || m.mode() & 0o022 != 0 {
            return Err(invalid("destination evidence is not private"));
        }
        Ok(Some(Self {
            device: m.dev(),
            inode: m.ino(),
            owner: m.uid(),
            group: m.gid(),
            mode: m.mode(),
            // Directory times change when the refresh lock or shared links are created.
            file: m.is_file().then_some((
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )),
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PolicyEvidence {
    source_host: String,
    source_login: String,
    stamps: Vec<Option<Stamp>>,
    pool: Vec<(ProfileName, Vec<Option<Stamp>>)>,
    approved: Vec<ProfileName>,
}

#[derive(Clone)]
pub(in crate::commands::profile) struct Evidence {
    policy: PolicyEvidence,
    credential: rotate::login_identity::LaunchBound,
}
impl std::fmt::Debug for Evidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Evidence(<redacted>)")
    }
}
impl Evidence {
    pub(in crate::commands::profile) fn names_conflict(&self, host: &str, login: &str) -> bool {
        self.policy.source_host == host && self.policy.source_login == login
    }
    pub(in crate::commands::profile) fn own_oauth(&self) -> bool {
        self.credential.account().is_some()
    }
    pub(in crate::commands::profile) fn source_host(&self) -> &str {
        &self.policy.source_host
    }
}

fn paths(store: &ProfileStore, name: &ProfileName, source: &str) -> Vec<PathBuf> {
    let root = store.root();
    let dir = root.join(name.as_str());
    vec![
        root.to_path_buf(),
        dir.clone(),
        dir.join(CREDS),
        dir.join(".claude.json"),
        dir.join("settings.json"),
        root.join(".auto-switch"),
        root.join(".auto-switch.d/expected")
            .join(format!("{name}.json")),
        root.join(".auto-switch.d/expected")
            .join(format!("{source}.json")),
    ]
}

fn stamps(
    store: &ProfileStore,
    name: &ProfileName,
    source: &str,
) -> Result<Vec<Option<Stamp>>, ProfileError> {
    paths(store, name, source)
        .iter()
        .map(|p| Stamp::read(p))
        .collect()
}

// Retain every profile's local policy inputs, including unselected peers.
// Usage is deliberately re-read: new peer capacity must narrow permission.
fn pool_stamps(
    store: &ProfileStore,
    source: &str,
) -> Result<Vec<(ProfileName, Vec<Option<Stamp>>)>, ProfileError> {
    let cfg = config::read_editable(store.root())?;
    config::every_profile(store.root())?
        .into_iter()
        .map(|name| {
            // Excluded alternatives cannot affect policy. The original
            // source remains an affinity input even when excluded. The
            // selected proof also stamps changes to the exclusion file.
            let proof = if cfg.exclude.contains(&name) && name.as_str() != source {
                Vec::new()
            } else {
                stamps(store, &name, source)?
            };
            Ok((name, proof))
        })
        .collect()
}

/// Local continuity of an OAuth identity with its recorded owner and display.
/// The credential-bound server identity is obtained separately, before locks.
fn identity_matches(
    state: &State,
    name: &ProfileName,
    account: &rotate::identify::Account,
) -> bool {
    let display = rotate::identify::Account::of_dir(&state.profile_dir(name.as_str()));
    if !display.is_some_and(|d| d.same(account)) {
        return false;
    }
    match expected::lookup(state, name) {
        Ok(Some(record)) => record.names(account),
        Ok(None) => true,
        Err(()) => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn ready(
    store: &ProfileStore,
    state: &State,
    ledger: &Ledger,
    cfg: &config::Config,
    overflow: &OverflowInputs,
    source: &str,
    name: &ProfileName,
) -> bool {
    !cfg.exclude.contains(name)
        && name.as_str() != source
        && !ledger.hosts.contains_key(name.as_str())
        && ledger.leased_to(name.as_str()).is_none()
        && store.verify_profile_dir(name).is_ok()
        && rerun::login_ready(state, overflow, name)
        && (kind::launch_guard(store, name, None, || false).is_ok()
            // A missing file says nothing about macOS storage. This is only
            // provisional: authentication must produce an owned OAuth proof.
            || (cfg!(target_os = "macos") && !fsx::exists(&state.live_creds(name.as_str()))))
}

/// Pins a policy-approved named destination without changing any login.
/// Network identity work completes before the protected launch boundary.
fn authenticate(
    store: &ProfileStore,
    name: &ProfileName,
    source_host: &str,
) -> Result<Evidence, ProfileError> {
    let state = State::new(store.root())?;
    let ledger = state.load_ledger()?;
    let source_login = ledger.account_in(source_host).to_string();
    let before = stamps(store, name, &source_login)?;
    let pool = pool_stamps(store, &source_login)?;
    let budget = rotate::login_identity::Budget::new();
    let read = rotate::login_identity::default_read();
    let reader = rotate::login_identity::Reader {
        read: &read,
        budget: &budget,
    };
    let kinds = kind::Ctx::load(store, None);
    let platform = kinds.kind(store, name) == kind::Kind::Platform;
    let bound =
        rotate::login_identity::launch_bound(&reader, &store.verify_profile_dir(name)?, platform)
            .map_err(invalid)?;
    if !bound.current()
        || bound
            .account()
            .is_some_and(|a| !identity_matches(&state, name, a))
        || before != stamps(store, name, &source_login)?
        || pool != pool_stamps(store, &source_login)?
    {
        return Err(invalid(
            "destination login identity changed or does not match its profile",
        ));
    }
    Ok(Evidence {
        policy: PolicyEvidence {
            source_host: source_host.to_string(),
            source_login,
            stamps: before,
            pool,
            approved: Vec::new(),
        },
        credential: bound,
    })
}

/// Authenticate the eligible pool before taking launch locks. Refused peers
/// remain visible to policy, but are never offered as launch destinations.
fn verified_pool(
    store: &ProfileStore,
    source: &str,
) -> Result<(Vec<(ProfileName, Evidence)>, Option<ProfileError>), ProfileError> {
    let state = State::new(store.root())?;
    let cfg = config::read_editable(store.root())?;
    let ledger = state.load_ledger()?;
    let overflow = OverflowInputs::load_at(store.root(), &cfg, fsx::now(), None);
    if overflow.kinds_unknown {
        return Err(invalid("fallback profile kinds cannot be read"));
    }
    let mut targets = Vec::new();
    let mut refusal = None;
    for name in config::every_profile(store.root())? {
        if ready(store, &state, &ledger, &cfg, &overflow, source, &name) {
            match authenticate(store, &name, source) {
                Ok(proof) => targets.push((name, proof)),
                Err(error) => {
                    tracing::debug!(profile = %name, error = %error, "fallback destination refused");
                    refusal = Some(error);
                }
            }
        }
    }
    let approved: Vec<_> = targets.iter().map(|(name, _)| name.clone()).collect();
    for (_, proof) in &mut targets {
        proof.policy.approved = approved.clone();
    }
    Ok((targets, refusal))
}

/// A supervisor transports intent, so the actual launching process obtains
/// its own evidence for the destination and the relevant candidate pool.
pub(in crate::commands::profile) fn pin(
    store: &ProfileStore,
    name: &ProfileName,
    source: &str,
) -> Result<Evidence, ProfileError> {
    let (targets, refusal) = verified_pool(store, source)?;
    targets
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, proof)| proof)
        .ok_or_else(|| refusal.unwrap_or_else(|| invalid("destination is no longer eligible")))
}

/// Select provisionally; the selected evidence and policy are checked again
/// under the launch hold. No lease is acquired and the saved default is intact.
pub(in crate::commands::profile) fn select(
    store: &ProfileStore,
    conflict: &ProfileName,
    hosted: &str,
) -> Result<Option<LaunchTarget>, ProfileError> {
    let state = State::new(store.root())?;
    let cfg = config::read_editable(store.root())?;
    let ledger = state.load_ledger()?;
    if ledger
        .hosts
        .get(conflict.as_str())
        .is_none_or(|h| h.live != hosted)
    {
        return Err(invalid("default directory lease changed during selection"));
    }
    let readings = state.try_load_usage()?.accounts;
    // Preserve specific corruption errors rather than silently closing a gate.
    rotate::ledger::RotateState::new(store.root()).load_ledger()?;
    let profiles = config::every_profile(store.root())?;
    let now = fsx::now();
    let overflow = OverflowInputs::load_at(store.root(), &cfg, now, None);
    if overflow.kinds_unknown {
        return Err(invalid("fallback profile kinds cannot be read"));
    }
    let (targets, refusal) = verified_pool(store, conflict.as_str())?;
    let pool = candidate::Pool {
        profiles: &profiles,
        readings: &readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now,
    };
    let picked = rerun::select_verified(
        &state,
        &ledger,
        &pool,
        &overflow,
        conflict.as_str(),
        &|n| {
            ready(
                store,
                &state,
                &ledger,
                &cfg,
                &overflow,
                conflict.as_str(),
                n,
            )
        },
        &|n| targets.iter().any(|(p, _)| p == n),
    );
    let Some(name) = picked else {
        return refusal.map_or(Ok(None), Err);
    };
    let check = overflow
        .check_note(store.root(), name)
        .ok_or_else(|| invalid("fallback billing check changed"))?;
    let reason = per_token::reason(
        &pool,
        &overflow,
        &cfg,
        ledger.account_in(conflict.as_str()),
        name,
        check,
    );
    let evidence = targets
        .into_iter()
        .find(|(n, _)| n == name)
        .ok_or_else(|| invalid("destination disappeared during selection"))?
        .1;
    let mut target = LaunchTarget::named(store, name, JoinPolicy::Never, SelectionOrigin::Default)?;
    target.set_evidence(
        evidence,
        Some((conflict.to_string(), hosted.to_string())),
        reason,
    );
    Ok(Some(target))
}

/// Called with the state and refresh locks held. Local reads only, no refresh,
/// request, nested lock or automatic reselection on invalidation.
pub(in crate::commands::profile) fn validate(
    store: &ProfileStore,
    name: &ProfileName,
    evidence: &Evidence,
) -> Result<(), ProfileError> {
    if !evidence.credential.current() {
        return Err(invalid(
            "destination credentials changed since authentication",
        ));
    }
    validate_policy(store, name, &evidence.policy)
}

fn validate_policy(
    store: &ProfileStore,
    name: &ProfileName,
    evidence: &PolicyEvidence,
) -> Result<(), ProfileError> {
    ProfileName::parse(&evidence.source_host)?;
    ProfileName::parse(&evidence.source_login)?;
    if stamps(store, name, &evidence.source_login)? != evidence.stamps {
        return Err(invalid("destination evidence changed since selection"));
    }
    if pool_stamps(store, &evidence.source_login)? != evidence.pool {
        return Err(invalid("candidate pool changed since authentication"));
    }
    let state = State::new(store.root())?;
    let ledger = state.load_ledger()?;
    if ledger.account_in(&evidence.source_host) != evidence.source_login {
        return Err(invalid("destination source lease changed since selection"));
    }
    let cfg = config::read_editable(store.root())?;
    let readings = state.try_load_usage()?.accounts;
    rotate::ledger::RotateState::new(store.root()).load_ledger()?;
    let profiles = config::every_profile(store.root())?;
    let now = fsx::now();
    let overflow = OverflowInputs::load_at(store.root(), &cfg, now, None);
    let pool = candidate::Pool {
        profiles: &profiles,
        readings: &readings,
        threshold: cfg.threshold,
        exclude: &cfg.exclude,
        now,
    };
    let picked = rerun::select_verified(
        &state,
        &ledger,
        &pool,
        &overflow,
        &evidence.source_host,
        &|p| {
            ready(
                store,
                &state,
                &ledger,
                &cfg,
                &overflow,
                &evidence.source_host,
                p,
            )
        },
        &|p| evidence.approved.contains(p),
    );
    if picked != Some(name) {
        return Err(invalid(
            "destination is no longer eligible under capacity, exclusion, affinity or billing policy",
        ));
    }
    Ok(())
}

pub(in crate::commands::profile) fn record(root: &Path, line: Option<&str>) {
    per_token::record(root, line);
}

#[cfg(test)]
#[path = "tests_launch_fallback_contract.rs"]
mod tests_launch_fallback_contract;
