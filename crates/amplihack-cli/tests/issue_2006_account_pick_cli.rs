//! Issue #2006: the command contract of `amplihack account pick`
//! (docs/reference/account-pick.md, "Synopsis" to "Exit codes and
//! messages"), run through the real binary against fake Codex accounts.
//! See `issue_2006_account_pick/fixture.rs` for the fixture.

#![cfg(unix)]

use crate::issue_2006_fixture::{Fx, stderr, stdout, text};
use std::collections::BTreeSet;

fn pick(fx: &Fx, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["account", "pick", "--cli", "codex"];
    args.extend_from_slice(extra);
    fx.cmd(&args).output().unwrap()
}

#[test]
fn help_lists_the_options() {
    let fx = Fx::new();
    let out = fx.cmd(&["account", "pick", "--help"]).output().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let help = stdout(&out);
    for opt in ["--cli", "--json", "--list", "--floor"] {
        assert!(help.contains(opt), "--help lacks {opt}:\n{help}");
    }
}

#[test]
fn usage_errors_exit_2() {
    let fx = Fx::with_80_50_20();
    for args in [
        vec!["account", "pick"],
        vec!["account", "pick", "--cli", "gemini"],
        vec!["account", "pick", "--cli", "codex", "--json", "--list"],
        vec!["account", "pick", "--cli", "codex", "--floor", "-1"],
        vec!["account", "pick", "--cli", "codex", "--floor", "NaN"],
        vec!["account", "pick", "--cli", "codex", "--floor", "inf"],
        vec!["account", "pick", "--cli", "codex", "--floor", "1000000001"],
        vec!["account", "pick", "--cli", "codex", "--floor", "lots"],
    ] {
        let out = fx.cmd(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}\n{}", text(&out));
    }
    assert!(fx.lease_files().is_empty(), "a usage error wrote a lease");
}

