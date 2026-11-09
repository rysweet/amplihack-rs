//! Acceptance checks for removing Python from the checkpoint tests
//! (#1554, PR #2020, consensus steps C-n to C-r).
//!
//! These compile against f14da87ed and fail at run time until the port,
//! the inventory map and the deletion commit are in place. They add no
//! guard logic of their own that could be relaxed later: each one checks a
//! file the round must produce or remove.
//!
//! Git runs through `amplihack_git::command_in`. Every bash started here has
//! `env_clear` and no `BASH_ENV` or `ENV`.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The eight approved owner dispositions (17.126 and 17.138). Nothing else
/// may appear as an allowlist row.
const EIGHT: [&str; 8] = [
    "17.138-1",
    "17.138-2",
    "17.138-3",
    "17.138-4",
    "17.138-6",
    "17.126-dest-read-79-site-1",
    "17.126-dest-read-79-site-2",
    "17.126-stage-strict-79",
];

const MAP: &str = "crates/amplihack-cli/tests/fixtures/workflow_checkpoint/inventory_map.toml";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn map() -> toml::Value {
    toml::from_str(&read(MAP)).expect("inventory_map.toml parses")
}

/// Bash with no inherited environment. `inherited` stands for whatever the
/// caller's environment held; `BASH_ENV` and `ENV` are always removed.
fn hermetic_bash(home: &Path, inherited: &[(&str, &OsStr)]) -> Command {
    let mut command = Command::new("bash");
    command.env_clear();
    for (key, value) in inherited {
        command.env(key, value);
    }
    command
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into()),
        )
        .env("HOME", home)
        .env("TMPDIR", home)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("BASH_ENV")
        .env_remove("ENV")
        .stdin(Stdio::null());
    command
}

// ------------------------------------------------------------- deletion
//
// Decision 17.146-2: these guards check the state of every commit on the
// way, not only the last one. The tracked `.py` set must equal the one
// rule every guard shares, `expected_tracked_python` in the validator; with
// no pending row both are empty and the script must pass.

use crate::workflow_checkpoint_inventory::{
    HELPER_USERS, PINNED_19, expected_tracked_python, tracked_python,
};

fn expected_now() -> BTreeSet<String> {
    expected_tracked_python(&map(), &HELPER_USERS)
}

#[test]
fn no_python_file_is_tracked() {
    let tracked = tracked_python(&root());
    let expected = expected_now();
    assert_eq!(
        tracked, expected,
        "the tracked .py files must be exactly the files that still have pending rows and the helpers they use"
    );
    let pinned: BTreeSet<String> = PINNED_19.iter().map(|s| (*s).to_owned()).collect();
    assert!(expected.is_subset(&pinned), "{expected:#?}");
    let pending = ["python_test", "procedural", "role", "site"]
        .iter()
        .flat_map(|t| rows_owned(&map(), t))
        .filter(|r| r.get("pending_file").is_some())
        .count();
    if pending == 0 {
        assert!(
            tracked.is_empty(),
            "no row is pending, so no .py file may be tracked: {tracked:#?}"
        );
    }
}

