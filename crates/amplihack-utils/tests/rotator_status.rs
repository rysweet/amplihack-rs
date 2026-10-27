//! Issue #1658 (A): `amplihack_utils::rotator_status`, the read-only view of
//! the rotator that the limit-resume hook uses. It resolves the profile root,
//! reads the auto-switch toggle and reads the rotate ledger's last cycle.
//!
//! The CLI owns the same rules (`root_config`, `auto_switch::config`,
//! `rotate::ledger`); the drift tests in amplihack-cli check the two agree.
//! These tests pin the hook-side contract.

#![cfg(unix)]

use amplihack_utils::rotator_status::{
    CycleStatus, FRESH_MARGIN_SECS, LIMIT_RESUME_TIMEOUT_SECS, MAX_WAIT, POLL_INTERVAL,
    SLOW_POLL_AFTER, SLOW_POLL_INTERVAL, auto_switch_on_from, ledger_path, parse_cycle, read_cycle,
    resolve_root_from, valid_live_name,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

const NOW: i64 = 1_792_592_000;

fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| OsString::from(v))
    }
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

fn private_file(p: &Path, bytes: &[u8]) {
    fs::write(p, bytes).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

// ── Constants ───────────────────────────────────────────────────────────────

#[test]
fn constants_match_the_documented_limits() {
    assert_eq!(MAX_WAIT, Duration::from_secs(7 * 24 * 60 * 60));
    assert_eq!(POLL_INTERVAL, Duration::from_secs(15));
    assert_eq!(SLOW_POLL_AFTER, Duration::from_secs(60 * 60));
    assert_eq!(SLOW_POLL_INTERVAL, Duration::from_secs(5 * 60));
    assert_eq!(FRESH_MARGIN_SECS, 60);
    assert_eq!(LIMIT_RESUME_TIMEOUT_SECS, 608_400);
}

#[test]
fn wait_plus_one_poll_fits_inside_the_registered_timeout() {
    assert!(
        (MAX_WAIT + SLOW_POLL_INTERVAL).as_secs() < LIMIT_RESUME_TIMEOUT_SECS,
        "the hook must decide before Claude Code kills it"
    );
}

// ── resolve_root_from ───────────────────────────────────────────────────────

#[test]
fn env_root_wins_over_pointer_and_default() {
    let home = tempfile::tempdir().unwrap();
    private_dir(&home.path().join(".amplihack"));
    private_file(
        &home.path().join(".amplihack/profile-root"),
        b"/somewhere/else\n",
    );
    let pairs = [("AMPLIHACK_CLAUDE_PROFILE_ROOT", "/env/root")];
    let env = env_of(&pairs);
    assert_eq!(
        resolve_root_from(env, Some(home.path())),
        Some(PathBuf::from("/env/root"))
    );
}

#[test]
fn pointer_file_is_used_when_env_unset_or_empty() {
    let home = tempfile::tempdir().unwrap();
    private_dir(&home.path().join(".amplihack"));
    private_file(
        &home.path().join(".amplihack/profile-root"),
        b"  /saved/root  \n",
    );
    for env in [vec![], vec![("AMPLIHACK_CLAUDE_PROFILE_ROOT", "")]] {
        assert_eq!(
            resolve_root_from(env_of(&env), Some(home.path())),
            Some(PathBuf::from("/saved/root"))
        );
    }
}

#[test]
fn default_root_is_amplihack_claude_profiles() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_root_from(env_of(&[]), Some(home.path())),
        Some(home.path().join(".amplihack").join("claude-profiles"))
    );
}

#[test]
fn bad_env_root_is_none_never_a_fallback() {
    let home = tempfile::tempdir().unwrap();
    for bad in ["relative/root", "/a/../b", "/a/b/..", "/tab\there"] {
        let pairs = [("AMPLIHACK_CLAUDE_PROFILE_ROOT", bad)];
        let env = env_of(&pairs);
        assert_eq!(
            resolve_root_from(env, Some(home.path())),
            None,
            "env root {bad:?} must disable the hook"
        );
    }
}

#[test]
fn bad_pointer_file_is_none_never_a_fallback() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".amplihack");
    private_dir(&dir);
    let ptr = dir.join("profile-root");
    for bad in [
        &b"relative\n"[..],
        b"/a/../b\n",
        b"\n",
        b"\xff\xfe\n",
        &[b'/'; 5000][..],
    ] {
        private_file(&ptr, bad);
        assert_eq!(
            resolve_root_from(env_of(&[]), Some(home.path())),
            None,
            "pointer {:?} must disable the hook",
            String::from_utf8_lossy(&bad[..bad.len().min(20)])
        );
    }
}