#[test]
fn a_plain_pick_prints_the_best_home_and_writes_a_null_pid_lease() {
    let fx = Fx::with_80_50_20();
    let out = pick(&fx, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(stdout(&out), format!("{}\n", fx.account("a").display()));
    let leases = fx.lease_json();
    assert_eq!(leases.len(), 1, "{leases:?}");
    assert_eq!(leases[0]["version"], 1);
    assert_eq!(leases[0]["cli"], "codex");
    assert_eq!(leases[0]["home"], fx.account("a").to_str().unwrap());
    assert!(
        leases[0]["pid"].is_null(),
        "a standalone pick records no pid"
    );
    assert!(leases[0]["created_at"].is_i64());
    fx.assert_credentials_untouched();
}

#[test]
fn json_has_the_fixed_key_set_on_one_line() {
    let fx = Fx::with_80_50_20();
    let out = pick(&fx, &["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let s = stdout(&out);
    assert_eq!(s.trim_end().lines().count(), 1, "one line: {s}");
    let v: serde_json::Value = serde_json::from_str(s.trim_end()).unwrap();
    let keys: BTreeSet<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    let want: BTreeSet<&str> = [
        "cli",
        "account",
        "home",
        "live_headroom",
        "running_jobs",
        "active_leases",
        "effective_headroom",
        "job_cost_pct",
        "read_at",
        "error",
        "candidates",
    ]
    .into();
    assert_eq!(keys, want);
    assert_eq!(v["cli"], "codex");
    assert_eq!(v["account"], "a");
    assert_eq!(v["home"], fx.account("a").to_str().unwrap());
    assert_eq!(v["live_headroom"], 80.0);
    assert_eq!(v["running_jobs"], 0);
    assert_eq!(v["active_leases"], 0);
    assert_eq!(v["effective_headroom"], 80.0);
    assert_eq!(v["job_cost_pct"], 15.0);
    assert!(v["read_at"].is_i64());
    assert!(v["error"].is_null());
    let cands = v["candidates"].as_array().unwrap();
    let names: Vec<&str> = cands
        .iter()
        .map(|c| c["account"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["a", "b", "c"]);
    let ckeys: BTreeSet<&str> = cands[0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let cwant: BTreeSet<&str> = [
        "account",
        "home",
        "live_headroom",
        "running_jobs",
        "active_leases",
        "effective_headroom",
        "usable",
        "note",
    ]
    .into();
    assert_eq!(ckeys, cwant);
    assert_eq!(fx.lease_files().len(), 1, "--json writes a lease");
}

#[test]
fn json_shows_null_headroom_for_an_account_with_no_reading() {
    let fx = Fx::new();
    fx.used("a", 20);
    fx.used("b", 50); // c is not signed in
    let out = pick(&fx, &["--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let c = &v["candidates"][2];
    assert_eq!(c["account"], "c", "unusable candidates come last");
    assert!(c["live_headroom"].is_null());
    assert!(c["effective_headroom"].is_null());
    assert_eq!(c["usable"], false);
    assert_eq!(c["note"], "not signed in");
}

#[test]
fn list_uses_the_codex_pick_format_and_writes_no_lease() {
    let fx = Fx::new();
    fx.used("a", 20);
    fx.used("b", 50);
    let out = pick(&fx, &["--list"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let lines: Vec<String> = stdout(&out).lines().map(str::to_owned).collect();
    assert_eq!(
        lines,
        [
            format!("  80.0  {}  (used 20%)", fx.account("a").display()),
            format!("  50.0  {}  (used 50%)", fx.account("b").display()),
            format!("  -1.0  {}  (not signed in)", fx.account("c").display()),
        ]
    );
    assert!(fx.lease_files().is_empty(), "--list wrote a lease");
}

#[test]
fn list_exits_0_when_nothing_is_usable() {
    let fx = Fx::new();
    let out = pick(&fx, &["--list"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(stdout(&out).lines().count(), 3);
}

#[test]
fn nothing_usable_exits_1_with_the_reason() {
    let fx = Fx::new();
    let out = pick(&fx, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(out.stdout.is_empty());
    assert!(
        stderr(&out).contains(
            "amplihack account pick: no usable codex account (none signed in or readable)"
        ),
        "{}",
        text(&out)
    );
    assert!(fx.lease_files().is_empty());
}

#[test]
fn nothing_usable_in_json_sets_error_and_still_lists_candidates() {
    let fx = Fx::new();
    let out = pick(&fx, &["--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["error"],
        "no usable codex account (none signed in or readable)"
    );
    for k in [
        "account",
        "home",
        "live_headroom",
        "running_jobs",
        "active_leases",
        "effective_headroom",
    ] {
        assert!(v[k].is_null(), "{k} must be null on failure: {v}");
    }
    assert_eq!(v["candidates"].as_array().unwrap().len(), 3);
}

#[test]
fn no_accounts_exits_1_with_no_accounts_found() {
    let fx = Fx::new();
    let empty = fx.tmp.path().join("empty-root");
    std::fs::create_dir(&empty).unwrap();
    let out = fx
        .cmd(&["account", "pick", "--cli", "codex"])
        .env("CODEX_PROFILES_ROOT", &empty)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        stderr(&out).contains("no usable codex account (no accounts found)"),
        "{}",
        text(&out)
    );
}

#[test]
fn the_codex_pick_caller_keeps_the_original_wording() {
    let fx = Fx::new();
    let out = fx
        .cmd(&["account", "pick", "--cli", "codex"])
        .env("AMPLIHACK_ACCOUNT_PICK_CALLER", "codex-pick")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        stderr(&out)
            .contains("codex-pick: no signed-in profile has headroom above the credit floor"),
        "{}",
        text(&out)
    );
}

#[test]
fn an_unsafe_lease_directory_exits_1_with_empty_candidates() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::with_80_50_20();
    std::fs::create_dir(&fx.leases).unwrap();
    std::fs::set_permissions(&fx.leases, std::fs::Permissions::from_mode(0o777)).unwrap();
    let out = pick(&fx, &["--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["error"].is_string());
    assert_eq!(v["candidates"], serde_json::json!([]));
    let plain = pick(&fx, &[]);
    assert_eq!(plain.status.code(), Some(1));
    assert!(
        stderr(&plain).starts_with("amplihack account pick: "),
        "{}",
        text(&plain)
    );
    let mode = std::fs::metadata(&fx.leases).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o777, "the picker must not repair permissions");
}

#[test]
fn the_off_switch_does_not_affect_the_standalone_command() {
    let fx = Fx::with_80_50_20();
    let out = fx
        .cmd(&["account", "pick", "--cli", "codex"])
        .env("AMPLIHACK_ACCOUNT_PICK", "off")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(stdout(&out).trim_end(), fx.account("a").to_str().unwrap());
}

#[test]
fn floor_is_ignored_for_claude_with_a_warning() {
    let fx = Fx::new();
    let out = fx
        .cmd(&[
            "account", "pick", "--cli", "claude", "--floor", "10", "--list",
        ])
        .env(
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
            fx.tmp.path().join("no-profiles"),
        )
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(2), "{}", text(&out));
    assert!(stderr(&out).contains("--floor"), "{}", text(&out));
}

#[test]
fn floor_decides_whether_a_full_account_with_credits_is_usable() {
    // The fake reports no credits, so a full account is unusable at any
    // floor; the option is accepted and the full account stays last.
    let fx = Fx::new();
    fx.used("a", 100);
    fx.used("b", 50);
    let out = pick(&fx, &["--floor", "0", "--list"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let first = stdout(&out).lines().next().unwrap().to_string();
    assert!(first.contains(fx.account("b").to_str().unwrap()), "{first}");
}

/// E2 at the binary level: ten `account pick` processes started together
/// spread 6/3/1 over accounts at 80, 50 and 20.
#[test]
fn ten_simultaneous_processes_spread_6_3_1() {
    let fx = Fx::with_80_50_20();
    let children: Vec<_> = (0..10)
        .map(|_| {
            fx.cmd(&["account", "pick", "--cli", "codex"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for c in children {
        let out = c.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        let home = stdout(&out).trim_end().to_string();
        let name = std::path::Path::new(&home)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        *counts.entry(name).or_default() += 1;
    }
    let want: std::collections::BTreeMap<String, usize> =
        [("a".into(), 6), ("b".into(), 3), ("c".into(), 1)].into();
    assert_eq!(counts, want);
    assert_eq!(fx.lease_files().len(), 10);
    fx.assert_credentials_untouched();
}

#[test]
fn output_never_contains_credential_sentinels() {
    let fx = Fx::with_80_50_20();
    for extra in [&[][..], &["--json"], &["--list"]] {
        let out = pick(&fx, extra);
        let all = text(&out);
        assert!(!all.contains("SENTINEL-TOKEN-2006"), "{all}");
    }
    fx.assert_credentials_untouched();
}
