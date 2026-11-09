//! Issue #2019: no test asserts on elapsed wall-clock time.
//!
//! A loaded host can stretch real time without limit, so a test that asserts
//! on `elapsed()` or on the difference of two `Instant`s fails whenever the
//! host is busy. This guard scans `crates/*/src/**/test*.rs`,
//! `crates/*/tests/**` and `tests/**` and fails on any line that uses
//! `elapsed()` or `duration_since(` inside an `assert` or a comparison.
//!
//! Exempt:
//! - a file containing `impl Clock for` (it measures injected time);
//! - `issue_1973_wait_helpers.rs` and this file;
//! - an `ALLOWLIST` entry `(path, follow-up issue)`.
//!
//! The allowlist can only shrink: an entry whose file no longer matches
//! fails the guard. The scan does not follow symlinks and skips `target/`,
//! `worktrees/` and `.git/`. The matcher is checked against inline samples.
//!
//! See docs/howto/keep-tests-passing-under-host-load.md, "The wall-clock scan
//! guard".

use std::fs;
use std::path::{Path, PathBuf};

/// Files that still assert on wall-clock time, each with the follow-up issue
/// that tracks it. Add an entry only for a test the current PR does not
/// touch, and only with a filed issue.
const ALLOWLIST: &[(&str, u32)] = &[
    (
        "crates/amplihack-cli/src/commands/agent/tests_supervise.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/auto_switch/tests_home_line.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/auto_switch/tests_record.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/rotate/tests_autostart.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/rotate/tests_issue_1886.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/rotate/tests_measure.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/rotate/tests_owner.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/rotate/tests_platform_identify.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/rotate/tests_switch.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/src/commands/profile/tests_usage_cmd.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/tests/issue_1570_run_output_stdin.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/tests/issue_1578_profile_usage.rs",
        2121,
    ),
    ("crates/amplihack-cli/tests/issue_1606_detach.rs", 2121),
    (
        "crates/amplihack-cli/tests/issue_1746_daily_archive_rotator.rs",
        2121,
    ),
    (
        "crates/amplihack-cli/tests/issue_1760_agent_tmux_scope.rs",
        2121,
    ),
    ("crates/amplihack-cli/tests/issue_1794_wait.rs", 2121),
    (
        "crates/amplihack-cli/tests/issue_1803_rotation_owner.rs",
        2121,
    ),
    ("crates/amplihack-cli/tests/profile_overflow.rs", 2121),
    ("crates/amplihack-cli/tests/profile_usage_direct.rs", 2121),
    ("crates/amplihack-cli/tests/runner_probe_cleanup.rs", 2121),
    ("crates/amplihack-cli/tests/runner_probe_lifecycle.rs", 2121),
    (
        "crates/amplihack-cli/tests/runner_validation_contract.rs",
        2121,
    ),
    ("crates/amplihack-utils/tests/idle_watchdog.rs", 2121),
    ("crates/amplihack-utils/tests/secure_files_private.rs", 2121),
    (
        "tests/integration/issue_1764_git_never_prompts_test.rs",
        2121,
    ),
];

/// Files whose `elapsed()` reads injected virtual time through a seam other
/// than `Clock`, each with the text that proves it. An entry whose proof is
/// gone fails the guard, so the exemption cannot outlive the fake.
const VIRTUAL_TIME: &[(&str, &str)] = &[
    (
        "crates/amplihack-cli/src/commands/profile/limit_wait/tests_wait.rs",
        "impl WaitDeps for Fake",
    ),
    (
        "crates/amplihack-cli/src/commands/profile/limit_wait/tests_wait_ends.rs",
        "use super::tests_wait::{Fake",
    ),
];