#[test]
fn symlinked_pointer_file_is_none() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".amplihack");
    private_dir(&dir);
    let real = home.path().join("real-pointer");
    private_file(&real, b"/saved/root\n");
    symlink(&real, dir.join("profile-root")).unwrap();
    assert_eq!(resolve_root_from(env_of(&[]), Some(home.path())), None);
}

#[test]
fn group_writable_pointer_file_is_none() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".amplihack");
    private_dir(&dir);
    let ptr = dir.join("profile-root");
    private_file(&ptr, b"/saved/root\n");
    fs::set_permissions(&ptr, fs::Permissions::from_mode(0o620)).unwrap();
    assert_eq!(resolve_root_from(env_of(&[]), Some(home.path())), None);
}

#[test]
fn no_home_and_no_env_is_none() {
    assert_eq!(resolve_root_from(env_of(&[]), None), None);
}

// ── auto_switch_on_from ─────────────────────────────────────────────────────

fn root_with_toggle(contents: Option<&[u8]>) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    if let Some(c) = contents {
        private_file(&root.path().join(".auto-switch"), c);
    }
    root
}

/// env value, toggle file contents, expected.
type ToggleCase = (Option<&'static str>, Option<&'static [u8]>, bool);

#[test]
fn toggle_grammar_table() {
    let cases: &[ToggleCase] = &[
        // env, file, expected
        (None, None, false),
        (None, Some(b"enabled=on\n"), true),
        (None, Some(b"enabled=off\n"), false),
        (None, Some(b"enabled=on\nthreshold=85\nexclude=a,b\n"), true),
        (None, Some(b"enabled=on"), true),
        (None, Some(b"enabled=yes\n"), false),
        (None, Some(b"enabled=on\nenabled=on\n"), false),
        (None, Some(b"enabled=on\nbogus=1\n"), false),
        (None, Some(b"threshold=50\n"), false),
        (None, Some(b"enabled=on\nthreshold=0\n"), false),
        (None, Some(b"enabled=on\nexclude=../x\n"), false),
        (None, Some(b"\xff"), false),
        (Some("1"), None, true),
        (Some("1"), Some(b"enabled=off\n"), true),
        (Some("0"), Some(b"enabled=on\n"), false),
        (Some(""), Some(b"enabled=on\n"), true),
        (Some("yes"), Some(b"enabled=on\n"), true),
        (Some("yes"), None, false),
    ];
    for (env, file, want) in cases {
        let root = root_with_toggle(*file);
        let got = auto_switch_on_from(env.map(OsStr::new), root.path());
        assert_eq!(
            got,
            *want,
            "env={env:?} file={:?}",
            file.map(String::from_utf8_lossy)
        );
    }
}

#[test]
fn oversize_toggle_file_is_off() {
    let mut big = b"enabled=on\n".to_vec();
    big.resize(5000, b'#');
    let root = root_with_toggle(Some(big.as_slice()));
    assert!(!auto_switch_on_from(None, root.path()));
}

#[test]
fn symlinked_toggle_file_is_off() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    private_file(&real, b"enabled=on\n");
    symlink(&real, root.path().join(".auto-switch")).unwrap();
    assert!(!auto_switch_on_from(None, root.path()));
}

#[test]
fn missing_root_is_off_and_not_created() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("missing");
    assert!(!auto_switch_on_from(None, &root));
    assert!(!root.exists());
}

// ── valid_live_name: the ProfileName rule ───────────────────────────────────

#[test]
fn live_name_rule_table() {
    let x64 = "x".repeat(64);
    let x65 = "x".repeat(65);
    for ok in ["work", "spare-2", "a.b_c", "A1", x64.as_str()] {
        assert!(valid_live_name(ok), "{ok:?} should be valid");
    }
    for bad in [
        "",
        "-lead",
        ".hidden",
        "a..b",
        "trail.",
        "a/b",
        "a b",
        "a\nb",
        "CON",
        "com1",
        "lpt9.txt",
        x65.as_str(),
        "é",
    ] {
        assert!(!valid_live_name(bad), "{bad:?} should be invalid");
    }
}

