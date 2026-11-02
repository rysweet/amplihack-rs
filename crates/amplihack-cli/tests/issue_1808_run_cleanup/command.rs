//! The `amplihack cleanup` command surface: dry run on an empty HOME, the
//! JSON contract, sort order, exit codes and work-root validation (R6; AC16).

use super::fixture::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// AC16: a dry run on an empty HOME succeeds and creates nothing, not even
/// the default work root.
#[test]
fn ac16_dry_run_on_empty_home_does_not_create_the_work_root() {
    let fx = Fx::new();
    let before = snapshot(&fx.home);
    let out = fx.cleanup_with(&[], |c| {
        c.env_remove("AMPLIHACK_WORK_ROOT");
    });
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(!fx.home.join(".amplihack/work").exists());
    assert_eq!(
        snapshot(&fx.home),
        before,
        "a dry run writes nothing under HOME"
    );
}

#[test]
fn ac16_dry_run_with_missing_custom_work_root_does_not_create_it() {
    let fx = Fx::new();
    let missing = fx.root.join("not-yet");
    let out = fx.cleanup_with(&["--format", "json"], |c| {
        c.env("AMPLIHACK_WORK_ROOT", &missing);
    });
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    assert!(!missing.exists());
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["work_root"], missing.display().to_string());
    assert_eq!(report["dry_run"], true);
}

/// The JSON contract: top-level `dry_run`, `work_root`, `entries`; each entry
/// has exactly the six documented keys; entries are sorted by category then
/// path; two dry runs give identical output.
#[test]
fn json_report_has_documented_shape_and_stable_order() {
    let fx = Fx::new();
    for name in ["b-target", "a-target"] {
        let t = fx.target(name, 64);
        age_tree(&t, 72);
    }
    let log_dir = fx.home.join(".amplihack/recipe-logs");
    fs::create_dir_all(&log_dir).unwrap();
    for name in ["z.log", "a.log"] {
        let p = log_dir.join(name);
        fs::write(&p, "log").unwrap();
        set_mtime_days_ago(&p, 60);
    }

    let first = fx.cleanup(&["--format", "json"]);
    let second = fx.cleanup(&["--format", "json"]);
    assert_eq!(first.stdout, second.stdout, "output must be deterministic");
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();

    let mut keys: Vec<_> = report.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["dry_run", "entries", "work_root"]);
    assert_eq!(report["work_root"], fx.work.display().to_string());

    let list = entries(&report);
    assert!(list.len() >= 4, "{report:#}");
    for e in &list {
        let mut k: Vec<_> = e.as_object().unwrap().keys().cloned().collect();
        k.sort();
        assert_eq!(
            k,
            [
                "action",
                "category",
                "path",
                "reason",
                "run_id",
                "size_bytes"
            ],
            "{e}"
        );
        assert!(e["size_bytes"].is_u64() || e["size_bytes"].is_null());
    }
    let order: Vec<(String, String)> = list
        .iter()
        .map(|e| {
            (
                e["category"].as_str().unwrap().into(),
                e["path"].as_str().unwrap().into(),
            )
        })
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "entries sorted by category, then path");
}

