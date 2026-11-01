//! Whether another login could take over (issue #1801): the one cheap,
//! read-only check that decides if a capacity wait starts. True when the store
//! holds a profile on another account than the step's that the selection could
//! choose: not excluded, no spend block, a plan login or a usable overflow
//! profile, and passing the login checks (issue #1804's rule for API keys). A profile's readings do not matter, because it may recover
//! during the wait. Every read error is `false`, which keeps today's skip.
//!
//! Wired from `auto_switch/mod.rs` with `mod tests_takeover;`.

use super::config::{self, FileState};
use super::fsx;
use super::overflow_inputs::OverflowInputs;
use super::takeover::{another_login_with, could_take_over, home_check, is_fresh, profile_check};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::Gate;
use crate::commands::profile::rotate::GateSource;
use crate::commands::profile::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn private(path: &Path, body: &str, mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn name(n: &str) -> ProfileName {
    ProfileName::parse(n).unwrap()
}

/// A store holding `names`, each with a login.
fn store(tmp: &tempfile::TempDir, names: &[&str]) -> ProfileStore {
    let store = ProfileStore::new(tmp.path().join("root"));
    for n in names {
        store.add(&name(n)).unwrap();
        private(
            &store.root().join(n).join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"t"}}"#,
            0o600,
        );
    }
    store
}

fn ledger(store: &ProfileStore, body: &str, mode: u32) {
    let path = amplihack_utils::rotator_status::ledger_path(store.root());
    private(&path, body, mode);
    for dir in [
        path.parent().unwrap(),
        path.parent().unwrap().parent().unwrap(),
    ] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn no_store_is_false() {
    assert!(!could_take_over(None, None));
    assert!(!could_take_over(None, Some("a")));
}

#[test]
fn a_missing_profile_root_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = ProfileStore::new(tmp.path().join("no-such-root"));
    assert!(!could_take_over(Some(&store), None));
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn a_second_profile_is_true() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    assert!(could_take_over(Some(&store), Some("a")));
}

#[test]
fn the_steps_own_login_alone_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a"]);
    assert!(!could_take_over(Some(&store), Some("a")));
}

/// The auto-switch ledger, recording that directory `dir` hosts `live`.
fn hosted(store: &ProfileStore, dir: &str, live: &str) {
    let path = store.root().join(".auto-switch.d").join("ledger.json");
    let body = serde_json::json!({
        "version": 1,
        "hosts": {dir: {"live": live, "swapped_at": 1, "pre_swap_fingerprint": null}},
    });
    private(&path, &body.to_string(), 0o600);
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn another_name_for_the_steps_own_account_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    hosted(&store, "b", "a");
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn the_account_a_steps_directory_hosts_is_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    hosted(&store, "a", "b");
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn a_profile_on_another_account_is_true() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b", "c"]);
    hosted(&store, "b", "a");
    assert!(could_take_over(Some(&store), Some("a")));
}

#[test]
fn with_no_live_profile_one_eligible_account_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a"]);
    assert!(!could_take_over(Some(&store), None));
}

#[test]
fn with_no_live_profile_two_names_for_one_account_are_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    hosted(&store, "b", "a");
    assert!(!could_take_over(Some(&store), None));
}

#[test]
fn with_no_live_profile_two_accounts_are_true() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    assert!(could_take_over(Some(&store), None));
}

#[test]
fn recorded_cycles_do_not_count_by_themselves() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a"]);
    let at = fsx::now();
    ledger(
        &store,
        &format!(
            r#"{{"version":1,"live":"a","alert":null,"last_cycle":{{"at":{at},"outcome":"swapped"}}}}"#
        ),
        0o600,
    );
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn a_profile_over_its_threshold_still_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    let now = fsx::now();
    let body = serde_json::json!({
        "version": 1,
        "accounts": {"b": {
            "five_hour": {"used_percentage": 100.0, "resets_at": now + 3600},
            "observed_at": now,
            "source": "statusline",
        }},
    });
    private(
        &store.root().join(".auto-switch.d").join("usage.json"),
        &body.to_string(),
        0o600,
    );
    fs::set_permissions(
        store.root().join(".auto-switch.d"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(could_take_over(Some(&store), Some("a")));
}

#[test]
fn an_excluded_profile_does_not_count() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    private(
        &store.root().join(".auto-switch"),
        "enabled=on\nthreshold=90\nexclude=b\n",
        0o600,
    );
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn a_spend_blocked_profile_does_not_count() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    let now = fsx::now();
    ledger(
        &store,
        &format!(
            r#"{{"version":1,"live":"a","alert":null,"overflow_blocked":{{"b":{{"at":{},"until":{},"reason":"spend_limit"}}}}}}"#,
            now - 10,
            now + 3600
        ),
        0o600,
    );
    assert!(!could_take_over(Some(&store), Some("a")));
}

