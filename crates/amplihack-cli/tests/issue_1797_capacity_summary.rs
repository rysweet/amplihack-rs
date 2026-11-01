//! Issue #1797: the capacity summary of `profile usage`, `--summary-json`,
//! the usage history and the `capacity:` line of `rotate status`, end to end
//! through the binary with the rotate `Fixture` (temporary HOME and profile
//! root, a fake `claude`, `isolated(..)`; never a real `claude` or profile
//! root).
//!
//! The fixture: `work` is live in `~/.claude` at 90/40, `spare` 10/20 and
//! `extra` 50/50. A wrapper around the fake `claude` counts every start in
//! `$RECORD_DIR/starts`, so a run that should start nothing can be checked.
//!
//! Contract:
//! - The summary adds no probe: a table run, a `--json` run and a
//!   `--summary-json` run start `claude` the same number of times, one
//!   `/usage` per profile.
//! - Six lines follow a blank line after the table. `--json` is unchanged
//!   and has no summary; `--summary-json` holds the `--json` array.
//! - History is written only when `.auto-switch.d` exists, no names were
//!   given, no `--raw` and no credential variable; it never holds an e-mail
//!   address, even one Claude printed.
//! - `rotate status` prints `capacity:` from the history and starts nothing.

#![cfg(target_os = "linux")]

use crate::support::exec_stub::write_executable;
use crate::support::profile_rotate::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Output;

const THRESHOLD_ENV: &str = "AMPLIHACK_PROFILE_USAGE_USABLE_SESSION_PCT";
const BANNER: &str = "You are currently using your subscription to power your Claude Code usage";

/// Wraps the fixture's fake `claude` so every start is counted.
fn counting(f: &Fixture) {
    let real = f.tools.join("claude.real");
    fs::rename(f.tools.join("claude"), &real).unwrap();
    write_executable(
        &f.tools.join("claude"),
        format!(
            "#!/bin/sh\necho start >> \"$RECORD_DIR/starts\"\nexec '{}' \"$@\"\n",
            real.display()
        ),
        0o755,
    );
}

fn starts(f: &Fixture) -> usize {
    fs::read_to_string(f.records.join("starts"))
        .unwrap_or_default()
        .lines()
        .count()
}

fn state_dir(f: &Fixture) -> PathBuf {
    f.root.join(".auto-switch.d")
}

fn history(f: &Fixture) -> PathBuf {
    state_dir(f).join("usage-history.jsonl")
}

