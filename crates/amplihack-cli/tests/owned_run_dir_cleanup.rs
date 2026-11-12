//! Real CLI refusal contracts. These tests intentionally remain unexecuted
//! until the complete guard and test isolation are committed (PR #2360).
//! Seeded historical reports are eligibility evidence, never ownership.

#![cfg(target_os = "linux")]

#[path = "common/run_fixture.rs"]
mod run_fixture;

use crate::isolated_amplihack::isolated;
use crate::support::amplihack_bin::amplihack_bin;
use crate::support::quiet_process::{child_remove_var, child_set_var, in_quiet_process};
use run_fixture::RunFixture;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

const ID: &str = "3f0c9a52-6d1e-4b8a-9f57-2c41d0e7a6b3";
const SHORT: &str = "3f0c9a526d1e";

fn in_fixture(name: &str, body: impl FnOnce(&RunFixture)) {
    in_quiet_process(module_path!(), name, || {
        let fx = RunFixture::new();
        struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in self.0.drain(..) {
                    match value {
                        Some(value) => child_set_var(key, value),
                        None => child_remove_var(key),
                    }
                }
            }
        }
        let values = fx.environment();
        let _restore = Restore(
            values
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect(),
        );
        for (key, value) in values {
            child_set_var(key, value);
        }
        body(&fx);
    });
}

fn seed(fx: &RunFixture, runtime: Option<&Path>) -> std::path::PathBuf {
    let dir = fx.work.join("runs").join(ID);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("owned-paths"), b"").unwrap();
    fs::write(
        dir.join("run.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "run_id": ID,
            "recipe": "legacy-cleanup-contract",
            "pid": 0,
            "pid_start_ticks": null,
            "cwd": fx.base,
            "started_at": "2000-01-01T00:00:00Z",
            "ended_at": "2000-01-01T00:00:01Z",
            "outcome": "success",
            "attempts": [],
            "steps": [],
            "log_path": null,
            "runtime_dir": runtime,
        }))
        .unwrap(),
    )
    .unwrap();
    dir
}

fn cleanup(fx: &RunFixture, apply: bool) -> Value {
    let mut command = isolated(amplihack_bin());
    command
        .current_dir(&fx.base)
        .envs(fx.environment())
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .args([
            "cleanup",
            "--only",
            "run-tmp",
            "--keep-hours",
            "0",
            "--format",
            "json",
        ]);
    if apply {
        command.arg("--apply");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "refusal is not an operational failure: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_skipped(report: &Value, path: &Path) {
    let entry = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"].as_str() == path.to_str())
        .unwrap_or_else(|| panic!("no entry for {}: {report}", path.display()));
    assert_eq!(entry["action"], "skipped", "{entry}");
    assert!(!entry["reason"].as_str().unwrap().is_empty());
}

#[test]
fn stale_cli_preserves_unmarked_default_and_shortened_runtime_directories() {
    in_fixture(
        "stale_cli_preserves_unmarked_default_and_shortened_runtime_directories",
        |fx| {
            let run = seed(fx, None);
            let default = run.join("runtime");
            let short = fx.work.join("r").join(SHORT);
            let default_sentinel = fx.sentinel(&default);
            let short_sentinel = fx.sentinel(&short);
            let sibling = fx.sentinel(&fx.work.join("r/unrelated"));
            let report = cleanup(fx, true);
            for path in [&default, &short] {
                assert_skipped(&report, path);
            }
            for sentinel in [&default_sentinel, &short_sentinel, &sibling] {
                fx.assert_sentinel(sentinel);
            }
            assert!(run.join("run.json").is_file());
        },
    );
}

#[test]
fn stale_cli_preserves_a_report_pointing_at_its_private_temp_root() {
    in_fixture(
        "stale_cli_preserves_a_report_pointing_at_its_private_temp_root",
        |fx| {
            seed(fx, Some(&fx.tmp));
            let sentinel = fx.sentinel(&fx.tmp);
            let report = cleanup(fx, true);
            assert_skipped(&report, &fx.tmp);
            fx.assert_sentinel(&sentinel);
        },
    );
}

#[test]
fn stale_cli_preserves_an_unmarked_external_runtime() {
    in_fixture("stale_cli_preserves_an_unmarked_external_runtime", |fx| {
        let runtime = fx.external.join(format!("ahr-{SHORT}"));
        seed(fx, Some(&runtime));
        let sentinel = fx.sentinel(&runtime);
        let report = cleanup(fx, true);
        assert_skipped(&report, &runtime);
        fx.assert_sentinel(&sentinel);
    });
}

#[test]
fn dry_sweep_does_not_advertise_deletion_of_an_unowned_runtime() {
    in_fixture(
        "dry_sweep_does_not_advertise_deletion_of_an_unowned_runtime",
        |fx| {
            let runtime = seed(fx, None).join("runtime");
            let sentinel = fx.sentinel(&runtime);
            let report = cleanup(fx, false);
            assert_eq!(report["dry_run"], true);
            assert_skipped(&report, &runtime);
            fx.assert_sentinel(&sentinel);
        },
    );
}