// World write only: `writable_by_others` trusts group write when the group is
// the user's private group, as it is on most hosts, so 0o620 passes there.
#[test]
fn world_write_on_the_config_is_false() {
    for mode in [0o602, 0o666] {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(&tmp, &["a", "b"]);
        private(
            &store.root().join(".auto-switch"),
            "enabled=on\nthreshold=90\nexclude=\n",
            mode,
        );
        assert!(!could_take_over(Some(&store), Some("a")), "mode {mode:o}");
    }
}

#[test]
fn world_write_on_the_rotate_ledger_is_false() {
    for mode in [0o602, 0o666] {
        let tmp = tempfile::tempdir().unwrap();
        let store = store(&tmp, &["a", "b"]);
        ledger(&store, r#"{"version":1,"live":"a","alert":null}"#, mode);
        assert!(!could_take_over(Some(&store), Some("a")), "mode {mode:o}");
    }
}

#[test]
fn malformed_files_are_false_without_panicking() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    ledger(&store, "{not json", 0o600);
    assert!(!could_take_over(Some(&store), Some("a")));

    let tmp = tempfile::tempdir().unwrap();
    let store = self::store(&tmp, &["a", "b"]);
    private(
        &store.root().join(".auto-switch"),
        "\u{0}\u{1}garbage",
        0o600,
    );
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn a_reading_is_fresh_only_after_the_start_and_near_now() {
    const START: i64 = 1_000;
    const NOW: i64 = 2_000;
    assert!(!is_fresh(START, START, NOW), "taken as the wait began");
    assert!(is_fresh(START + 1, START, NOW));
    assert!(is_fresh(NOW + 60, START, NOW));
    assert!(!is_fresh(NOW + 61, START, NOW), "stamped in the future");
    assert!(is_fresh(i64::MAX, START, i64::MAX), "no overflow");
}

// S4 (issue #1804 on main): an API-key profile counts only as the selection
// would use it, marked `overflow=`, with the gate open, no API-key block and a
// key Claude can read. An unmarked one never counts, so a capacity wait never
// starts for a login nothing will choose.

/// `b` is an API-key profile with a `{}` login, as on the host; `overflow`
/// is the config's `overflow=` value, empty for none.
fn api_key_b(store: &ProfileStore, overflow: &str) {
    private(
        &store.root().join("b").join(".credentials.json"),
        "{}",
        0o600,
    );
    private(
        &store.root().join(".auto-switch"),
        &format!("enabled=on\nthreshold=90\nexclude=\nkind=b:platform\noverflow={overflow}\n"),
        0o600,
    );
}

/// `b`'s own settings name an API key, so its login check passes.
fn key_for_b(store: &ProfileStore) {
    private(
        &store.root().join("b").join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"SENTINEL"}}"#,
        0o600,
    );
}

/// `a` used up: the step's own plan login at 100% of its week.
fn a_used_up(store: &ProfileStore) {
    let now = fsx::now();
    let body = serde_json::json!({
        "version": 1,
        "accounts": {"a": {
            "seven_day": {"used_percentage": 100.0, "resets_at": now + 4 * 86_400},
            "observed_at": now,
            "source": "statusline",
        }},
    });
    private(
        &store.root().join(".auto-switch.d").join("usage.json"),
        &body.to_string(),
        0o600,
    );
    fs::set_permissions(
        store.root().join(".auto-switch.d"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}

const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};

/// The takeover check with the overflow gate given, `None` for closed.
fn takes_over_with_gate(store: &ProfileStore, own: &str, gate: Option<Gate>) -> bool {
    let FileState::Valid(cfg) = config::read(store.root()) else {
        panic!("the test config must be valid");
    };
    let source = match gate {
        Some(_) => GateSource::Environment,
        None => GateSource::Closed("test".to_string()),
    };
    let inputs = OverflowInputs::load_with_gate(store.root(), &cfg, (gate, source), fsx::now());
    another_login_with(store, Some(own), &cfg, &inputs).unwrap_or(false)
}

#[test]
fn an_exhausted_plan_login_and_an_unmarked_api_key_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    api_key_b(&store, "");
    key_for_b(&store);
    a_used_up(&store);
    ledger(&store, r#"{"version":1,"live":"a","alert":null}"#, 0o600);
    assert!(
        !could_take_over(Some(&store), Some("a")),
        "auto-switch never chooses an unmarked key, so no wait may start for it"
    );
    assert!(!takes_over_with_gate(&store, "a", Some(GATE)));
}

#[test]
fn a_marked_api_key_with_a_passing_check_and_an_open_gate_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    api_key_b(&store, "b");
    key_for_b(&store);
    a_used_up(&store);
    ledger(&store, r#"{"version":1,"live":"a","alert":null}"#, 0o600);
    assert!(takes_over_with_gate(&store, "a", Some(GATE)));
}

#[test]
fn a_marked_api_key_with_the_gate_closed_does_not_count() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    api_key_b(&store, "b");
    key_for_b(&store);
    ledger(&store, r#"{"version":1,"live":"a","alert":null}"#, 0o600);
    assert!(
        !takes_over_with_gate(&store, "a", None),
        "a closed gate leaves only plan logins"
    );
}

#[test]
fn a_marked_api_key_that_fails_its_login_check_does_not_count() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    api_key_b(&store, "b");
    ledger(&store, r#"{"version":1,"live":"a","alert":null}"#, 0o600);
    assert!(
        !takes_over_with_gate(&store, "a", Some(GATE)),
        "no key in b's own files: the selection refuses it"
    );
}

#[test]
fn a_platform_blocked_profile_does_not_count() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    api_key_b(&store, "b");
    key_for_b(&store);
    let now = fsx::now();
    ledger(
        &store,
        &format!(
            r#"{{"version":1,"live":"a","alert":null,"platform_block":{{"at":{},"until":{},"reason":"key_exhausted"}}}}"#,
            now - 10,
            now + 3600
        ),
        0o600,
    );
    assert!(!takes_over_with_gate(&store, "a", Some(GATE)));
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn an_expired_platform_block_does_not_exclude() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    api_key_b(&store, "b");
    key_for_b(&store);
    let now = fsx::now();
    ledger(
        &store,
        &format!(
            r#"{{"version":1,"live":"a","alert":null,"platform_block":{{"at":{},"until":{},"reason":"key_rate_limited"}}}}"#,
            now - 7200,
            now - 3600
        ),
        0o600,
    );
    assert!(takes_over_with_gate(&store, "a", Some(GATE)));
}

