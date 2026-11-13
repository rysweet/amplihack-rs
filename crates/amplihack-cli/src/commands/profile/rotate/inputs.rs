//! Semantic input acquisition. Kernel events are hints: file metadata selects
//! changed observations; startup and overflow read every observation once.
use super::coordinator::{EvidenceInput, InputState, LoginInput};
use super::watch_loop::ReadPass;
use crate::commands::profile::auto_switch::{config, kit::State};
use crate::commands::profile::{
    ProfileError, default_live, expected, name::ProfileName, store::ProfileStore,
};
use amplihack_utils::usage_observation::{ObservationStore, ReadOutcome, Record};
use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

type Fingerprint = (u64, u64, u64, i64, i64, u32, u32, i64, i64);
fn fingerprint(path: &Path) -> Result<Option<Fingerprint>, ProfileError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) => Ok(Some((
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.mode(),
            m.uid(),
            m.ctime(),
            m.ctime_nsec(),
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
struct Cached {
    file: Option<Fingerprint>,
    accepted: Option<u64>,
    record: Option<Record>,
}
#[derive(Default)]
pub(super) struct Inputs {
    cached: BTreeMap<String, Cached>,
}
impl Inputs {
    pub fn read(
        &mut self,
        store: &ProfileStore,
        home: &Path,
        pass: ReadPass,
    ) -> Result<InputState, ProfileError> {
        let state = State::new(store.root())?;
        let _lock = state.lock()?;
        let observations = ObservationStore::open(store.root())?;
        let mut guard = observations.under_state_lock();
        let profiles: Vec<_> = store
            .list()?
            .iter()
            .map(|n| ProfileName::parse(n))
            .collect::<Result<_, _>>()?;
        // Upgrade recovery uses only an existing authorised expected-account
        // record. Ordinary observation producers never perform this step.
        if pass == ReadPass::Startup {
            let legacy_live =
                default_live::live_name(store, Some(home)).map(|name| name.to_string());
            for name in &profiles {
                if !guard.registered(name.as_str())?
                    && let Ok(Some(account)) = expected::lookup(&state, name)
                {
                    let binding = guard.authorised_login(name.as_str(), &account.account)?;
                    if legacy_live.as_deref() == Some(name.as_str()) {
                        guard.set_location(&binding, &home.join(".claude/.credentials.json"))?;
                    }
                }
            }
        }
        let live = match guard.binding_at(&home.join(".claude/.credentials.json")) {
            Ok(binding) => binding.map(|binding| binding.profile),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let effective = config::effective(store.root());
        let excluded =
            crate::commands::profile::usage_cmd::exclude_list(store.root(), |w| eprintln!("{w}"))?;
        let pin = super::pin::read(&super::ledger::RotateState::new(store.root()));
        let mut result = InputState {
            held: matches!(super::pin::gate(&pin), super::pin::Gate::Held(_)),
            enabled: effective.on,
            control: policy_control(&effective, &pin),
            directories: super::watch_source::watch_dirs(store.root(), home, &profiles),
            ..Default::default()
        };
        for (binding, accepted) in guard.reconcile_bindings()? {
            let name = ProfileName::parse(&binding.profile)?;
            if !profiles.contains(&name) {
                guard.retire(&binding)?;
                continue;
            }
            let expected = expected::lookup(&state, &name).ok().flatten();
            let organisation = expected
                .as_ref()
                .filter(|e| e.account == binding.account_id)
                .and_then(|e| e.org.clone());
            let eligible = !excluded.contains(&name)
                && !effective.config.is_overflow(&name)
                && store.verify_profile_dir(&name).is_ok();
            result.logins.push(LoginInput {
                live: live.as_deref() == Some(binding.profile.as_str()),
                eligible,
                organisation,
                profile: binding.profile.clone(),
                account_id: binding.account_id.clone(),
                generation: binding.generation.clone(),
            });
            let path = observations.record_path(&binding)?;
            let stamp = fingerprint(&path)?;
            let key = format!("{}:{}", binding.profile, binding.generation);
            let reload = matches!(pass, ReadPass::Startup | ReadPass::Overflow)
                || self
                    .cached
                    .get(&key)
                    .is_none_or(|old| old.file != stamp || old.accepted != accepted);
            if reload {
                let record = match guard.read(&binding)? {
                    ReadOutcome::Usable(r) => Some(r),
                    ReadOutcome::Missing | ReadOutcome::StaleGeneration => None,
                    refused => {
                        tracing::warn!(profile = %binding.profile, ?refused, "usage observation refused");
                        None
                    }
                };
                self.cached.insert(
                    key.clone(),
                    Cached {
                        file: stamp,
                        accepted,
                        record,
                    },
                );
            }
            if let Some(record) = self
                .cached
                .get(&key)
                .and_then(|cached| cached.record.as_ref())
            {
                result.observations.push(evidence_input(record));
                result.records.push(record.clone());
            }
        }
        self.cached.retain(|key, _| {
            result
                .logins
                .iter()
                .any(|l| key == &format!("{}:{}", l.profile, l.generation))
        });
        Ok(result)
    }
}
/// Compare effective controls, not exclusion ordering or a file toggle
/// shadowed by the environment. Revalidation uses the identical normal form.
pub(super) fn policy_control(effective: &config::Effective, pin: &super::pin::PinState) -> String {
    let mut policy = effective.config.clone();
    policy.enabled = effective.on;
    policy.exclude.sort();
    policy.exclude.dedup();
    format!("{policy:?}|{pin:?}")
}

pub(super) fn evidence_input(record: &Record) -> EvidenceInput {
    let capture = &record.capture;
    EvidenceInput {
        profile: capture.binding.profile.clone(),
        account_id: capture.binding.account_id.clone(),
        generation: capture.binding.generation.clone(),
        observation_id: capture.observation_id.clone(),
        ticket: capture.ticket,
        captured_at: capture.captured_at,
        session_window: record.evidence.session_window,
        session_reset_at: record.evidence.session_reset_at,
        week_reset_at: record.evidence.week_reset_at,
        session_used: record.evidence.session_used,
        week_used: record.evidence.week_used,
        limit_reached: record.evidence.limit_reached,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn issue_2029_control_identity_ignores_exclusion_order_and_shadowed_toggles() {
        use super::*;
        let mut first = config::Config {
            enabled: true,
            exclude: vec![
                ProfileName::parse("work").unwrap(),
                ProfileName::parse("spare").unwrap(),
            ],
            ..Default::default()
        };
        let a = config::effective_from(
            config::EnvToggle::On,
            &config::FileState::Valid(first.clone()),
        );
        first.enabled = false;
        first.exclude.reverse();
        let b = config::effective_from(config::EnvToggle::On, &config::FileState::Valid(first));
        assert_eq!(
            policy_control(&a, &super::super::pin::PinState::None),
            policy_control(&b, &super::super::pin::PinState::None)
        );
    }
    #[test]
    fn issue_2029_cache_does_not_resurrect_a_record_after_ordering_advances_without_replacement() {
        use super::*;
        use amplihack_utils::usage_observation::{Evidence, SCHEMA_VERSION, SessionWindow};
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir_in("/tmp").unwrap();
        let root = tmp.path().join("profiles");
        let home = tmp.path().join("home");
        for path in [&root, &home, &root.join("work"), &home.join(".claude")] {
            fs::create_dir(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let credentials = home.join(".claude/.credentials.json");
        fs::write(&credentials, b"opaque inode fixture").unwrap();
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o600)).unwrap();
        let observations = ObservationStore::open(&root).unwrap();
        let evidence = Evidence {
            session_window: SessionWindow::Present,
            session_used: Some(10.0),
            week_used: Some(20.0),
            ..Default::default()
        };
        let binding = {
            let mut guard = observations.lock().unwrap();
            let binding = guard.authorised_login("work", "account-work").unwrap();
            guard.set_location(&binding, &credentials).unwrap();
            let capture = guard.capture_new(&binding, 1_790_000_000).unwrap();
            guard.publish(&capture, &evidence).unwrap();
            binding
        };
        let store = ProfileStore::new(root.clone());
        let mut inputs = Inputs::default();
        let baseline = inputs.read(&store, &home, ReadPass::Startup).unwrap();
        assert_eq!(baseline.observations.len(), 1);
        assert!(
            baseline.logins[0].live,
            "registered home ownership must not depend on a rotation ledger or ~/.claude.json"
        );
        let record_path = observations.record_path(&binding).unwrap();
        let before = fs::read(&record_path).unwrap();
        // Persist the exact crash boundary: accepted ordering has advanced,
        // while the previous complete record still occupies its old inode.
        let mut guard = observations.lock().unwrap();
        let newer = guard.capture_new(&binding, 1_790_000_001).unwrap();
        let metadata = root.join(".auto-switch.d/observation-order.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata).unwrap()).unwrap();
        state["logins"]["work"]["accepted"] = serde_json::to_value(Record {
            schema_version: SCHEMA_VERSION,
            capture: newer,
            evidence,
            revision: 2,
        })
        .unwrap();
        fs::write(&metadata, serde_json::to_vec(&state).unwrap()).unwrap();
        drop(guard);
        assert!(
            inputs
                .read(&store, &home, ReadPass::Changed)
                .unwrap()
                .observations
                .is_empty()
        );
        assert_eq!(fs::read(record_path).unwrap(), before);
    }

    #[test]
    fn issue_2029_observation_cache_rechecks_permission_changes() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir_in("/tmp").unwrap();
        let record = tmp.path().join("observation.json");
        std::fs::write(&record, b"{}").unwrap();
        std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        let private = super::fingerprint(&record).unwrap();
        std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_ne!(private, super::fingerprint(&record).unwrap());
    }
}