// ── parse_cycle ─────────────────────────────────────────────────────────────

fn ledger(json: &str) -> Vec<u8> {
    json.as_bytes().to_vec()
}

#[test]
fn parse_cycle_reads_the_four_fields() {
    let s = parse_cycle(
        &ledger(&format!(
            r#"{{"version":1,"live":"work","account":"acct-1","since":1,
                "swapped_at":{t},"previous":"spare","alert":"all-over:-",
                "last_cycle":{{"at":{t},"outcome":"stayed"}}}}"#,
            t = NOW - 10
        )),
        NOW,
    )
    .expect("valid ledger");
    assert_eq!(
        s,
        CycleStatus {
            at: Some(NOW - 10),
            outcome: Some("stayed".into()),
            alert: true,
            live: Some("work".into()),
            overflow: None,
        }
    );
}

#[test]
fn parse_cycle_ignores_unknown_fields() {
    let s = parse_cycle(
        &ledger(&format!(
            r#"{{"version":1,"future_field":{{"x":1}},"damp":[1,2],
                "last_cycle":{{"at":{NOW},"outcome":"swapped","extra":true}}}}"#
        )),
        NOW,
    )
    .expect("unknown fields are tolerated");
    assert_eq!(s.outcome.as_deref(), Some("swapped"));
    assert_eq!(s.at, Some(NOW));
    assert!(!s.alert);
    assert_eq!(s.live, None);
}

#[test]
fn parse_cycle_without_last_cycle_has_no_at_or_outcome() {
    let s = parse_cycle(b"{\"version\":1}", NOW).unwrap();
    assert_eq!(s.at, None);
    assert_eq!(s.outcome, None);
    assert!(!s.has_capacity());
}

#[test]
fn parse_cycle_null_alert_is_not_set() {
    let s = parse_cycle(
        &ledger(&format!(
            r#"{{"version":1,"alert":null,"last_cycle":{{"at":{NOW},"outcome":"swapped"}}}}"#
        )),
        NOW,
    )
    .unwrap();
    assert!(!s.alert);
}

