//! Issue #1658 (A): `amplihack_utils::rotator_status`, the read-only view of
//! the rotator that the limit-resume hook uses. It resolves the profile root,
//! reads the auto-switch toggle, reads the rotate ledger's last cycle and
//! checks that the rotator runs. Since #1659 the CLI calls the same root and
//! toggle code, so no drift test is needed.

#![cfg(unix)]

use amplihack_utils::rotator_status::{
    CycleStatus, FRESH_MARGIN_SECS, LIMIT_RESUME_TIMEOUT_MARGIN, LIMIT_RESUME_TIMEOUT_SECS, Ledger,
    MAX_WAIT, auto_switch_on_from, foreign_rotator, is_rotator, ledger_path, parse_cycle, pid_path,
    read_cycle, read_ledger, resolve_root_from, rotator_down_at, valid_live_name,
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
    // Issue #2085: the hook waits on events, so there is no poll interval;
    // the margin is what keeps the hook's own deadline inside the timeout.
    assert_eq!(LIMIT_RESUME_TIMEOUT_MARGIN, Duration::from_secs(300));
    // `fresh_capacity` (and its margin) stays for the recipe-run wait (D12).
    assert_eq!(FRESH_MARGIN_SECS, 60);
    assert_eq!(LIMIT_RESUME_TIMEOUT_SECS, 608_400);
}

