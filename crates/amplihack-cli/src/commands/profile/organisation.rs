//! Metadata-only organisation resolution. Hosting, rather than the directory
//! name, identifies the live login. Credential files are never read here.
use super::{
    ProfileError,
    auto_switch::kit::{Ledger, State},
};
use amplihack_utils::organisation_affinity::valid_id;

pub fn resolve(state: &State, ledger: &Ledger, host: &str) -> Result<Option<String>, ProfileError> {
    let login = ledger.account_in(host);
    resolve_login(state, login)
}

pub fn resolve_login(state: &State, login: &str) -> Result<Option<String>, ProfileError> {
    super::name::ProfileName::parse(login)?;
    use amplihack_utils::secure_files::{
        check_private_dir, check_private_file, read_bounded_nofollow,
    };
    match check_private_dir(&state.dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ProfileError::Io(e)),
    }
    let directory = state.dir.join("expected");
    match check_private_dir(&directory) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ProfileError::Io(e)),
    }
    let Some((raw, metadata)) =
        read_bounded_nofollow(&directory.join(format!("{login}.json")), 64 * 1024)?
    else {
        return Ok(None);
    };
    check_private_file(&metadata)?;
    // Malformed metadata is unknown. Operational failures above remain errors.
    let record: Option<serde_json::Value> = serde_json::from_slice(&raw).ok();
    if !record
        .as_ref()
        .and_then(|r| r.get("account"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(valid_id)
    {
        return Ok(None);
    }
    Ok(record
        .as_ref()
        .and_then(|r| r.get("org"))
        .and_then(serde_json::Value::as_str)
        .filter(|s| valid_id(s))
        .map(str::to_owned))
}

/// Restrict auto-switch candidates using the actual hosted source identity.
/// Unknown readings retain the selector's existing unread-candidate behavior.
#[allow(clippy::too_many_arguments)]
pub fn allowed(
    state: &State,
    ledger: &Ledger,
    profiles: &[super::name::ProfileName],
    source_host: &str,
    current: &str,
    necessary: bool,
    capacity: impl Fn(&super::name::ProfileName) -> bool,
    eligible: impl Fn(&super::name::ProfileName) -> bool,
) -> Result<Vec<super::name::ProfileName>, ProfileError> {
    use amplihack_utils::organisation_affinity::{Candidate, Purpose, allowed_indices};
    let source = resolve(state, ledger, source_host)?;
    let eligibility: Vec<_> = profiles
        .iter()
        .map(|p| p.as_str() != current && eligible(p))
        .collect();
    let orgs = profiles
        .iter()
        .zip(&eligibility)
        .map(|(p, allowed)| {
            if *allowed {
                resolve_login(state, p.as_str())
            } else {
                Ok(None)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let candidates = profiles
        .iter()
        .zip(&orgs)
        .zip(&eligibility)
        .map(|((p, org), allowed)| Candidate {
            org: org.as_deref(),
            eligible: *allowed,
            capacity: capacity(p),
        })
        .collect::<Vec<_>>();
    Ok(allowed_indices(
        source.as_deref(),
        if necessary {
            Purpose::CapacityRelief
        } else {
            Purpose::Upgrade
        },
        necessary,
        &candidates,
    )
    .into_iter()
    .map(|i| profiles[i].clone())
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn fixture() -> (tempfile::TempDir, State) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = State::new(tmp.path()).unwrap();
        state
            .write_sub_json(
                "expected",
                "work",
                &serde_json::json!({"account":"u-work", "org":"org-a"}),
            )
            .unwrap();
        (tmp, state)
    }
    #[test]
    fn issue_2136_hosting_changes_source_but_not_the_targets_own_slot() {
        let (_tmp, state) = fixture();
        state
            .write_sub_json(
                "expected",
                "other",
                &serde_json::json!({"account":"u-other", "org":"org-b"}),
            )
            .unwrap();
        let mut ledger = Ledger::default();
        ledger.hosts.insert(
            "work".into(),
            serde_json::from_value(
                serde_json::json!({"live":"other","swapped_at":1,"pre_swap_fingerprint":null}),
            )
            .unwrap(),
        );
        assert_eq!(
            resolve(&state, &ledger, "work").unwrap().as_deref(),
            Some("org-b")
        );
        assert_eq!(
            resolve_login(&state, "work").unwrap().as_deref(),
            Some("org-a")
        );
    }
    #[test]
    fn issue_2136_malformed_metadata_is_unknown_but_unsafe_files_are_errors() {
        let (_tmp, state) = fixture();
        let record = state.dir.join("expected/work.json");
        std::fs::write(&record, "invalid JSON").unwrap();
        assert_eq!(resolve_login(&state, "work").unwrap(), None);
        std::fs::remove_file(&record).unwrap();
        symlink("missing", &record).unwrap();
        assert!(resolve_login(&state, "work").is_err());
    }
}
