// File: crates/amplihack-workflows/tests/canonical_staging_portability_test.rs
//! Refs #1554 and Refs #1914. The portable Git-and-tempdir controls run in
//! plain `cargo test` and nextest. When CANONICAL_STAGING_EVIDENCE_DIR is
//! unset, each Fixture owns a temporary evidence directory for its lifetime;
//! when it is set, evidence goes under it and is kept. One code path; only
//! the root differs.
//!
//! The environment cannot be changed safely inside a multi-threaded test
//! process, so each case re-runs this test binary for the gated probe below
//! with the variable set or removed.
#![cfg(unix)]
#[path = "support/canonical_staging.rs"]
mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use support::*;

const PROBE: &str = "CANONICAL_STAGING_EVIDENCE_PROBE";

/// Runs only when re-executed by the tests below.
#[test]
fn evidence_root_probe() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    let f = Fixture::checkpoint_clean("evidence-probe");
    let evidence = f.evidence.as_path().to_path_buf();
    assert!(evidence.is_dir(), "evidence directory missing");
    fs::write(evidence.join("probe.json"), b"{}\n").unwrap();
    println!("EVIDENCE={}", evidence.display());
    drop(f);
    println!("EXISTS_AFTER_DROP={}", evidence.exists());
}

fn run_probe(evidence_dir: Option<&std::path::Path>) -> (PathBuf, bool) {
    let mut c = Command::new(std::env::current_exe().unwrap());
    c.args([
        "--exact",
        "evidence_root_probe",
        "--nocapture",
        "--test-threads=1",
    ])
    .env(PROBE, "1");
    match evidence_dir {
        Some(d) => c.env("CANONICAL_STAGING_EVIDENCE_DIR", d),
        None => c.env_remove("CANONICAL_STAGING_EVIDENCE_DIR"),
    };
    let out = c.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "probe failed (variable {}):\n{text}\n{}",
        if evidence_dir.is_some() {
            "set"
        } else {
            "unset"
        },
        String::from_utf8_lossy(&out.stderr)
    );
    let field = |name: &str| {
        text.lines()
            .find_map(|l| l.split_once(&format!("{name}=")).map(|(_, v)| v))
            .unwrap_or_else(|| panic!("{name} not printed:\n{text}"))
            .to_owned()
    };
    (
        PathBuf::from(field("EVIDENCE")),
        field("EXISTS_AFTER_DROP") == "true",
    )
}

#[test]
fn fixture_owns_a_temporary_evidence_directory_when_the_variable_is_unset() {
    let (evidence, kept) = run_probe(None);
    assert!(
        !kept,
        "an owned evidence directory must be removed with the fixture: {}",
        evidence.display()
    );
}

#[test]
fn fixture_keeps_evidence_under_the_assigned_directory_when_the_variable_is_set() {
    let assigned = tempfile::Builder::new()
        .prefix("canonical-staging-evidence-")
        .tempdir()
        .unwrap();
    let root = assigned.path().canonicalize().unwrap();
    let (evidence, kept) = run_probe(Some(&root));
    assert!(
        evidence.starts_with(&root),
        "{} is outside {}",
        evidence.display(),
        root.display()
    );
    assert!(kept, "evidence under the assigned directory must be kept");
    assert!(evidence.join("probe.json").is_file());
}

const ADAPTER: &str =
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py";
const RECEIPTS: &str =
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py";
/// The only Python user of both fixtures (decision 17.146-2 helper table).
const LAST_USER: &str = "amplifier-bundle/recipes/tests/test-checkpoint-scope.py";

fn tracked_python() -> std::collections::BTreeSet<String> {
    let out = amplihack_git::command_in(root())
        .args(["ls-files", "-z", "--", "*.py"])
        .output()
        .expect("git ls-files");
    assert!(out.status.success());
    out.stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8(p.to_vec()).unwrap())
        .collect()
}

/// Why the support source may not name `fixture`, or `None` if it may: a
/// Python fixture is installed only while it is tracked and its last user
/// is still tracked (the expected set of 17.146-2).
fn adapter_violation(
    src: &str,
    fixture: &str,
    tracked: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let name = fixture.rsplit('/').next().unwrap();
    if !src.contains(name) {
        return None;
    }
    if !tracked.contains(fixture) {
        return Some(format!("{name} is installed but not tracked"));
    }
    if !tracked.contains(LAST_USER) {
        return Some(format!(
            "{name} is installed after its last user was ported"
        ));
    }
    None
}

#[test]
fn the_fixture_installs_no_python_adapter() {
    let src = fs::read_to_string(
        root().join("crates/amplihack-workflows/tests/support/canonical_staging.rs"),
    )
    .unwrap();
    let tracked = tracked_python();
    for fixture in [ADAPTER, RECEIPTS] {
        assert_eq!(
            adapter_violation(&src, fixture, &tracked),
            None,
            "{fixture}"
        );
    }
}

#[test]
fn the_adapter_guard_fails_when_an_adapter_outlives_its_user() {
    let both: std::collections::BTreeSet<String> = [ADAPTER, LAST_USER]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let installs = "include_str!(\"../fixtures/canonical_staging/private_adapter.py\")";
    assert_eq!(adapter_violation(installs, ADAPTER, &both), None);
    assert_eq!(
        adapter_violation("no adapter", ADAPTER, &Default::default()),
        None
    );
    let only_adapter: std::collections::BTreeSet<String> = [ADAPTER.to_owned()].into();
    assert!(adapter_violation(installs, ADAPTER, &only_adapter).is_some());
    assert!(adapter_violation(installs, ADAPTER, &Default::default()).is_some());
}

#[test]
fn the_fixture_does_not_require_the_evidence_variable() {
    let src = fs::read_to_string(
        root().join("crates/amplihack-workflows/tests/support/canonical_staging.rs"),
    )
    .unwrap();
    assert!(
        !src.contains("expect(\"set CANONICAL_STAGING_EVIDENCE_DIR"),
        "Fixture must fall back to an owned tempdir"
    );
}
