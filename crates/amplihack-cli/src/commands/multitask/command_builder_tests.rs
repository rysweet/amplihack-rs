//! Issue #1567: generated workstream scripts pass the pinned Claude login
//! profile down to the nested `amplihack` launch.

use super::command_builder::{
    CREATE_ISSUE_ENV, build_resume_context_with, classic_run_script, profile_exports,
    recipe_run_script, sh_single_quote,
};
use super::models::{Workstream, WorkstreamKey};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

const PROFILE_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE";
const ROOT_VAR: &str = "AMPLIHACK_CLAUDE_PROFILE_ROOT";

fn getter(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |k| map.get(k).cloned()
}

/// The auto-switch substitution seam with no substitution (switch off).
pub(super) fn keep(_inherited: &str) -> Option<String> {
    None
}

/// Issue #2014: an injected environment in which no variable is set. Tests
/// pass it to the launcher seam so that they never read the process
/// environment, which other tests in the same binary may change.
pub(super) fn no_env(_key: &str) -> Option<OsString> {
    None
}

pub(super) fn workstream(dir: &Path) -> Workstream {
    Workstream::new(
        WorkstreamKey::new("1567").unwrap(),
        Some(1567),
        "fix/1567".to_string(),
        "Pin profile".to_string(),
        "Keep the parent profile".to_string(),
        "default-workflow".to_string(),
        dir,
        dir,
    )
}

#[test]
fn sh_single_quote_wraps_and_escapes_quotes() {
    assert_eq!(sh_single_quote("work"), "'work'");
    assert_eq!(sh_single_quote(""), "''");
    assert_eq!(sh_single_quote("it's"), "'it'\\''s'");
}

#[test]
fn profile_exports_writes_quoted_lines_when_set() {
    let out = profile_exports(
        getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, "/home/u/profiles")]),
        keep,
    )
    .unwrap();
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE='work'\n"),
        "{out}"
    );
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/home/u/profiles'\n"),
        "{out}"
    );
}

#[test]
fn profile_exports_skips_unset_or_empty_values() {
    assert_eq!(profile_exports(getter(&[]), keep).unwrap(), "");
    assert_eq!(
        profile_exports(getter(&[(PROFILE_VAR, ""), (ROOT_VAR, "")]), keep).unwrap(),
        ""
    );
    let only_name = profile_exports(getter(&[(PROFILE_VAR, "work")]), keep).unwrap();
    assert!(only_name.contains("AMPLIHACK_CLAUDE_PROFILE='work'"));
    assert!(!only_name.contains(ROOT_VAR), "{only_name}");
}

#[test]
fn profile_exports_never_exports_claude_config_dir() {
    let out = profile_exports(
        getter(&[
            (PROFILE_VAR, "work"),
            (ROOT_VAR, "/r"),
            ("CLAUDE_CONFIG_DIR", "/r/work"),
        ]),
        keep,
    )
    .unwrap();
    assert!(!out.contains("CLAUDE_CONFIG_DIR"), "{out}");
}

#[cfg(unix)]
#[test]
fn hostile_root_round_trips_through_bash_without_running_anything() {
    let tmp = tempfile::tempdir().unwrap();
    let root = "/tmp/it's $(touch pwned) `touch pwned2`\nnext";
    let exports =
        profile_exports(getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, root)]), keep).unwrap();
    let script = format!("{exports}printf '%s' \"$AMPLIHACK_CLAUDE_PROFILE_ROOT\"");
    // `bash` is resolved from PATH, which other tests point at an empty
    // tempdir while holding the env lock; hold it so the spawn cannot see
    // their PATH and fail with ENOENT (#1730, #1731).
    let _env = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let out = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .current_dir(tmp.path())
        .env_remove(PROFILE_VAR)
        .env_remove(ROOT_VAR)
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(out.stdout, root.as_bytes());
    assert!(!tmp.path().join("pwned").exists());
    assert!(!tmp.path().join("pwned2").exists());
}

#[test]
fn recipe_run_script_exports_profile_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    // Issue #2014: the environment is injected, never read from the process.
    let script = recipe_run_script(
        &ws,
        "amplihack claude",
        getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, "/p/it's")]),
        keep,
    )
    .unwrap();
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE='work'\n"),
        "{script}"
    );
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/p/it'\\''s'\n"),
        "{script}"
    );
    assert!(!script.contains("CLAUDE_CONFIG_DIR"), "{script}");
    let exports_at = script.find("AMPLIHACK_CLAUDE_PROFILE=").unwrap();
    let exec_at = script.find("exec bash launcher.sh").unwrap();
    assert!(exports_at < exec_at, "exports must precede the launch");
}

#[test]
fn classic_run_script_exports_profile_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    let script = classic_run_script(
        &ws,
        "amplihack claude",
        getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, "/p")]),
        keep,
    )
    .unwrap();
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE='work'\n"),
        "{script}"
    );
    assert!(
        script.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/p'\n"),
        "{script}"
    );
    assert!(!script.contains("CLAUDE_CONFIG_DIR"), "{script}");
    let exports_at = script.find("AMPLIHACK_CLAUDE_PROFILE=").unwrap();
    let launch_at = script.find("amplihack claude --subprocess-safe").unwrap();
    assert!(exports_at < launch_at, "exports must precede the launch");
}