/// Text output shows category, action, a human-readable size and the path,
/// with the reason on its own line.
#[test]
fn text_report_lists_category_action_size_path_and_reason() {
    let fx = Fx::new();
    let t = fx.target("stray-old", 2 * 1024 * 1024);
    age_tree(&t, 72);
    let out = fx.cleanup(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", describe(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.contains(t.to_str().unwrap()))
        .unwrap_or_else(|| panic!("no line for the target:\n{text}"));
    assert!(line.starts_with("untagged-target"), "{line}");
    assert!(line.contains("would-remove"), "{line}");
    assert!(line.contains("MiB"), "size is human-readable: {line}");
    assert!(text.contains("reason:"), "{text}");
}

/// Exit code 2 for bad usage.
#[test]
fn bad_usage_exits_2() {
    let fx = Fx::new();
    let help = fx.cleanup(&["--help"]);
    assert_eq!(
        help.status.code(),
        Some(0),
        "the command exists: {}",
        describe(&help)
    );
    for args in [
        &["--only", "not-a-category"][..],
        &["--only", "run-tmp", "--skip-logs"],
        &["--format", "xml"],
        &["--keep-hours", "87601"],
        &["--log-days", "36501"],
        &["--keep-hours", "-1"],
        &["restore"],
    ] {
        let out = fx.cleanup(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", describe(&out));
    }
}

/// Every documented category is accepted by `--only`.
#[test]
fn every_category_is_accepted_by_only() {
    let fx = Fx::new();
    for cat in [
        "run-tmp",
        "run-worktree",
        "run-target",
        "untagged-target",
        "tmp-leftover",
        "recipe-log",
        "session-log",
    ] {
        let out = fx.cleanup(&["--only", cat]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "--only {cat}: {}",
            describe(&out)
        );
    }
}

/// `--only` limits the report to the named categories.
#[test]
fn only_limits_categories() {
    let fx = Fx::new();
    let t = fx.target("stray-old", 64);
    age_tree(&t, 72);
    let leftover = fx.tmp.join("replay-old");
    fs::create_dir_all(&leftover).unwrap();
    age_tree(&leftover, 72);
    let report = fx.cleanup_json(&["--only", "tmp-leftover"]);
    for e in entries(&report) {
        assert_eq!(e["category"], "tmp-leftover", "{e}");
    }
}

/// Work-root validation: refused roots are a fatal error (exit 1, message
/// prefixed `amplihack cleanup:`), and nothing under them is touched.
#[test]
fn unsafe_work_roots_are_refused() {
    let fx = Fx::new();
    let group_writable = fx.root.join("gw");
    fs::create_dir_all(&group_writable).unwrap();
    fs::set_permissions(&group_writable, fs::Permissions::from_mode(0o770)).unwrap();
    let cases = [
        "relative/path".to_string(),
        "/".to_string(),
        fx.home.display().to_string(),
        fx.root.display().to_string(),
        "/usr".to_string(),
        "/etc".to_string(),
        fx.tmp.display().to_string(),
        fx.repo.display().to_string(),
        group_writable.display().to_string(),
    ];
    for root in &cases {
        let out = fx.cleanup_with(&["--apply"], |c| {
            c.env("AMPLIHACK_WORK_ROOT", root);
        });
        assert_eq!(out.status.code(), Some(1), "{root}: {}", describe(&out));
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("amplihack cleanup:"), "{root}: {err}");
    }
    assert!(fx.repo.join("file.txt").exists());
    assert!(!group_writable.join(".cleanup.lock").exists());
}

/// `amplihack hygiene cleanup` is unchanged: its JSON items have no
/// `size_bytes` key. A regression guard; it passes before and after.
#[test]
fn hygiene_cleanup_output_has_no_size_bytes_key() {
    let fx = Fx::new();
    let mut c = fx.amplihack();
    c.args(["hygiene", "cleanup", "--all", "--format", "json"])
        .current_dir(&fx.repo);
    let out = c.output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("size_bytes"));
}

/// §8 test 13: a symlink planted at the lock-file path is refused; the file
/// it points at is neither created nor changed.
#[test]
fn planted_symlink_at_lock_path_is_refused() {
    let fx = Fx::new();
    let sentinel = fx.root.join("sentinel");
    fs::write(&sentinel, "precious").unwrap();
    std::os::unix::fs::symlink(&sentinel, fx.work.join(".cleanup.lock")).unwrap();
    let dangling = fx.root.join("never-created");

    let out = fx.cleanup(&["--apply"]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "precious");

    fs::remove_file(fx.work.join(".cleanup.lock")).unwrap();
    std::os::unix::fs::symlink(&dangling, fx.work.join(".cleanup.lock")).unwrap();
    let out = fx.cleanup(&["--apply"]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    assert!(
        !dangling.exists(),
        "the lock must not be created through a symlink"
    );
}
