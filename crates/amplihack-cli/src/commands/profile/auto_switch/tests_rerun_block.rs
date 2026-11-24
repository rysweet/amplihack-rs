//! Issue #2967: the retry after an earliest-reset wait: the note it prints,
//! and the logins whose blocks may end that wait ([`reset_logins`]).

use super::{NOTE_BLOCK_ENDED, Switch, reset_logins};
use crate::commands::profile::auto_switch::config::TOGGLE_ENV;
use crate::commands::profile::auto_switch::state::{CREDS, HostEntry, State};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::recipe::run::limit_patterns::all_markers;
use crate::commands::recipe::run::{FailureClass, classify_failure_text};
use crate::test_support::{EnvGuard, env_lock};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

#[test]
fn the_block_ended_note_carries_no_usage_limit_marker() {
    // It reaches the step's stderr, which the recipe runner's classifier
    // reads: a note that named a usage limit would be read as one.
    let line = format!("{NOTE_BLOCK_ENDED} \"personal\" (auto-switch)");
    let lowered = line.to_ascii_lowercase();
    for marker in all_markers() {
        assert!(!lowered.contains(marker), "{marker:?} in {line:?}");
    }
    assert_ne!(
        classify_failure_text(&line).0,
        FailureClass::UsageLimit,
        "{line:?}"
    );
    assert!(line.starts_with("amplihack: "), "{line:?}");
    assert_eq!(line.lines().count(), 1, "{line:?}");
}

const OAUTH: &str = r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#;

fn private_file(p: &Path, body: &str) {
    fs::write(p, body).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// The logins an earliest-reset wait reads are the ones the step ran on and
/// every other stored login the candidate rule could move it onto once its
/// block ends. Each login below is refused by one standing rule of the
/// candidate rule, so a retry could never run there; `plain` passes them all
/// and the step never ran on it, which is the incident's case.
#[test]
fn the_logins_an_earliest_reset_wait_reads() {
    let _l = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    private_dir(&home);
    let root = base.join("root");
    private_dir(&root);
    for (name, login) in [
        // The inherited login: its own reset times the wait.
        ("home", Some(OAUTH)),
        // A login the step ran on. It is an overflow profile, which only the
        // whole candidate rule, gate and all, could have moved the step onto,
        // so it counts because the step ran there and for no other reason.
        ("ran", Some(OAUTH)),
        // Never run on, and the candidate rule could move the step here.
        ("plain", Some(OAUTH)),
        // Excluded from auto-switch.
        ("excluded", Some(OAUTH)),
        // An overflow profile: its spend is the overflow gate's decision.
        ("credit", Some(OAUTH)),
        // An API-key profile is never a plan login (issue #1804).
        ("key", Some("{}")),
        // No login of its own: a retry there could not start.
        ("nologin", None),
        // Holds `leased`'s login, so neither is free in the ledger.
        ("hosting", Some(OAUTH)),
        ("leased", Some(OAUTH)),
    ] {
        let dir = root.join(name);
        private_dir(&dir);
        if let Some(body) = login {
            private_file(&dir.join(CREDS), body);
        }
    }
    private_file(
        &root.join(".auto-switch"),
        "enabled=on\nexclude=excluded\noverflow=credit,ran\n",
    );
    let _env = EnvGuard::set([("HOME", home.to_str().unwrap())]);
    let _unset = EnvGuard::unset([TOGGLE_ENV]);
    let state = State::new(&root).unwrap();
    state.ensure().unwrap();
    let mut ledger = state.load_ledger().unwrap();
    ledger.hosts.insert(
        "hosting".to_string(),
        HostEntry {
            live: "leased".to_string(),
            swapped_at: 0,
            pre_swap_fingerprint: None,
        },
    );
    state.save_ledger(&ledger).unwrap();
    let store = ProfileStore::new(root.clone());
    let sw = Switch {
        store: &store,
        state,
        inherited: ProfileName::parse("home").unwrap(),
    };

    let got = reset_logins(&sw, &["home".to_string(), "ran".to_string()]);
    assert_eq!(got, ["ran", "plain"]);

    // With nothing run on but the inherited login, as on 21 November 2026,
    // the logins the step never ran on are still read, and an overflow
    // profile it never ran on is not.
    assert_eq!(reset_logins(&sw, &["home".to_string()]), ["plain"]);
}
