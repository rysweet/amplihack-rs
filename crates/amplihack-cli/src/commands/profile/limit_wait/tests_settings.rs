//! The usage-limit wait's settings (issue #1618): read from `config.toml`
//! and the environment through the user-config loader, on by default, off
//! only when `recipe.usage_limit_wait` or its variable says so, and `1`/`0`
//! accepted in the variable. `CI` is not read (issue #2911); a run of the
//! real binary with `CI=true` is in `tests/issue_1618_wait_settings.rs`.
//! There is no length setting (issue #2911): a wait ends on an event, and a
//! host still exporting the old variable is told it is no longer read.
//! Every load uses a temporary HOME and a supplied environment.
//!
//! Wired from `limit_wait/mod.rs` with `#[cfg(test)] mod tests_settings;`.

use super::settings::{RETIRED_MAX_WAIT_ENV, WaitSettings, from_effective, retired_warning};
use crate::user_config::{LoadCtx, load};
use std::fs;
use std::path::Path;

fn settings(home: &Path, env: &[(&str, &str)]) -> WaitSettings {
    let lookup = |name: &str| {
        env.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    };
    let effective = load(&LoadCtx {
        home: Some(home.to_path_buf()),
        cwd: home.to_path_buf(),
        read_project: false,
        env: &lookup,
        migrate: false,
    });
    from_effective(&effective)
}

fn user_file(home: &Path, body: &str) {
    let dir = home.join(".amplihack");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), body).unwrap();
}

const ON: WaitSettings = WaitSettings { enabled: true };
const OFF: WaitSettings = WaitSettings { enabled: false };

/// Issue #2911: with nothing set the wait is on, and `CI` is not one of the
/// settings. The loader is handed a `CI` of every kind that used to turn the
/// wait off and must still return the wait on; only the key or its variable
/// turns it off.
#[test]
fn issue_2911_the_wait_is_on_by_default_and_ci_does_not_turn_it_off() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(settings(home.path(), &[]), ON);
    for ci in ["true", "TRUE", "1", "yes", "github", " true "] {
        assert_eq!(settings(home.path(), &[("CI", ci)]), ON, "CI={ci}");
        let off = [("CI", ci), ("AMPLIHACK_USAGE_LIMIT_WAIT", "0")];
        assert_eq!(
            settings(home.path(), &off),
            OFF,
            "CI={ci} with the variable 0"
        );
    }
}

#[test]
fn an_explicit_setting_turns_the_wait_on_or_off() {
    let home = tempfile::tempdir().unwrap();
    let env = [("AMPLIHACK_USAGE_LIMIT_WAIT", "1")];
    assert_eq!(settings(home.path(), &env), ON);
    user_file(home.path(), "[recipe]\nusage_limit_wait = true\n");
    assert_eq!(settings(home.path(), &[]), ON);
    user_file(home.path(), "[recipe]\nusage_limit_wait = false\n");
    assert_eq!(settings(home.path(), &[]), OFF);
}

#[test]
fn the_variable_outranks_the_file() {
    let home = tempfile::tempdir().unwrap();
    user_file(home.path(), "[recipe]\nusage_limit_wait = false\n");
    assert_eq!(settings(home.path(), &[]), OFF);
    let env = [("AMPLIHACK_USAGE_LIMIT_WAIT", "true")];
    assert_eq!(settings(home.path(), &env), ON);
    let off = [("AMPLIHACK_USAGE_LIMIT_WAIT", "0")];
    assert!(!settings(home.path(), &off).enabled);
}

#[test]
fn an_invalid_variable_falls_back_to_the_file() {
    let home = tempfile::tempdir().unwrap();
    user_file(home.path(), "[recipe]\nusage_limit_wait = false\n");
    let env = [("AMPLIHACK_USAGE_LIMIT_WAIT", "maybe")];
    assert_eq!(settings(home.path(), &env), OFF);
}

// --- no length setting (issue #2911) --------------------------------------
//
// A wait whose limit stated no usable reset ends on a login with room or a
// login change, and a wait for a stated reset ends at that reset. A length
// setting would be a timer that runs the step's agent again on a login that
// may still be spent, so there is none.

/// The old key in a config file, at any value that used to mean something,
/// changes nothing the wait reads, and the wait stays on.
#[test]
fn issue_2911_the_old_length_key_changes_nothing() {
    for body in ["0", "60", "600", "21600", "86400"] {
        let home = tempfile::tempdir().unwrap();
        user_file(
            home.path(),
            &format!("[recipe]\nusage_limit_max_wait_secs = {body}\n"),
        );
        assert_eq!(settings(home.path(), &[]), ON, "a configured {body}");
    }
}

/// A host that still exports the old variable is told that it is no longer
/// read and what ends a wait instead, while the wait is on; the line names
/// the variable and never its value.
#[test]
fn issue_2911_a_host_still_exporting_the_old_variable_is_told_it_is_not_read() {
    let warning = retired_warning(true, &ON).expect("a warning while the wait is on");
    assert!(warning.starts_with("warning: "), "{warning}");
    assert!(warning.contains(RETIRED_MAX_WAIT_ENV), "{warning}");
    assert!(warning.contains("no longer read"), "{warning}");
    assert!(warning.contains("no timed bound"), "{warning}");
    assert!(
        warning.contains("amplihack profile switch"),
        "it says how to make the event: {warning}"
    );
    assert!(
        warning.contains("recipe.usage_limit_wait = false"),
        "{warning}"
    );
    assert_eq!(retired_warning(false, &ON), None, "nothing exported");
    assert_eq!(
        retired_warning(true, &OFF),
        None,
        "the wait is off, so nothing waits"
    );
    // The variable no longer reaches the settings at all.
    let home = tempfile::tempdir().unwrap();
    for v in ["0", "60", "abc"] {
        assert_eq!(
            settings(home.path(), &[(RETIRED_MAX_WAIT_ENV, v)]),
            ON,
            "{v}"
        );
    }
}