#[test]
fn wait_plus_the_margin_fits_inside_the_registered_timeout() {
    assert!(
        (MAX_WAIT + LIMIT_RESUME_TIMEOUT_MARGIN).as_secs() < LIMIT_RESUME_TIMEOUT_SECS,
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
fn parse_cycle_reads_the_five_fields() {
    let s = parse_cycle(
        &ledger(&format!(
            r#"{{"version":1,"live":"work","account":"acct-1","since":1,
                "swapped_at":{t},"previous":"spare","alert":"all-over:-",
                "last_cycle":{{"at":{t},"measured_at":{m},"outcome":"stayed"}}}}"#,
            m = NOW - 40,
            t = NOW - 10
        )),
        NOW,
    )
    .expect("valid ledger");
    assert_eq!(
        s,
        CycleStatus {
            at: Some(NOW - 10),
            measured_at: Some(NOW - 40),
            outcome: Some("stayed".into()),
            alert: true,
            live: Some("work".into()),
            overflow: None,
            platform_live: false,
            live_room: None,
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
    assert_eq!(s.measured_at, None);
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
        br#"{"version":1,"live":5,"alert":7,"last_cycle":{"at":"soon","measured_at":[],"outcome":3}}"#,
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
        measured_at: Some(NOW),
        outcome: Some(outcome.into()),
        alert,
        live: Some("work".into()),
        overflow: None,
        platform_live: false,
        live_room: None,
    }
}

#[test]
fn has_capacity_only_for_swapped_stayed_or_switched_without_alert() {
    assert!(status("swapped", false).has_capacity());
    assert!(status("stayed", false).has_capacity());
    // Issue #1691: `profile switch` records the login the person chose.
    assert!(status("switched", false).has_capacity());
    for o in [
        "alerted", "skipped", "stopped", "idle", "held", "", "SWAPPED", "SWITCHED", "unknown",
    ] {
        assert!(!status(o, false).has_capacity(), "outcome {o:?}");
    }
    assert!(!status("swapped", true).has_capacity());
    assert!(!status("stayed", true).has_capacity());
    assert!(!status("switched", true).has_capacity());
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

// ── fresh_capacity on overflow (issue #1606) ────────────────────────────────
//
// A cycle that moved onto an overflow profile (`swapped`, overflow `in`) or
// stayed on one (`stayed`, overflow `on`) left a login that can work, so the
// limit-resume hook and the recipe-run wait see capacity. `fresh_capacity`
// stays `has_capacity` plus the time check; the overflow tag adds nothing
// and takes nothing away.

fn on_overflow_cycle(outcome: &str, tag: &str, at: i64) -> CycleStatus {
    CycleStatus {
        at: Some(at),
        measured_at: None,
        outcome: Some(outcome.into()),
        alert: false,
        live: Some("cred".into()),
        overflow: Some(tag.into()),
        platform_live: false,
        live_room: None,
    }
}

#[test]
fn moving_onto_or_staying_on_overflow_is_fresh_capacity() {
    let start = NOW - 600;
    let after = start + FRESH_MARGIN_SECS + 1;
    for (outcome, tag) in [("swapped", "in"), ("stayed", "on")] {
        let s = on_overflow_cycle(outcome, tag, after);
        assert!(s.on_overflow(), "{outcome}/{tag}");
        assert!(s.has_capacity(), "{outcome}/{tag}");
        assert!(s.fresh_capacity(start), "{outcome}/{tag}");
    }
}

#[test]
fn a_overflow_cycle_before_the_limit_hit_is_not_fresh() {
    let start = NOW - 600;
    for at in [start - 1, start, start + FRESH_MARGIN_SECS] {
        let s = on_overflow_cycle("swapped", "in", at);
        assert!(!s.fresh_capacity(start), "at {at}");
    }
}

#[test]
fn an_alerted_cycle_with_a_overflow_tag_is_not_capacity() {
    let mut s = on_overflow_cycle("stayed", "on", NOW);
    s.alert = true;
    assert!(!s.fresh_capacity(NOW - 600));
    let s = on_overflow_cycle("alerted", "in", NOW);
    assert!(!s.fresh_capacity(NOW - 600));
}

/// With #1659's `measured_at`, an overflow cycle is fresh exactly when its
/// probe began after the limit: the overflow tag changes neither side.
#[test]
fn an_overflow_cycle_measured_after_the_limit_is_fresh_capacity() {
    let start = NOW - 600;
    for (outcome, tag) in [("swapped", "in"), ("stayed", "on")] {
        let mut s = on_overflow_cycle(outcome, tag, start + 2);
        s.measured_at = Some(start + 1);
        assert!(s.fresh_capacity(start), "{outcome}/{tag} measured after");
        s.measured_at = Some(start);
        assert!(
            !s.fresh_capacity(start),
            "{outcome}/{tag} measured at the limit"
        );
    }
    // Through the ledger file, as the limit-resume hook reads it.
    let raw = ledger(&format!(
        r#"{{"version":1,"live":"cred","last_cycle":{{"at":{},"measured_at":{},"outcome":"stayed","overflow":"on"}}}}"#,
        start + 2,
        start + 1
    ));
    let s = parse_cycle(&raw, NOW).unwrap();
    assert!(s.on_overflow());
    assert_eq!(s.measured_at, Some(start + 1));
    assert!(s.fresh_capacity(start));
}

#[test]
fn a_ledger_that_moved_onto_overflow_reads_as_fresh_capacity() {
    let raw = ledger(
        r#"{"version":1,"live":"cred","last_cycle":{"at":1760000000,"outcome":"swapped","overflow":"in"}}"#,
    );
    let s = parse_cycle(&raw, 1_760_000_100).unwrap();
    assert_eq!(s.overflow.as_deref(), Some("in"));
    assert!(s.fresh_capacity(1_760_000_000 - FRESH_MARGIN_SECS - 1));
}

#[test]
fn read_ledger_tells_missing_from_unreadable() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(read_ledger(root.path(), NOW), Ledger::Missing);
    let root = root_with_ledger(br#"{"version":2}"#);
    assert_eq!(read_ledger(root.path(), NOW), Ledger::Unreadable);
    let root = root_with_ledger(&good_ledger());
    assert!(matches!(read_ledger(root.path(), NOW), Ledger::Read(_)));
}

// ── rotator_down ────────────────────────────────────────────────────────────

const PID: u32 = 4242;

/// A `/proc` with one process of this user running `cmdline`.
fn fake_proc(cmdline: &[u8]) -> tempfile::TempDir {
    let proc_root = tempfile::tempdir().unwrap();
    let dir = proc_root.path().join(PID.to_string());
    private_dir(&dir);
    fs::write(dir.join("cmdline"), cmdline).unwrap();
    proc_root
}

const WATCH: &[u8] = b"/home/u/.cargo/bin/amplihack\0profile\0rotate\0watch\0";

fn with_pid_file(root: &Path) {
    let p = pid_path(root);
    private_dir(p.parent().unwrap());
    private_file(&p, format!("{PID}\n").as_bytes());
}

#[test]
fn a_running_rotator_with_a_readable_or_missing_ledger_is_up() {
    let proc_root = fake_proc(WATCH);
    let root = tempfile::tempdir().unwrap();
    private_dir(&root.path().join(".auto-switch.d"));
    with_pid_file(root.path());
    assert!(
        !rotator_down_at(root.path(), proc_root.path(), NOW),
        "no cycle yet"
    );
    let root = root_with_ledger(&good_ledger());
    with_pid_file(root.path());
    assert!(!rotator_down_at(root.path(), proc_root.path(), NOW));
}

#[test]
fn no_pid_file_or_a_pid_that_is_no_rotator_is_down() {
    let root = root_with_ledger(&good_ledger());
    assert!(rotator_down_at(root.path(), fake_proc(WATCH).path(), NOW));
    with_pid_file(root.path());
    let other = fake_proc(b"/usr/bin/sleep\x00600\x00");
    assert!(rotator_down_at(root.path(), other.path(), NOW));
    let empty = tempfile::tempdir().unwrap();
    assert!(rotator_down_at(root.path(), empty.path(), NOW), "pid gone");
}

#[test]
fn a_running_rotator_with_an_unreadable_ledger_is_down() {
    let root = root_with_ledger(br#"{"version":2,"last_cycle":{"at":1,"outcome":"swapped"}}"#);
    with_pid_file(root.path());
    assert!(rotator_down_at(root.path(), fake_proc(WATCH).path(), NOW));
}

#[test]
fn without_proc_only_the_ledger_counts() {
    let root = root_with_ledger(&good_ledger());
    let no_proc = Path::new("/nonexistent-proc-for-test");
    with_pid_file(root.path());
    assert!(!rotator_down_at(root.path(), no_proc, NOW));
    let root = root_with_ledger(b"not json");
    with_pid_file(root.path());
    assert!(rotator_down_at(root.path(), no_proc, NOW));
}

#[test]
fn is_rotator_checks_the_exe_name_only_when_given() {
    assert!(is_rotator(WATCH, None));
    assert!(is_rotator(WATCH, Some(OsStr::new("amplihack"))));
    assert!(!is_rotator(WATCH, Some(OsStr::new("amplihack-hooks"))));
    assert!(!is_rotator(b"amplihack\0profile\0rotate\0status\0", None));
}

/// A `/proc` whose rotator runs the executable `exe`. A hard link, like
/// the real `exe` link, keeps the inode that ran even if `exe` is replaced.
fn fake_proc_running(exe: &Path) -> tempfile::TempDir {
    let proc_root = fake_proc(WATCH);
    fs::hard_link(exe, proc_root.path().join(PID.to_string()).join("exe")).unwrap();
    proc_root
}

#[test]
fn a_rotator_running_another_binary_is_foreign() {
    let bins = tempfile::tempdir().unwrap();
    let (old, new) = (bins.path().join("old"), bins.path().join("new"));
    fs::write(&old, b"old").unwrap();
    fs::write(&new, b"new").unwrap();
    let root = tempfile::tempdir().unwrap();
    private_dir(&root.path().join(".auto-switch.d"));
    with_pid_file(root.path());
    let pid_file = pid_path(root.path());

    let same = fake_proc_running(&new);
    assert_eq!(foreign_rotator(&pid_file, same.path(), Some(&new)), None);
    let other = fake_proc_running(&old);
    assert_eq!(
        foreign_rotator(&pid_file, other.path(), Some(&new)),
        Some(PID)
    );
    // Replaced at the same path by an upgrade: another inode.
    fs::remove_file(&new).unwrap();
    fs::write(&new, b"upgraded").unwrap();
    assert_eq!(
        foreign_rotator(&pid_file, same.path(), Some(&new)),
        Some(PID)
    );
    // Unknown is treated as foreign.
    assert_eq!(
        foreign_rotator(&pid_file, fake_proc(WATCH).path(), Some(&new)),
        Some(PID)
    );
    // No rotator running: nothing to protect.
    let none = tempfile::tempdir().unwrap();
    assert_eq!(foreign_rotator(&pid_file, none.path(), Some(&new)), None);
}

// ── Issue #1907: what counts as capacity ────────────────────────────────────

/// A ledger whose last cycle ended `outcome`, measured after a limit hit at
/// `NOW - 100`, with `extra` fields at the top level.
fn ledger_1907(outcome: &str, extra: &str) -> Vec<u8> {
    ledger(&format!(
        r#"{{"version":1,"live":"work"{extra},
            "last_cycle":{{"at":{t},"measured_at":{m},"outcome":"{outcome}"}}}}"#,
        m = NOW - 40,
        t = NOW - 10
    ))
}

#[test]
fn issue_1907_a_stayed_unread_cycle_is_not_capacity() {
    let s = parse_cycle(&ledger_1907("stayed-unread", ""), NOW).unwrap();
    assert_eq!(s.outcome.as_deref(), Some("stayed-unread"));
    assert!(!s.alert);
    assert!(!s.has_capacity());
    assert!(!s.fresh_capacity(NOW - 100));
}

#[test]
fn issue_1907_an_unknown_outcome_without_an_alert_is_not_capacity() {
    for o in ["stayed-later", "stayed_unread", "Stayed", ""] {
        let s = parse_cycle(&ledger_1907(o, r#","alert":null"#), NOW).unwrap();
        assert!(!s.has_capacity(), "{o:?}");
    }
}

#[test]
fn issue_1907_a_stay_with_only_an_advisory_alert_is_capacity() {
    let raw = ledger_1907(
        "stayed",
        r#","alert":null,"advisory_alert":"no-capacity:work""#,
    );
    let s = parse_cycle(&raw, NOW).unwrap();
    assert!(!s.alert, "advisory_alert is not read as alert");
    assert!(s.has_capacity());
    assert!(s.fresh_capacity(NOW - 100));
    // And without the null `alert` key at all.
    let raw = ledger_1907("stayed", r#","advisory_alert":"no-capacity:work""#);
    assert!(parse_cycle(&raw, NOW).unwrap().fresh_capacity(NOW - 100));
}

#[test]
fn issue_1907_a_hard_alert_still_blocks_whatever_the_advisory_field_says() {
    let raw = ledger_1907(
        "stayed",
        r#","alert":"no-capacity:work","advisory_alert":null"#,
    );
    assert!(!parse_cycle(&raw, NOW).unwrap().has_capacity());
}

// ── Issue #2857: `live_room`, the rotator's reading of room on the live login
//
// `last_cycle.live_room` is one optional bit the rotator writes from the
// measurement it already holds: the live login after the cycle reads below the
// rotator's own `--session-at` on the session window and below 100% on the
// week. The limit wait wakes on a login change only when it is `true`.
//
// `fresh_room(start)` is exactly `fresh_capacity(start) && says_room()`, and
// `says_room()` is a pure reading of the outcome and the bit: `true` only on
// `Some(true)` for a `swapped` or `stayed` cycle, and for a `switched` one
// (a person's own `profile switch`, #1691) unless the bit says `false`. So it
// adds a requirement and takes none away. `has_capacity` and
// `fresh_capacity` keep their meaning byte for byte, because the limit-resume
// hook reads them and #2857 does not change that hook.

fn cycle_with(last_cycle_extra: &str, top_extra: &str) -> Vec<u8> {
    format!(
        r#"{{"version":1,"live":"work","alert":null{top_extra},"last_cycle":{{"at":{at},"measured_at":{m},"outcome":"swapped"{last_cycle_extra}}}}}"#,
        at = NOW - 10,
        m = NOW - 20,
    )
    .into_bytes()
}

#[test]
fn issue_2857_live_room_reads_true_and_false_from_the_last_cycle() {
    let yes = parse_cycle(&cycle_with(r#","live_room":true"#, ""), NOW).unwrap();
    assert_eq!(yes.live_room, Some(true));
    let no = parse_cycle(&cycle_with(r#","live_room":false"#, ""), NOW).unwrap();
    assert_eq!(no.live_room, Some(false));
}

#[test]
fn issue_2857_a_ledger_from_an_older_rotator_has_no_live_room() {
    let s = parse_cycle(&cycle_with("", ""), NOW).unwrap();
    assert_eq!(
        s.live_room, None,
        "absent is absent, never defaulted to room"
    );
    assert!(
        !s.fresh_room(NOW - 100),
        "an older rotator's cycle never counts as room, whatever its capacity says"
    );
    assert!(
        s.fresh_capacity(NOW - 100),
        "and the hook's rule for the same ledger is untouched"
    );
}

/// Parsed loosely, as every field of this ledger is: anything that is not a
/// JSON boolean in `last_cycle` reads as absent, and absent is no room.
#[test]
fn issue_2857_a_wrongly_typed_or_misplaced_live_room_reads_as_absent() {
    for (what, last, top) in [
        ("null", r#","live_room":null"#, ""),
        ("a string", r#","live_room":"true""#, ""),
        ("a number", r#","live_room":1"#, ""),
        ("an array", r#","live_room":[true]"#, ""),
        ("an object", r#","live_room":{"v":true}"#, ""),
        ("a top-level key", "", r#","live_room":true"#),
    ] {
        let s = parse_cycle(&cycle_with(last, top), NOW).unwrap_or_else(|| panic!("{what}"));
        assert_eq!(s.live_room, None, "{what}");
        assert!(!s.fresh_room(NOW - 100), "{what}");
    }
}

#[test]
fn issue_2857_fresh_room_needs_fresh_capacity_and_a_true_live_room() {
    let start = NOW - 600;
    let mut s = status("swapped", false);
    s.live_room = Some(true);
    assert!(s.fresh_room(start), "capacity, fresh, room");
    s.live_room = Some(false);
    assert!(
        !s.fresh_room(start),
        "capacity and fresh, but the login is full"
    );
    assert!(
        s.fresh_capacity(start),
        "the same cycle is still capacity for the hook"
    );
    s.live_room = None;
    assert!(!s.fresh_room(start));
}

/// The positive control for every case below: the same cycle with
/// `live_room: true` and nothing else wrong is room.
#[test]
fn issue_2857_a_true_live_room_does_not_rescue_a_cycle_that_is_not_capacity() {
    let start = NOW - 600;
    let mut ok = status("swapped", false);
    ok.live_room = Some(true);
    assert!(ok.fresh_room(start), "positive control");

    for outcome in [
        "alerted",
        "stayed-unread",
        "held",
        "stopped",
        "idle",
        "skipped",
        "",
    ] {
        let mut s = status(outcome, false);
        s.live_room = Some(true);
        assert!(!s.fresh_room(start), "outcome {outcome:?}");
    }
    let mut alerted = status("swapped", true);
    alerted.live_room = Some(true);
    assert!(!alerted.fresh_room(start), "an ALERT is set");
}

#[test]
fn issue_2857_a_stale_cycle_with_room_is_not_fresh_room() {
    let start = NOW - 600;
    let mut s = status("swapped", false);
    s.live_room = Some(true);
    for measured in [start - 1, start] {
        s.measured_at = Some(measured);
        assert!(!s.fresh_room(start), "measured at {measured}");
    }
    s.measured_at = Some(start + 1);
    assert!(
        s.fresh_room(start),
        "positive control: measured after the limit"
    );
    // From a rotator that wrote no `measured_at`, the cycle must have been
    // recorded more than the margin after the limit: `fresh_capacity`'s rule.
    s.measured_at = None;
    s.at = Some(start + FRESH_MARGIN_SECS);
    assert!(!s.fresh_room(start));
    s.at = Some(start + FRESH_MARGIN_SECS + 1);
    assert!(s.fresh_room(start));
}

#[test]
fn issue_2857_live_room_changes_neither_has_capacity_nor_fresh_capacity() {
    let start = NOW - 600;
    for room in [None, Some(false), Some(true)] {
        let mut s = status("stayed", false);
        s.live_room = room;
        assert!(s.has_capacity(), "{room:?}");
        assert!(s.fresh_capacity(start), "{room:?}");
        let mut alerted = status("stayed", true);
        alerted.live_room = room;
        assert!(!alerted.has_capacity(), "{room:?}");
    }
}

#[test]
fn issue_2857_live_room_survives_the_file_checks_unchanged() {
    // Through `read_cycle`, as the limit wait reads it: a private file.
    let root = root_with_ledger(&cycle_with(r#","live_room":true"#, ""));
    let s = read_cycle(root.path(), NOW).expect("a private ledger is read");
    assert_eq!(s.live_room, Some(true));
    // A ledger the checks refuse yields no status at all, so no room.
    let root = root_with_ledger(&cycle_with(r#","live_room":true"#, ""));
    fs::set_permissions(ledger_path(root.path()), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(read_cycle(root.path(), NOW).is_none(), "world-writable");
    let big = format!(
        r#"{{"version":1,"pad":"{}","last_cycle":{{"at":{NOW},"outcome":"swapped","live_room":true}}}}"#,
        "x".repeat(70 * 1024)
    );
    assert!(parse_cycle(big.as_bytes(), NOW).is_none(), "over 64 KiB");
    let v2 = br#"{"version":2,"last_cycle":{"at":1,"outcome":"swapped","live_room":true}}"#;
    assert!(parse_cycle(v2, NOW).is_none(), "an unknown version");
}

// ── Issue #2857: `says_room`, the one rule the wait gate and `fresh_room` share
//
// `says_room()` is the whole of the question "does this record claim room on
// the live login", with no clock and no I/O, so the gate in the limit wait and
// `fresh_room` cannot disagree about it. It does not look at `alert`; the
// callers take that from `has_capacity`.
//
// The one carve-out is `switched`. A person who runs `profile switch` has
// chosen the login, and the rotator has measured nothing about it, so the
// record has no bit. The wake #1691 gave that person stays unless the record
// positively says there is no room. Every other outcome needs `Some(true)`,
// and an outcome this reader does not know is never room, whatever the bit.

/// `(outcome, live_room none, live_room true, live_room false)`.
const SAYS_ROOM: [(&str, bool, bool, bool); 11] = [
    ("swapped", false, true, false),
    ("stayed", false, true, false),
    ("switched", true, true, false),
    ("stayed-unread", false, false, false),
    ("alerted", false, false, false),
    ("held", false, false, false),
    ("idle", false, false, false),
    ("skipped", false, false, false),
    ("stopped", false, false, false),
    ("Swapped", false, false, false),
    ("", false, false, false),
];

#[test]
fn issue_2857_says_room_for_every_outcome_and_every_reading_of_the_bit() {
    for (outcome, none, yes, no) in SAYS_ROOM {
        for (room, want) in [(None, none), (Some(true), yes), (Some(false), no)] {
            let mut s = status(outcome, false);
            s.live_room = room;
            assert_eq!(
                s.says_room(),
                want,
                "outcome {outcome:?}, live_room {room:?}"
            );
        }
    }
}

/// SEC-4: fail closed. An outcome outside swapped/stayed/switched is never
/// room, and the bit cannot make it so; and a record with no outcome at all
/// says nothing.
#[test]
fn issue_2857_an_outcome_nobody_knows_is_never_room_whatever_the_bit_says() {
    for outcome in ["stopped", "resumed", "SWITCHED", "switched ", "swapped\n"] {
        let mut s = status(outcome, false);
        s.live_room = Some(true);
        assert!(!s.says_room(), "outcome {outcome:?}");
    }
    let mut none = status("swapped", false);
    none.outcome = None;
    none.live_room = Some(true);
    assert!(!none.says_room(), "no outcome at all");
}

/// SEC-8: pure. The answer depends on the outcome and the bit and on nothing
/// else in the record: not when it was written, not when it measured, not
/// which login is live.
#[test]
fn issue_2857_says_room_reads_only_the_outcome_and_the_bit() {
    for (outcome, room) in [
        ("swapped", Some(true)),
        ("switched", None),
        ("stayed", Some(false)),
    ] {
        let mut base = status(outcome, false);
        base.live_room = room;
        let want = base.says_room();
        for (at, measured_at) in [(None, None), (Some(1), Some(1)), (Some(NOW * 2), None)] {
            let mut s = base.clone();
            s.at = at;
            s.measured_at = measured_at;
            s.live = None;
            s.overflow = Some("in".into());
            s.platform_live = true;
            assert_eq!(s.says_room(), want, "{outcome}, {room:?}, at {at:?}");
        }
    }
}

/// A manual switch has capacity for the hook and says room for the wait, with
/// no bit written, and a fresh record of it is fresh room. The same switch is
/// stale room once the limit is newer than its probe.
#[test]
fn issue_2857_a_manual_switch_with_no_bit_is_fresh_room() {
    let start = NOW - 600;
    let mut s = status("switched", false);
    assert_eq!(s.live_room, None, "profile switch measures nothing");
    assert!(s.fresh_capacity(start), "capacity, as the hook reads it");
    assert!(s.fresh_room(start), "and room for the wait");
    s.measured_at = Some(start);
    assert!(!s.fresh_room(start), "freshness is still required");
    s.measured_at = Some(start + 1);
    assert!(s.fresh_room(start), "positive control");
}

#[test]
fn issue_2857_a_manual_switch_that_recorded_no_room_is_not_fresh_room() {
    let start = NOW - 600;
    let mut s = status("switched", false);
    s.live_room = Some(false);
    assert!(
        s.fresh_capacity(start),
        "still capacity for the hook, whatever the bit"
    );
    assert!(
        !s.fresh_room(start),
        "a record that positively says there is no room is not room"
    );
    s.live_room = Some(true);
    assert!(s.fresh_room(start), "positive control");
}

/// The carve-out lifts the bit's requirement and nothing else: a `switched`
/// cycle under an ALERT has no capacity, so it has no room.
#[test]
fn issue_2857_a_manual_switch_under_an_alert_is_no_room() {
    let start = NOW - 600;
    for room in [None, Some(true)] {
        let mut s = status("switched", true);
        s.live_room = room;
        assert!(!s.fresh_room(start), "{room:?}");
    }
}

/// Only a `switched` record is let through without a bit. The rotator's own
/// `swapped` and `stayed` were the incident, and still need `Some(true)`.
#[test]
fn issue_2857_the_carve_out_is_for_switched_alone() {
    let start = NOW - 600;
    for outcome in ["swapped", "stayed"] {
        let s = status(outcome, false);
        assert!(s.fresh_capacity(start), "{outcome}: capacity");
        assert!(!s.fresh_room(start), "{outcome}: no bit, no room");
    }
}

/// Through the parser as the wait reads the file: a `switched` ledger with no
/// `live_room` key, and one with a wrongly typed key, which reads as absent.
#[test]
fn issue_2857_a_switched_ledger_from_disk_says_room_unless_the_bit_says_otherwise() {
    let ledger = |extra: &str| {
        format!(
            r#"{{"version":1,"live":"work","alert":null,"last_cycle":{{"at":{at},"measured_at":{m},"outcome":"switched"{extra}}}}}"#,
            at = NOW - 10,
            m = NOW - 20,
        )
        .into_bytes()
    };
    for (what, extra, want) in [
        ("no key", "", true),
        ("true", r#","live_room":true"#, true),
        ("false", r#","live_room":false"#, false),
        ("a string", r#","live_room":"false""#, true),
        ("null", r#","live_room":null"#, true),
    ] {
        let s = parse_cycle(&ledger(extra), NOW).unwrap_or_else(|| panic!("{what}"));
        assert_eq!(s.says_room(), want, "{what}");
        assert_eq!(s.fresh_room(NOW - 100), want, "{what}");
    }
}