fn rows_owned(map: &toml::Value, table: &str) -> Vec<toml::Value> {
    map.get(table)
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Violations:` list the script prints on stderr.
fn script_violations(stderr: &str) -> BTreeSet<String> {
    stderr
        .split_once("\nViolations:\n")
        .map(|(_, list)| {
            list.lines()
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn run_check_no_python_assets() -> std::process::Output {
    Command::new("/bin/bash")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .current_dir(root())
        .arg("scripts/check-no-python-assets.sh")
        .stdin(Stdio::null())
        .output()
        .expect("run scripts/check-no-python-assets.sh")
}

#[test]
fn check_no_python_assets_script_passes() {
    let out = run_check_no_python_assets();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let expected = expected_now();
    if expected.is_empty() {
        assert_eq!(out.status.code(), Some(0), "{stderr}");
        assert!(out.stderr.is_empty(), "{stderr}");
    } else {
        // Expected to fail until the last file is ported, and only for the
        // right files. The tracked list comes from git too, so a tracked file
        // missing on disk (which the script skips) cannot hide.
        assert_eq!(out.status.code(), Some(1), "{stderr}");
        assert_eq!(script_violations(&stderr), expected, "{stderr}");
        assert_eq!(tracked_python(&root()), expected);
    }
}

#[test]
fn script_violation_parser_reads_only_the_listed_paths() {
    let stderr = "FAIL: tracked Python\n\nprose a.py\n\nViolations:\nx/a.py\ny/b.py\n";
    assert_eq!(
        script_violations(stderr),
        ["x/a.py", "y/b.py"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    );
    assert!(script_violations("PASS: no tracked Python\n").is_empty());
}

#[test]
fn hermetic_bash_ignores_a_hostile_bash_env_and_env() {
    let home = tempfile::tempdir().unwrap();
    let sentinel = home.path().join("hostile-ran");
    let hostile = home.path().join("hostile.sh");
    fs::write(&hostile, format!(": > '{}'\n", sentinel.display())).unwrap();
    let out = hermetic_bash(
        home.path(),
        &[
            ("BASH_ENV", hostile.as_os_str()),
            ("ENV", hostile.as_os_str()),
        ],
    )
    .args([
        "-c",
        "printf '%s|%s' \"${BASH_ENV-unset}\" \"${ENV-unset}\"",
    ])
    .output()
    .unwrap();
    assert_eq!(out.stdout, b"unset|unset");
    assert!(!sentinel.exists(), "a hostile startup file ran");
}

// -------------------------------------------------------- inventory map

fn rows<'a>(map: &'a toml::Value, table: &str) -> &'a [toml::Value] {
    map.get(table)
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

#[test]
fn inventory_map_allowlist_is_exactly_the_eight_approved_rows() {
    let map = map();
    let ids: Vec<&str> = rows(&map, "disposition")
        .iter()
        .map(|r| r.get("id").and_then(toml::Value::as_str).expect("id"))
        .collect();
    let set: BTreeSet<&str> = ids.iter().copied().collect();
    assert_eq!(ids.len(), set.len(), "duplicate disposition rows: {ids:?}");
    assert_eq!(set, EIGHT.into_iter().collect::<BTreeSet<_>>());
    for row in rows(&map, "disposition") {
        let covers = row.get("covers").and_then(toml::Value::as_array);
        assert!(
            covers.is_none_or(Vec::is_empty),
            "{row:?}: a disposition covers no obligation"
        );
        let pin = row.get("pinning_test").and_then(toml::Value::as_str);
        assert!(pin.is_some_and(|p| !p.is_empty()), "{row:?}: pinning_test");
    }
}

#[test]
fn inventory_map_coverage_rows_name_a_rust_test_or_one_of_the_eight() {
    let map = map();
    for table in ["python_test", "procedural", "role", "site"] {
        assert!(
            !rows(&map, table).is_empty(),
            "[[{table}]] rows are missing"
        );
        for row in rows(&map, table) {
            let test = row.get("rust_test").and_then(toml::Value::as_str);
            let disp = row.get("disposition").and_then(toml::Value::as_str);
            // 17.146-2: a pending row names the tracked file still holding
            // the behaviour. It is neither a target nor a disposition.
            let pending = row.get("pending_file").and_then(toml::Value::as_str);
            match (test, disp, pending) {
                (Some(t), None, None) => assert!(
                    t.contains("::") && !t.contains("amplifier-bundle"),
                    "[[{table}]] {row:?}: rust_test must be a Rust path"
                ),
                (None, Some(d), None) => assert!(EIGHT.contains(&d), "[[{table}]] {row:?}: {d}"),
                (None, None, Some(p)) => {
                    assert!(p.ends_with(".py"), "[[{table}]] {row:?}: pending_file");
                    assert!(!EIGHT.contains(&p), "[[{table}]] {row:?}");
                }
                _ => panic!(
                    "[[{table}]] {row:?}: exactly one of rust_test, disposition or pending_file"
                ),
            }
        }
    }
}

#[test]
fn inventory_map_has_one_row_per_reference_id() {
    let ids: serde_json::Value = serde_json::from_str(&read(
        "crates/amplihack-cli/tests/fixtures/workflow_checkpoint/inventory_ids.json",
    ))
    .unwrap();
    let map = map();
    let mapped = |table: &str| -> BTreeSet<String> {
        rows(&map, table)
            .iter()
            .map(|r| r["ref_id"].as_str().unwrap().to_owned())
            .collect()
    };
    let python: BTreeSet<String> = ids["python_tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(mapped("python_test"), python, "python_test rows");
    let sites: BTreeSet<String> = ids["checkpoint_sites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(mapped("site"), sites, "site rows");
    assert_eq!(
        rows(&map, "role").len(),
        ids["roles"].as_array().unwrap().len(),
        "one row per role entry"
    );
    assert_eq!(
        rows(&map, "procedural").len(),
        ids["procedural"].as_array().unwrap().len(),
        "one row per procedural assertion"
    );
}

#[test]
fn stage_strict_79_pin_test_exists_and_runs() {
    let map = map();
    let row = rows(&map, "disposition")
        .iter()
        .find(|r| r["id"].as_str() == Some("17.126-stage-strict-79"))
        .expect("17.126-stage-strict-79 row");
    let pin = row["pinning_test"].as_str().unwrap();
    let name = pin.rsplit("::").next().unwrap();
    assert_eq!(name, "stage_strict_passes_a_real_git_79_through");
}

// ------------------------------------------------- manifest and docs

#[test]
fn base64_is_the_only_new_dependency_and_has_one_lock_entry() {
    let manifest = read("crates/amplihack-cli/Cargo.toml");
    assert!(manifest.contains("base64 = \"0.22.1\""));
    let lock = read("Cargo.lock");
    assert_eq!(
        lock.matches("name = \"base64\"\nversion = \"0.22.1\"")
            .count(),
        1
    );
}

#[test]
fn checkpoint_doc_lists_every_authorised_difference() {
    let doc = read("docs/reference/workflow-checkpoint-cli.md");
    let section = doc
        .split_once("## Authorised changes (17.126)")
        .expect("17.126 section")
        .1
        .split_once("\n## Unresolved differences for owner disposition")
        .expect("unresolved section follows")
        .0;
    for id in EIGHT {
        assert!(section.contains(&format!("| {id} |")), "doc lacks {id}");
    }
    assert!(
        !section.contains("| 17.138-5 |"),
        "17.138 item 5 is restored, not an authorised difference"
    );
}