#[test]
fn run_scripts_have_no_profile_exports_when_unset_or_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    let unset: &[(&str, &str)] = &[];
    let empty: &[(&str, &str)] = &[(PROFILE_VAR, ""), (ROOT_VAR, "")];
    for pairs in [unset, empty] {
        // `impl Fn` getters are not `Copy`: build one per call.
        let recipe = recipe_run_script(&ws, "amplihack claude", getter(pairs), keep).unwrap();
        let classic = classic_run_script(&ws, "amplihack claude", getter(pairs), keep).unwrap();
        for script in [recipe, classic] {
            assert!(!script.contains(PROFILE_VAR), "{script}");
        }
    }
}

#[cfg(unix)]
#[test]
fn profile_exports_refuses_non_utf8_values_without_printing_them() {
    use std::os::unix::ffi::OsStringExt;
    for bad_key in [PROFILE_VAR, ROOT_VAR] {
        let get = move |k: &str| match k {
            k if k == bad_key => Some(OsString::from_vec(vec![b'/', 0xff, b'x'])),
            PROFILE_VAR => Some(OsString::from("work")),
            _ => None,
        };
        let err = profile_exports(get, keep).expect_err("non-UTF-8 value must be refused");
        let msg = format!("{err:#}");
        assert!(msg.contains(bad_key), "{msg}");
        assert!(msg.contains("not valid UTF-8"), "{msg}");
        assert!(
            !msg.contains('\u{fffd}'),
            "value must not be printed: {msg}"
        );
        assert!(!msg.contains("export "), "{msg}");
    }
}

#[cfg(unix)]
#[test]
fn run_scripts_refuse_a_non_utf8_profile_root() {
    use std::os::unix::ffi::OsStringExt;
    let tmp = tempfile::tempdir().unwrap();
    let ws = workstream(tmp.path());
    // Issue #2014: the invalid root is injected, so this test no longer sets
    // AMPLIHACK_CLAUDE_PROFILE_ROOT in the process for other tests to see.
    let bad = |k: &str| match k {
        PROFILE_VAR => Some(OsString::from("work")),
        ROOT_VAR => Some(OsString::from_vec(vec![b'/', 0xff])),
        _ => None,
    };
    let results = [
        recipe_run_script(&ws, "amplihack claude", bad, keep),
        classic_run_script(&ws, "amplihack claude", bad, keep),
    ];
    for result in results {
        let msg = format!("{:#}", result.expect_err("script must not be built"));
        assert!(msg.contains(ROOT_VAR), "{msg}");
        assert!(msg.contains("not valid UTF-8"), "{msg}");
        assert!(
            !msg.contains('\u{fffd}'),
            "value must not be printed: {msg}"
        );
    }
}

// Issue #1569: with auto-switch on, the exported profile is the candidate the
// substitution seam picks; the root and everything else are unchanged.

#[test]
fn profile_exports_writes_the_substituted_profile() {
    let out = profile_exports(
        getter(&[(PROFILE_VAR, "work"), (ROOT_VAR, "/r")]),
        |inherited: &str| (inherited == "work").then(|| "spare".to_string()),
    )
    .unwrap();
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE='spare'\n"),
        "{out}"
    );
    assert!(!out.contains("'work'"), "{out}");
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE_ROOT='/r'\n"),
        "{out}"
    );
}

#[test]
fn profile_exports_does_not_consult_the_seam_without_an_inherited_profile() {
    let out = profile_exports(getter(&[(ROOT_VAR, "/r")]), |_: &str| -> Option<String> {
        panic!("substitution must only be asked about an inherited profile")
    })
    .unwrap();
    assert!(!out.contains("export AMPLIHACK_CLAUDE_PROFILE="), "{out}");
}

#[test]
fn profile_exports_quotes_the_substituted_name() {
    let out = profile_exports(getter(&[(PROFILE_VAR, "work")]), |_: &str| {
        Some("it's".to_string())
    })
    .unwrap();
    assert!(
        out.contains("export AMPLIHACK_CLAUDE_PROFILE='it'\\''s'\n"),
        "{out}"
    );
}

/// Issue #1881: the parent's create_issue opt-in reaches each workstream's
/// recipe context; without it the workstream keeps the local default.
#[test]
fn resume_context_forwards_create_issue_only_when_set() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workstream(dir.path());
    let ctx =
        build_resume_context_with(&ws, |k| (k == CREATE_ISSUE_ENV).then(|| "true".to_string()));
    assert_eq!(ctx.get("create_issue"), Some(&serde_json::json!("true")));
    assert_eq!(ctx.get("issue_number"), Some(&serde_json::json!(1567)));

    let ctx = build_resume_context_with(&ws, |_| None);
    assert!(!ctx.contains_key("create_issue"));
    let ctx = build_resume_context_with(&ws, |_| Some("  ".to_string()));
    assert!(!ctx.contains_key("create_issue"));
}