#[test]
fn an_unknown_kind_in_the_config_is_false() {
    // The config parser fails closed on an unknown kind; so does the check.
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    private(
        &store.root().join(".auto-switch"),
        "enabled=on\nthreshold=90\nexclude=\nkind=b:mystery\n",
        0o600,
    );
    assert!(!could_take_over(Some(&store), Some("a")));
}

#[test]
fn a_store_that_cannot_be_listed_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o000)).unwrap();
    // Root ignores directory modes; the row only means something otherwise.
    let readable = fs::read_dir(store.root()).is_ok();
    let got = could_take_over(Some(&store), Some("a"));
    fs::set_permissions(store.root(), fs::Permissions::from_mode(0o700)).unwrap();
    if !readable {
        assert!(!got, "an unreadable store must fail closed");
    }
}

/// Holds the external rotator's lock under `store`, as a running prototype
/// rotator does, until the returned file is dropped.
fn rotator_running(store: &ProfileStore) -> fs::File {
    use std::os::fd::AsRawFd;
    let dir = store.root().join(".rotator");
    fs::create_dir_all(&dir).unwrap();
    let lock = dir.join("lock");
    fs::write(&lock, "").unwrap();
    let holder = fs::File::open(&lock).unwrap();
    // SAFETY: valid open descriptor.
    assert_eq!(
        unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    holder
}

// A `$HOME/.claude` step's wait can end only on a rotator cycle or a login
// change, so with no rotator a far reset keeps its immediate skip (#1801).
#[test]
fn a_home_step_with_two_logins_and_no_rotator_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    assert!(
        could_take_over(Some(&store), None),
        "the store alone qualifies"
    );
    assert!(!home_check(Some(store))());
}

#[test]
fn a_home_step_with_two_logins_and_a_running_rotator_is_true() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    let _held = rotator_running(&store);
    assert!(home_check(Some(store))());
}

#[test]
fn a_running_rotator_alone_is_not_enough() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a"]);
    let _held = rotator_running(&store);
    assert!(!home_check(Some(store))());
}

#[test]
fn a_home_step_with_no_store_is_false() {
    assert!(!home_check(None)());
}

// An inherited-profile step's wait ends only on a fresh reading, a rotator
// cycle, a login change or its own reset. In an unattended run nothing may
// record a reading without a rotator, so a far reset keeps its immediate skip
// there too (#1801).
#[test]
fn an_inherited_profile_step_with_two_logins_and_no_rotator_is_false() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    assert!(
        could_take_over(Some(&store), Some("a")),
        "the store alone qualifies"
    );
    assert!(!profile_check(&store, "a"));
}

#[test]
fn an_inherited_profile_step_with_two_logins_and_a_running_rotator_is_true() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a", "b"]);
    let _held = rotator_running(&store);
    assert!(profile_check(&store, "a"));
}

#[test]
fn a_running_rotator_alone_is_not_enough_for_a_profile_step() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(&tmp, &["a"]);
    let _held = rotator_running(&store);
    assert!(!profile_check(&store, "a"));
}