#[test]
fn parse_cycle_refuses_other_versions_and_garbage() {
    for bad in [
        &b"{\"version\":2}"[..],
        b"{\"version\":0}",
        b"{\"version\":\"1\"}",
        b"{}",
        b"[]",
        b"null",
        b"not json",
        b"",
        b"\xff\xfe",
    ] {
        assert_eq!(
            parse_cycle(bad, NOW),
            None,
            "{:?}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn parse_cycle_refuses_more_than_64_kib() {
    let mut raw =
        format!(r#"{{"version":1,"last_cycle":{{"at":{NOW},"outcome":"swapped"}},"pad":""#)
            .into_bytes();
    raw.resize(64 * 1024 + 10, b'x');
    raw.extend_from_slice(b"\"}");
    assert_eq!(parse_cycle(&raw, NOW), None);
}

#[test]
fn parse_cycle_ignores_zero_negative_and_far_future_at() {
    for at in [0, -5, NOW + 301, i64::MAX] {
        let s = parse_cycle(
            &ledger(&format!(
                r#"{{"version":1,"last_cycle":{{"at":{at},"outcome":"swapped"}}}}"#
            )),
            NOW,
        )
        .expect("ledger itself is fine");
        assert_eq!(s.at, None, "at={at} must be ignored");
    }
    // Up to 300 s ahead is accepted (clock skew).
    let s = parse_cycle(
        &ledger(&format!(
            r#"{{"version":1,"last_cycle":{{"at":{},"outcome":"swapped"}}}}"#,
            NOW + 300
        )),
        NOW,
    )
    .unwrap();
    assert_eq!(s.at, Some(NOW + 300));
}

#[test]
fn parse_cycle_drops_live_that_fails_the_name_rule() {
    for bad in ["a\nb", "../x", "", "a b", "CON"] {
        let json = serde_json::json!({"version": 1, "live": bad}).to_string();
        let s = parse_cycle(json.as_bytes(), NOW).unwrap();
        assert_eq!(s.live, None, "live {bad:?} must be dropped");
    }
}

#[test]
fn parse_cycle_wrong_typed_fields_are_treated_as_absent() {
    let s = parse_cycle(
        br#"{"version":1,"live":5,"alert":7,"last_cycle":{"at":"soon","outcome":3}}"#,
        NOW,
    )
    .expect("loose parse keeps the ledger");
    assert_eq!(s.live, None);
    assert_eq!(s.at, None);
    assert_eq!(s.outcome, None);
    assert!(!s.has_capacity());
}

// ── has_capacity ────────────────────────────────────────────────────────────

fn status(outcome: &str, alert: bool) -> CycleStatus {
    CycleStatus {
        at: Some(NOW),
        outcome: Some(outcome.into()),
        alert,
        live: Some("work".into()),
        overflow: None,
    }
}

#[test]
fn has_capacity_only_for_swapped_or_stayed_without_alert() {
    assert!(status("swapped", false).has_capacity());
    assert!(status("stayed", false).has_capacity());
    for o in [
        "alerted", "skipped", "stopped", "idle", "", "SWAPPED", "unknown",
    ] {
        assert!(!status(o, false).has_capacity(), "outcome {o:?}");
    }
    assert!(!status("swapped", true).has_capacity());
    assert!(!status("stayed", true).has_capacity());
}

// ── read_cycle: file checks ─────────────────────────────────────────────────

fn root_with_ledger(bytes: &[u8]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".auto-switch.d/rotate");
    private_dir(&root.path().join(".auto-switch.d"));
    private_dir(&dir);
    private_file(&dir.join("ledger.json"), bytes);
    root
}

fn good_ledger() -> Vec<u8> {
    format!(r#"{{"version":1,"live":"work","last_cycle":{{"at":{NOW},"outcome":"swapped"}}}}"#)
        .into_bytes()
}

#[test]
fn ledger_path_is_under_auto_switch_d_rotate() {
    assert_eq!(
        ledger_path(Path::new("/r")),
        PathBuf::from("/r/.auto-switch.d/rotate/ledger.json")
    );
}

#[test]
fn read_cycle_reads_a_private_ledger() {
    let root = root_with_ledger(&good_ledger());
    let s = read_cycle(root.path(), NOW).expect("readable ledger");
    assert!(s.has_capacity());
    assert_eq!(s.live.as_deref(), Some("work"));
}

#[test]
fn read_cycle_missing_ledger_is_none() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(read_cycle(root.path(), NOW), None);
    assert!(
        !root.path().join(".auto-switch.d").exists(),
        "reading must not create anything"
    );
}

#[test]
fn read_cycle_refuses_symlinked_ledger() {
    let root = root_with_ledger(b"{}");
    let real = root.path().join("real.json");
    private_file(&real, &good_ledger());
    let p = ledger_path(root.path());
    fs::remove_file(&p).unwrap();
    symlink(&real, &p).unwrap();
    assert_eq!(read_cycle(root.path(), NOW), None);
}

#[test]
fn read_cycle_refuses_world_or_group_writable_ledger() {
    let root = root_with_ledger(&good_ledger());
    let p = ledger_path(root.path());
    fs::set_permissions(&p, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(read_cycle(root.path(), NOW), None);
}

#[test]
fn read_cycle_accepts_a_0644_ledger() {
    // Readable by others is not a trust problem; writable is.
    let root = root_with_ledger(&good_ledger());
    fs::set_permissions(ledger_path(root.path()), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_cycle(root.path(), NOW).is_some());
}

#[test]
fn read_cycle_refuses_a_directory_in_place_of_the_ledger() {
    let root = root_with_ledger(b"{}");
    let p = ledger_path(root.path());
    fs::remove_file(&p).unwrap();
    private_dir(&p);
    assert_eq!(read_cycle(root.path(), NOW), None);
}

// ── Overflow (issue #1606) ──────────────────────────────────────────────────
//
// `CycleStatus.overflow` is `Some` only for `last_cycle.overflow` in
// {"in","out","on"}; anything else, or a wrong type, is `None`.
// `on_overflow()` is true for "in" and "on". `has_capacity()` is unchanged:
// overflow moves and stays use `swapped` / `stayed`.

fn overflow_ledger(outcome: &str, tag: serde_json::Value, alert: bool) -> Vec<u8> {
    let mut v = serde_json::json!({
        "version": 1,
        "live": "cred",
        "last_cycle": { "at": NOW - 5, "outcome": outcome, "overflow": tag },
        "overflow_blocked": {
            "cred2": { "at": NOW - 60, "until": NOW + 86_400, "reason": "spend_limit" }
        }
    });
    if alert {
        v["alert"] = "all-over:cred".into();
    }
    v.to_string().into_bytes()
}

#[test]
fn overflow_moves_and_stays_are_capacity() {
    for (outcome, tag, on) in [
        ("swapped", "in", true),
        ("stayed", "on", true),
        ("swapped", "out", false),
    ] {
        let s = parse_cycle(&overflow_ledger(outcome, tag.into(), false), NOW).expect("readable");
        assert_eq!(s.overflow.as_deref(), Some(tag));
        assert!(s.has_capacity(), "{tag}");
        assert_eq!(s.on_overflow(), on, "{tag}");
    }
}

#[test]
fn a_blocked_overflow_with_no_fallback_is_no_capacity() {
    let s = parse_cycle(&overflow_ledger("alerted", "on".into(), true), NOW).unwrap();
    assert!(!s.has_capacity());
}

#[test]
fn unknown_or_wrongly_typed_overflow_tags_read_as_none() {
    for tag in [
        serde_json::json!("IN"),
        serde_json::json!("sideways"),
        serde_json::json!(""),
        serde_json::json!(1),
        serde_json::json!(null),
        serde_json::json!({"x": 1}),
    ] {
        let s = parse_cycle(&overflow_ledger("stayed", tag.clone(), false), NOW).expect("lenient");
        assert_eq!(s.overflow, None, "{tag}");
        assert!(!s.on_overflow(), "{tag}");
        assert!(
            s.has_capacity(),
            "the outcome still decides capacity: {tag}"
        );
    }
}

#[test]
fn a_ledger_without_overflow_reads_as_before() {
    let s = parse_cycle(
        format!(
            r#"{{"version":1,"live":"work","last_cycle":{{"at":{},"outcome":"stayed"}}}}"#,
            NOW - 5
        )
        .as_bytes(),
        NOW,
    )
    .unwrap();
    assert_eq!(s.overflow, None);
    assert!(!s.on_overflow());
    assert!(s.has_capacity());
}

// ── fresh_capacity on credits (issue #1606) ─────────────────────────────────
//
// A cycle that moved onto an overflow profile (`swapped`, overflow `in`) or
// stayed on one (`stayed`, overflow `on`) left a login that can work, so the
// limit-resume hook and the recipe-run wait see capacity. `fresh_capacity`
// stays `has_capacity` plus the time check; the overflow tag adds nothing
// and takes nothing away.

fn on_credits(outcome: &str, tag: &str, at: i64) -> CycleStatus {
    CycleStatus {
        at: Some(at),
        outcome: Some(outcome.into()),
        alert: false,
        live: Some("cred".into()),
        overflow: Some(tag.into()),
    }
}

#[test]
fn moving_onto_or_staying_on_credits_is_fresh_capacity() {
    let start = NOW - 600;
    let after = start + FRESH_MARGIN_SECS + 1;
    for (outcome, tag) in [("swapped", "in"), ("stayed", "on")] {
        let s = on_credits(outcome, tag, after);
        assert!(s.on_overflow(), "{outcome}/{tag}");
        assert!(s.has_capacity(), "{outcome}/{tag}");
        assert!(s.fresh_capacity(start), "{outcome}/{tag}");
    }
}

#[test]
fn a_credits_cycle_before_the_limit_hit_is_not_fresh() {
    let start = NOW - 600;
    for at in [start - 1, start, start + FRESH_MARGIN_SECS] {
        let s = on_credits("swapped", "in", at);
        assert!(!s.fresh_capacity(start), "at {at}");
    }
}

#[test]
fn an_alerted_cycle_with_a_credits_tag_is_not_capacity() {
    let mut s = on_credits("stayed", "on", NOW);
    s.alert = true;
    assert!(!s.fresh_capacity(NOW - 600));
    let s = on_credits("alerted", "in", NOW);
    assert!(!s.fresh_capacity(NOW - 600));
}

#[test]
fn a_ledger_that_moved_onto_credits_reads_as_fresh_capacity() {
    let raw = ledger(
        r#"{"version":1,"live":"cred","last_cycle":{"at":1760000000,"outcome":"swapped","overflow":"in"}}"#,
    );
    let s = parse_cycle(&raw, 1_760_000_100).unwrap();
    assert_eq!(s.overflow.as_deref(), Some("in"));
    assert!(s.fresh_capacity(1_760_000_000 - FRESH_MARGIN_SECS - 1));
}