const THIS_FILE: &str = "issue_2019_no_wall_clock_asserts.rs";
const EXEMPT_NAMES: &[&str] = &[THIS_FILE, "issue_1973_wait_helpers.rs"];
const SKIP_DIRS: &[&str] = &["target", "worktrees", ".git"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// Whether one source line asserts on, or compares, wall-clock time.
fn is_wall_clock_check(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with("//") {
        return false;
    }
    let code = match code.find("//") {
        Some(i) if !code[..i].contains('"') => &code[..i],
        _ => code,
    };
    let measures = code.contains("elapsed()") || code.contains("duration_since(");
    if !measures {
        return false;
    }
    let asserts = code.contains("assert");
    let compares = [" < ", " > ", " <= ", " >= ", ".lt(", ".gt(", ".le(", ".ge("]
        .iter()
        .any(|op| code.contains(op));
    asserts || compares
}

fn is_test_source(rel: &Path) -> bool {
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let Some(name) = parts.last() else {
        return false;
    };
    if !name.ends_with(".rs") {
        return false;
    }
    match parts.first().map(String::as_str) {
        Some("tests") => true,
        Some("crates") if parts.len() >= 4 => match parts[2].as_str() {
            "tests" => true,
            "src" => name.starts_with("test"),
            _ => false,
        },
        _ => false,
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // symlink_metadata: a symlink is never followed, file or directory.
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if meta.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) {
                walk(root, &path, out);
            }
        } else if meta.is_file() {
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if is_test_source(&rel) {
                out.push(rel);
            }
        }
    }
}

/// `(relative path, 1-based line, text)` of every non-exempt match.
fn scan(root: &Path) -> Vec<(String, usize, String)> {
    let mut files = Vec::new();
    for top in ["crates", "tests"] {
        walk(root, &root.join(top), &mut files);
    }
    files.sort();
    let mut hits = Vec::new();
    for rel in files {
        let name = rel.file_name().unwrap().to_string_lossy();
        if EXEMPT_NAMES.contains(&name.as_ref()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(root.join(&rel)) else {
            continue;
        };
        if text.contains("impl Clock for") {
            // Injected time is exempt, real time in the same file is not.
            for (i, line) in real_instant_checks(&text) {
                hits.push((rel.display().to_string(), i, line));
            }
            continue;
        }
        let rel_s = rel.display().to_string();
        if VIRTUAL_TIME
            .iter()
            .any(|(p, proof)| *p == rel_s && text.contains(proof))
        {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if is_wall_clock_check(line) {
                hits.push((rel.display().to_string(), i + 1, line.trim().to_string()));
            }
        }
    }
    hits
}

/// Names bound to the real clock: `let <name> = Instant::now();` (also
/// `std::time::Instant::now()`).
fn real_instant_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let Some(rest) = l.strip_prefix("let ") else {
            continue;
        };
        let rest = rest.strip_prefix("mut ").unwrap_or(rest);
        let Some((name, value)) = rest.split_once('=') else {
            continue;
        };
        let name = name.split(':').next().unwrap_or("").trim();
        let value = value.trim();
        if (value.starts_with("Instant::now()") || value.starts_with("std::time::Instant::now()"))
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            names.push(name.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// In a file that also holds an injected clock: `(1-based line, text)` of
/// each wall-clock check on a name bound to `Instant::now()`.
fn real_instant_checks(text: &str) -> Vec<(usize, String)> {
    let names = real_instant_names(text);
    text.lines()
        .enumerate()
        .filter(|(_, l)| is_wall_clock_check(l))
        .filter(|(_, l)| {
            names.iter().any(|n| {
                l.contains(&format!("{n}.elapsed()")) || l.contains(&format!("duration_since({n})"))
            })
        })
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .collect()
}

// ── the matcher on inline samples ───────────────────────────────────────────

#[test]
fn the_matcher_flags_elapsed_in_an_assert() {
    assert!(is_wall_clock_check(
        "    assert!(start.elapsed() < Duration::from_secs(3));"
    ));
    assert!(is_wall_clock_check(
        "assert!(t0.elapsed() >= Duration::from_millis(200), \"too fast\");"
    ));
    assert!(is_wall_clock_check(
        "debug_assert!(started.elapsed().as_secs() == 0);"
    ));
}

#[test]
fn the_matcher_flags_elapsed_in_a_comparison() {
    assert!(is_wall_clock_check("    if start.elapsed() > LIMIT {"));
    assert!(is_wall_clock_check("let late = t.elapsed() >= budget;"));
    assert!(is_wall_clock_check(
        "    let ok = Instant::now().duration_since(start) < Duration::from_secs(25);"
    ));
}

#[test]
fn the_matcher_ignores_measurements_that_decide_nothing() {
    assert!(!is_wall_clock_check("    let spent = start.elapsed();"));
    assert!(!is_wall_clock_check(
        "    eprintln!(\"took {:?}\", start.elapsed());"
    ));
    assert!(!is_wall_clock_check(
        "    // assert!(start.elapsed() < Duration::from_secs(3));"
    ));
    assert!(!is_wall_clock_check("    let v: Vec<u8> = Vec::new();"));
    assert!(!is_wall_clock_check("    assert!(a < b);"));
}

#[test]
fn a_clock_file_is_still_checked_for_real_instants() {
    let sample = "struct FakeClock;\n\
                  impl Clock for FakeClock {}\n\
                  fn t() {\n\
                  \x20   let start = Instant::now();\n\
                  \x20   let v = clock.elapsed();\n\
                  \x20   assert!(v >= LIMIT);\n\
                  \x20   assert!(start.elapsed() < COUNTER_WAIT);\n\
                  }\n";
    assert_eq!(real_instant_names(sample), ["start"]);
    let hits = real_instant_checks(sample);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].0, 7);
}

