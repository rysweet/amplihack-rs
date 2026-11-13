//! Fresh policy checks while the caller holds the observation/state lock.
use crate::commands::profile::auto_switch::config;
use crate::commands::profile::rotate::{ledger::RotateState, pin};
use crate::commands::profile::{ProfileError, ProfileStore, name::ProfileName};
use std::path::Path;

pub(super) fn allowed(root: &Path, profile: &ProfileName) -> Result<bool, ProfileError> {
    let effective = config::effective(root);
    if !effective.on
        || config::excluded_profiles(root)?.contains(profile)
        || effective.config.is_overflow(profile)
        || matches!(
            pin::gate(&pin::read(&RotateState::new(root))),
            pin::Gate::Held(_)
        )
    {
        return Ok(false);
    }
    ProfileStore::new(root.to_path_buf()).verify_profile_dir(profile)?;
    Ok(true)
}

/// A publication can make queued work unnecessary before collection starts.
/// The caller holds the existing state lock throughout this check.
pub(super) fn before_collection(
    root: &Path,
    profile: &ProfileName,
    guard: &mut amplihack_utils::usage_observation::ObservationGuard<'_>,
    binding: &amplihack_utils::usage_observation::Binding,
    location: &amplihack_utils::usage_observation::LoginLocation,
) -> Result<Option<crate::commands::profile::rotate::coordinator::ProbeResult>, ProfileError> {
    use crate::commands::profile::rotate::coordinator::ProbeResult;
    use amplihack_utils::usage_observation::ReadOutcome;
    if !allowed(root, profile)? {
        return Ok(Some(ProbeResult::Failed));
    }
    if !guard.probe_location_unchanged(binding, location)? {
        return Ok(Some(ProbeResult::RetiredGeneration));
    }
    if matches!(guard.read(binding)?, ReadOutcome::Usable(_)) {
        return Ok(Some(ProbeResult::Superseded));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, env_lock};
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn issue_2029_queued_probes_recheck_controls_before_profile_access() {
        let _lock = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _env = EnvGuard::unset([config::TOGGLE_ENV]);
        let tmp = tempfile::tempdir_in("/tmp").unwrap();
        let root = tmp.path();
        let name = ProfileName::parse("work").unwrap();
        fs::create_dir(root.join("work")).unwrap();
        fs::set_permissions(root.join("work"), fs::Permissions::from_mode(0o700)).unwrap();
        let control = root.join(config::FILE);
        let write = |raw: &str| {
            fs::write(&control, raw).unwrap();
            fs::set_permissions(&control, fs::Permissions::from_mode(0o600)).unwrap();
        };
        write("enabled=on\n");
        assert!(allowed(root, &name).unwrap());
        let observation_store =
            amplihack_utils::usage_observation::ObservationStore::open(root).unwrap();
        let mut guard = observation_store.lock().unwrap();
        let binding = guard.authorised_login("work", "account-work").unwrap();
        let location = guard
            .probe_location(&binding, &root.join("work/.credentials.json"))
            .unwrap()
            .unwrap();
        assert_eq!(
            before_collection(root, &name, &mut guard, &binding, &location).unwrap(),
            None
        );
        let capture = guard.capture_new(&binding, 1_790_000_000).unwrap();
        guard
            .publish(
                &capture,
                &amplihack_utils::usage_observation::Evidence {
                    session_window: amplihack_utils::usage_observation::SessionWindow::Present,
                    session_used: Some(10.0),
                    week_used: Some(20.0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            before_collection(root, &name, &mut guard, &binding, &location).unwrap(),
            Some(crate::commands::profile::rotate::coordinator::ProbeResult::Superseded)
        );
        drop(guard);
        let state = RotateState::new(root);
        pin::write(&state, &name).unwrap();
        assert!(!allowed(root, &name).unwrap());
        pin::clear(&state).unwrap();
        assert!(allowed(root, &name).unwrap());
        // If exclusion were checked after profile access, this symlink would
        // produce an unsafe-path error instead of a terminal policy refusal.
        fs::remove_dir(root.join("work")).unwrap();
        symlink(root.join("missing"), root.join("work")).unwrap();
        write("enabled=on\nexclude=work\n");
        assert!(!allowed(root, &name).unwrap());
        write("untrusted toggle syntax\n");
        assert!(!allowed(root, &name).unwrap());
    }
}