fn history_lines(f: &Fixture) -> Vec<String> {
    fs::read_to_string(history(f))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Turns recording on the way `profile usage` checks for it: a private
/// `.auto-switch.d`.
fn recording_on(f: &Fixture) {
    fs::create_dir_all(state_dir(f)).unwrap();
    fs::set_permissions(state_dir(f), fs::Permissions::from_mode(0o700)).unwrap();
}

fn usage(f: &Fixture, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut all = vec!["profile", "usage"];
    all.extend_from_slice(args);
    let mut c = f.cmd(&all);
    for (k, v) in env {
        c.env(k, v);
    }
    let out = c.output().unwrap();
    assert_no_secrets(&out);
    out
}

fn usage_ok(f: &Fixture, args: &[&str], env: &[(&str, &str)]) -> String {
    let out = usage(f, args, env);
    assert!(
        out.status.success(),
        "{}\n{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    text(&out.stdout)
}

/// Claude's `/usage` output for `p`, with an e-mail address in it.
fn usage_with_email(f: &Fixture, p: &str, session: u32, week: u32) {
    fs::write(
        f.canned.join(format!("{}.out", email(p))),
        format!(
            "{BANNER}\nLogged in as {}\n\nCurrent session: {session}% used\n\
             Current week (all models): {week}% used\n",
            email(p)
        ),
    )
    .unwrap();
}

fn summary_lines(stdout: &str) -> Vec<&str> {
    let (_, block) = stdout
        .split_once("\n\nweekly headroom")
        .unwrap_or_else(|| panic!("no summary after a blank line: {stdout}"));
    let mut v = vec!["weekly headroom"];
    v.extend(block.lines().skip(1));
    v
}

#[test]
fn the_summary_follows_the_table_and_adds_no_probe() {
    let f = Fixture::new();
    counting(&f);
    let table = usage_ok(&f, &[], &[]);
    let after_table = starts(&f);
    for p in PROFILES {
        assert_eq!(f.usage_probes(p), 1, "{p} probed once");
    }
    // 60 + 80 + 50 points left of 300.
    assert!(
        table.contains("\n\nweekly headroom      1.90 login-weeks of 3 (63%)\n"),
        "{table}"
    );
    let lines = summary_lines(&table);
    assert_eq!(lines.len(), 6, "{table}");
    assert!(
        table.contains("\nusable now           3 of 3 logins (session < 95% and weekly < 100%)\n")
    );
    assert!(table.contains("\nnext session reset   none\n"), "{table}");
    assert!(
        table.contains("\nrunway               unknown\n"),
        "{table}"
    );
    assert!(table.contains(
        "\nunknown              0 profiles (readings timed out or failed; excluded from all figures)\n"
    ));
    assert!(
        table.find("PROFILE").unwrap() < table.find("weekly headroom").unwrap(),
        "table first"
    );

    usage_ok(&f, &["--json"], &[]);
    let after_json = starts(&f) - after_table;
    usage_ok(&f, &["--summary-json"], &[]);
    let after_summary = starts(&f) - after_table - after_json;
    assert_eq!(
        after_table, after_json,
        "the table run starts no extra claude"
    );
    assert_eq!(
        after_summary, after_json,
        "--summary-json starts no extra claude"
    );
}

#[test]
fn json_has_no_summary_and_summary_json_holds_the_same_array() {
    let f = Fixture::new();
    let json: serde_json::Value = serde_json::from_str(&usage_ok(&f, &["--json"], &[])).unwrap();
    assert!(json.is_array(), "--json stays an array");
    let both: serde_json::Value =
        serde_json::from_str(&usage_ok(&f, &["--summary-json"], &[])).unwrap();
    assert_eq!(both["profiles"], json, "the same rows in the same order");
    let at = both["at"].as_str().unwrap();
    assert!(at.ends_with('Z') && at.len() == 20, "{at}");
    let s = &both["summary"];
    assert_eq!(s["valid_readings"], 3);
    assert_eq!(s["weekly_headroom_login_weeks"], 1.9);
    assert_eq!(s["weekly_headroom_pct"], 63.3);
    assert_eq!(s["usable_now"], 3);
    assert_eq!(s["unknown"], serde_json::json!([]));
    assert!(s["rate_login_weeks_per_hour"].is_null());
    assert!(s["runway_hours"].is_null());
}

#[test]
fn a_timed_out_profile_is_unknown_never_zero_or_one_hundred() {
    let f = Fixture::new();
    fs::write(f.canned.join(format!("{}.slow", email("extra"))), "").unwrap();
    let out = usage_ok(&f, &[], &[("AMPLIHACK_PROFILE_USAGE_TIMEOUT_SECS", "2")]);
    // work 60 + spare 80 of 200; extra is out of every figure.
    assert!(
        out.contains("\nweekly headroom      1.40 login-weeks of 2 (70%)\n"),
        "{out}"
    );
    assert!(out.contains("\nunknown              1 profile ("), "{out}");
}

#[test]
fn the_threshold_flag_and_variable_change_usable_now() {
    let f = Fixture::new();
    let out = usage_ok(&f, &["--usable-session-pct", "80"], &[]);
    assert!(
        out.contains("\nusable now           2 of 3 logins (session < 80% and weekly < 100%)\n"),
        "work is at 90%: {out}"
    );
    let out = usage_ok(&f, &[], &[(THRESHOLD_ENV, "50")]);
    assert!(
        out.contains("usable now           1 of 3 logins (session < 50%"),
        "{out}"
    );
    let out = usage(&f, &[], &[(THRESHOLD_ENV, "most")]);
    assert!(out.status.success());
    assert!(
        text(&out.stderr).contains(&format!(
            "warning: ignored {THRESHOLD_ENV} \"most\" (use 1 to 100); using 95"
        )),
        "{}",
        text(&out.stderr)
    );
    assert!(text(&out.stdout).contains("(session < 95% and weekly < 100%)"));
}

#[test]
fn conflicting_or_out_of_range_flags_are_usage_errors() {
    let f = Fixture::new();
    counting(&f);
    for args in [
        &["--summary-json", "--json"][..],
        &["--summary-json", "--raw"],
        &["--usable-session-pct", "0"],
        &["--usable-session-pct", "101"],
        &["--usable-session-pct", "most"],
    ] {
        let out = usage(&f, args, &[]);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    assert_eq!(starts(&f), 0, "a usage error probes nothing");
}

#[test]
fn raw_prints_no_summary() {
    let f = Fixture::new();
    let out = usage_ok(&f, &["--raw"], &[]);
    assert!(!out.contains("weekly headroom"), "{out}");
}

#[test]
fn history_is_written_only_for_a_whole_unfiltered_run_with_recording_on() {
    let f = Fixture::new();
    usage_ok(&f, &[], &[]);
    assert!(
        !state_dir(&f).exists(),
        "profile usage never creates .auto-switch.d"
    );

    recording_on(&f);
    usage_ok(&f, &[], &[]);
    let lines = history_lines(&f);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let names: Vec<&str> = v["profiles"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, ["extra", "spare", "work"]);
    assert_eq!(v["profiles"]["work"]["s"], 90);
    assert_eq!(v["profiles"]["work"]["w"], 40);
    assert_eq!(
        fs::metadata(history(&f)).unwrap().permissions().mode() & 0o777,
        0o600
    );

    usage_ok(&f, &["spare"], &[]);
    usage_ok(&f, &["--raw"], &[]);
    usage_ok(&f, &[], &[("ANTHROPIC_API_KEY", "sk-test-not-real")]);
    assert_eq!(
        history_lines(&f).len(),
        1,
        "filtered, raw and credential runs record nothing"
    );

    usage_ok(&f, &["--json"], &[]);
    usage_ok(&f, &["--summary-json"], &[]);
    assert_eq!(
        history_lines(&f).len(),
        3,
        "--json and --summary-json record"
    );
}

#[test]
fn an_email_claude_printed_reaches_neither_the_summary_nor_the_history() {
    let f = Fixture::new();
    recording_on(&f);
    for (p, s, w) in [("work", 90, 40), ("spare", 10, 20), ("extra", 50, 50)] {
        usage_with_email(&f, p, s, w);
    }
    let table = usage_ok(&f, &[], &[]);
    assert!(
        table.contains("weekly headroom      1.90 login-weeks of 3"),
        "{table}"
    );
    let both: serde_json::Value =
        serde_json::from_str(&usage_ok(&f, &["--summary-json"], &[])).unwrap();
    let summary = both["summary"].to_string();
    for p in PROFILES {
        assert!(!summary.contains(&email(p)), "{summary}");
    }
    assert!(!summary.contains('@'), "{summary}");
    let body = fs::read_to_string(history(&f)).unwrap();
    assert!(!body.contains('@'), "{body}");
    assert!(!body.contains("uuid-"), "{body}");
    assert!(!body.contains("SENTINEL"), "{body}");
}

#[test]
fn a_rotator_cycle_appends_one_line_of_the_profiles_it_measured() {
    let f = Fixture::new();
    f.ok(&["profile", "rotate"]);
    let lines = history_lines(&f);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let v: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let p = v["profiles"].as_object().unwrap();
    assert!(
        p.contains_key("work"),
        "the live login under its profile: {v}"
    );
    assert!(p.contains_key("spare") && p.contains_key("extra"), "{v}");
    assert!(!lines[0].contains('@'), "{}", lines[0]);
}

#[test]
fn rotate_status_prints_the_capacity_line_and_starts_nothing() {
    let f = Fixture::new();
    counting(&f);
    let status = |f: &Fixture| text(&f.ok(&["profile", "rotate", "status"]).stdout);

    let out = status(&f);
    assert!(out.contains("\ncapacity: no recorded cycle\n"), "{out}");

    recording_on(&f);
    let at = chrono::Utc::now() - chrono::Duration::minutes(3);
    let line = format!(
        r#"{{"at":"{}","profiles":{{"work":{{"s":20,"w":30,"w_reset":null}},"spare":{{"s":100,"w":68,"w_reset":null}}}}}}"#,
        at.format("%Y-%m-%dT%H:%M:%SZ")
    );
    fs::write(history(&f), format!("{line}\n")).unwrap();
    fs::set_permissions(history(&f), fs::Permissions::from_mode(0o600)).unwrap();

    let out = status(&f);
    let cap = out
        .lines()
        .find(|l| l.starts_with("capacity: "))
        .unwrap_or_else(|| panic!("no capacity line: {out}"));
    assert!(
        cap.starts_with(
            "capacity: headroom 1.02 login-weeks of 2 (51%), usable 1 of 2 logins, runway unknown (as of "
        ),
        "{cap}"
    );
    assert!(cap.ends_with(", 3m ago)"), "{cap}");
    let last = out.find("\nlast cycle: ").expect("last cycle line");
    assert!(
        last < out.find("\ncapacity: ").unwrap(),
        "after last cycle: {out}"
    );
    assert_eq!(starts(&f), 0, "status started claude");
}