// ── the scan ────────────────────────────────────────────────────────────────

#[test]
fn the_scan_finds_test_sources_in_every_root() {
    let root = workspace_root();
    let mut files = Vec::new();
    for top in ["crates", "tests"] {
        walk(&root, &root.join(top), &mut files);
    }
    let has = |p: &str| files.iter().any(|f| f == Path::new(p));
    assert!(has("crates/amplihack-hooks/tests/limit_resume_fs.rs"));
    assert!(has(
        "crates/amplihack-cli/src/commands/profile/tests_usage_direct.rs"
    ));
    assert!(has("crates/amplihack-cli/tests/support/wait.rs"));
    assert!(
        files.iter().all(|f| !f.components().any(|c| {
            let c = c.as_os_str();
            c == "target" || c == "worktrees" || c == ".git"
        })),
        "target/, worktrees/ and .git/ are skipped"
    );
    assert!(
        !has("crates/amplihack-cli/src/commands/profile/usage_direct.rs"),
        "production files are not scanned"
    );
}

#[test]
fn no_test_asserts_on_wall_clock_time() {
    let root = workspace_root();
    let allowed: Vec<&str> = ALLOWLIST.iter().map(|(p, _)| *p).collect();
    let bad: Vec<String> = scan(&root)
        .into_iter()
        .filter(|(p, _, _)| !allowed.contains(&p.as_str()))
        .map(|(p, n, l)| format!("{p}:{n}: {l}"))
        .collect();
    assert!(
        bad.is_empty(),
        "these tests assert on wall-clock time; wait on an event or inject a \
         clock instead (issue #2019):\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_allowlist_only_shrinks() {
    let root = workspace_root();
    let hits = scan(&root);
    for (path, issue) in ALLOWLIST {
        assert!(*issue > 0, "{path}: every entry names its follow-up issue");
        assert!(
            hits.iter().any(|(p, _, _)| p == path),
            "{path} (#{issue}) no longer asserts on wall-clock time; remove it \
             from ALLOWLIST"
        );
    }
    let mut seen = std::collections::HashSet::new();
    for (path, _) in ALLOWLIST {
        assert!(seen.insert(*path), "{path} is listed twice");
    }
}

#[test]
fn every_virtual_time_exemption_still_holds_its_proof() {
    let root = workspace_root();
    for (path, proof) in VIRTUAL_TIME {
        let text = fs::read_to_string(root.join(path))
            .unwrap_or_else(|e| panic!("{path}: {e}; remove it from VIRTUAL_TIME"));
        assert!(
            text.contains(proof),
            "{path} no longer contains {proof:?}; its elapsed() may be real time"
        );
    }
}

/// The tests #2019, #1977 and #1998 name are fixed in this PR, so none may
/// sit on the allowlist.
#[test]
fn the_issue_tests_are_not_allowlisted() {
    for path in [
        "crates/amplihack-hooks/tests/limit_resume_fs.rs",
        "crates/amplihack-cli/src/commands/profile/tests_usage_direct.rs",
        "crates/amplihack-cli/src/commands/profile/rotate/tests_daemon.rs",
        "crates/amplihack-cli/src/commands/recipe/run/tests_execute.rs",
        "crates/amplihack-cli/src/commands/recipe/run/tests_teardown.rs",
        "crates/amplihack-cli/tests/issue_1659_limit_resume_gate.rs",
        "crates/amplihack-cli/tests/codex_frontdoor_contract.rs",
        "crates/amplihack-cli/tests/claude_code_plugin_manifest.rs",
        "crates/amplihack-cli/tests/issue_1811_governor.rs",
        "crates/amplihack-cli/tests/issue_1743_autodrive_budget.rs",
    ] {
        assert!(
            !ALLOWLIST.iter().any(|(p, _)| *p == path),
            "{path} is fixed by #2019 and must not be allowlisted"
        );
    }
}

// ── stub writers (ETXTBSY) ──────────────────────────────────────────────────

/// Every stub writer named in the how-to goes through `write_executable`,
/// which writes a temp name, closes it and renames it into place. Writing
/// the stub in place and then `chmod`ing it lets a sibling fork inherit the
/// write descriptor, and the exec then fails with ETXTBSY.
#[test]
fn named_stub_writers_use_write_executable() {
    let root = workspace_root();
    // (file, text that marks a chmod of a directory, not a stub)
    let files: &[(&str, &[&str])] = &[
        ("crates/amplihack-cli/src/commands/install/clone.rs", &[]),
        (
            "crates/amplihack-cli/src/commands/recipe/run/tests_teardown.rs",
            &[],
        ),
        (
            "crates/amplihack-cli/src/commands/recipe/run/tests_execute.rs",
            &[],
        ),
        (
            "crates/amplihack-cli/src/commands/install/tests/mermaid_cli_tests.rs",
            &[],
        ),
        (
            "crates/amplihack-cli/src/path_conflicts/tests.rs",
            &["&user_bin"],
        ),
        ("crates/amplihack-cli/src/commands/fleet/tests.rs", &[]),
        (
            "crates/amplihack-cli/src/commands/fleet/tests_issue_1570_reasoner.rs",
            &[],
        ),
        ("crates/amplihack-hooks/src/session_start/blarify.rs", &[]),
    ];
    let mut bad = Vec::new();
    for (rel, dir_marks) in files {
        let text = fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        for (i, line) in text.lines().enumerate() {
            let chmods_exec = line.contains("from_mode(0o755)")
                || line.contains("set_mode(0o755)")
                || line.contains("from_mode(0o775)")
                || line.contains("from_mode(0o777)");
            if chmods_exec && !dir_marks.iter().any(|m| line.contains(m)) {
                bad.push(format!("{rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "write these stubs with test_support::write_executable (ETXTBSY, \
         issue #2019):\n{}",
        bad.join("\n")
    );
}

// ── the probe knob reaches every isolated child ────────────────────────────

/// `isolated(..)` sets `AMPLIHACK_TEST_PROBE_WAIT=exit` after `env_clear`, so
/// a child's `--version` probe waits for the stub's exit, not 3 s.
#[test]
fn isolated_children_get_the_probe_wait_knob() {
    let root = workspace_root();
    let rel = "tests/support/isolated_amplihack.rs";
    let text = fs::read_to_string(root.join(rel)).unwrap();
    assert!(
        text.contains("\"AMPLIHACK_TEST_PROBE_WAIT\"") && text.contains("\"exit\""),
        "{rel} must set AMPLIHACK_TEST_PROBE_WAIT=exit on isolated children"
    );
}
