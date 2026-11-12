//! Unit contracts for protected policy validation, without network or homes.
use super::*;
use crate::commands::profile::auto_switch::state::{HostEntry, UsageFile};
use crate::commands::profile::auto_switch::usage::{Reading, ReadingSource, Window};
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;

fn name(raw: &str) -> ProfileName {
    ProfileName::parse(raw).unwrap()
}

fn private(path: &Path, body: impl AsRef<[u8]>) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

struct PolicyFixture {
    _temp: tempfile::TempDir,
    store: ProfileStore,
    state: State,
}
impl PolicyFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("profiles");
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let store = ProfileStore::new(root).with_seed_hooks(Err("isolated fixture".into()));
        let state = State::new(store.root()).unwrap();
        state.ensure().unwrap();
        state.ensure_sub(&state.dir.join("expected")).unwrap();
        for (profile, org) in [
            ("X", "org-team"),
            ("Y", "org-team"),
            ("W", "org-team"),
            ("Z", "org-other"),
        ] {
            let dir = store.root().join(profile);
            fs::create_dir(&dir).unwrap();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
            private(
                &dir.join(CREDS),
                r#"{"claudeAiOauth":{"accessToken":"UNIT-ONLY"}}"#,
            );
            private(
                &state.dir.join("expected").join(format!("{profile}.json")),
                serde_json::json!({"account":format!("uuid-{profile}"),"org":org}).to_string(),
            );
        }
        private(
            &store.root().join(".auto-switch"),
            "enabled=on\nthreshold=90\n",
        );
        state
            .save_ledger(&Ledger {
                version: 1,
                hosts: BTreeMap::from([(
                    "X".into(),
                    HostEntry {
                        live: "Y".into(),
                        swapped_at: fsx::now(),
                        pre_swap_fingerprint: None,
                    },
                )]),
            })
            .unwrap();
        let fixture = Self {
            _temp: temp,
            store,
            state,
        };
        fixture.usage(100.0, 10.0);
        fixture
    }

    fn usage(&self, peer: f64, selected: f64) {
        let now = fsx::now();
        self.state
            .save_usage(&UsageFile {
                version: 1,
                accounts: [("X", 100.0), ("Y", 100.0), ("W", peer), ("Z", selected)]
                    .into_iter()
                    .map(|(profile, used)| {
                        (
                            profile.to_string(),
                            Reading {
                                five_hour: Some(Window {
                                    used_percentage: used,
                                    resets_at: now + 3600,
                                }),
                                seven_day: Some(Window {
                                    used_percentage: used,
                                    resets_at: now + 86400,
                                }),
                                observed_at: now,
                                source: ReadingSource::Statusline,
                            },
                        )
                    })
                    .collect(),
            })
            .unwrap();
    }

    /// Construct only the continuity input this unit validates. The end-to-end
    /// tests separately require credential authentication before obtaining it.
    fn evidence(&self) -> PolicyEvidence {
        PolicyEvidence {
            source_host: "X".into(),
            source_login: "Y".into(),
            stamps: stamps(&self.store, &name("Z"), "Y").unwrap(),
            pool: pool_stamps(&self.store, "Y").unwrap(),
            approved: vec![name("W"), name("Z")],
        }
    }

    fn validate_protected(&self, evidence: &PolicyEvidence) -> Result<(), ProfileError> {
        // This unit exercises policy under owned locks. Lease recovery and
        // live-session discovery belong to the separate integration fixture.
        let mut hold = super::super::LaunchHold {
            _lock: Some(self.state.lock()?),
            _refresh: None,
        };
        hold.protect_refresh(&self.store.root().join("Z"))?;
        validate_policy(&self.store, &name("Z"), evidence)
    }
}

#[test]
fn full_pool_capacity_recovery_invalidates_cross_organisation_permission() {
    let f = PolicyFixture::new();
    let evidence = f.evidence();
    f.validate_protected(&evidence)
        .expect("crossing is allowed while peers are full");
    f.usage(20.0, 10.0);
    assert!(
        f.validate_protected(&evidence).is_err(),
        "checking only Z hides W's capacity and wrongly retains crossing permission"
    );
}

#[test]
fn either_selected_usage_window_reaching_the_threshold_refuses_launch() {
    let f = PolicyFixture::new();
    let evidence = f.evidence();
    f.validate_protected(&evidence).unwrap();
    for weekly in [false, true] {
        f.usage(100.0, 10.0);
        let mut usage = f.state.try_load_usage().unwrap();
        let selected = usage.accounts.get_mut("Z").unwrap();
        let window = if weekly {
            &mut selected.seven_day
        } else {
            &mut selected.five_hour
        };
        window.as_mut().unwrap().used_percentage = 90.0;
        f.state.save_usage(&usage).unwrap();
        assert!(f.validate_protected(&evidence).is_err(), "weekly={weekly}");
    }
}

#[test]
fn changed_expected_identity_invalidates_the_selected_destination() {
    let f = PolicyFixture::new();
    let evidence = f.evidence();
    f.validate_protected(&evidence).unwrap();
    private(
        &f.state.dir.join("expected/Z.json"),
        r#"{"account":"uuid-other","org":"org-other"}"#,
    );
    assert!(
        f.validate_protected(&evidence).is_err(),
        "a changed expected account cannot authorise the previously selected login"
    );
}
