//! Issue #2085 (T1): `rotator_status::swap_record`, the swap record the
//! rotator writes after each swap and the limit-resume hook waits for.
//!
//! Contract (design C1):
//! - `SwapRecord { seq, live_login, from_login, swapped_at, reading:
//!   SwapReading { session, week, measured_at } }`, serde, `PartialEq`.
//! - `SWAP_FILE == "swap.json"`, `MAX_SWAP_BYTES == 4096`,
//!   `rotate_dir(root)`, `swap_path(root)`.
//! - `parse_swap(raw, now) -> Option<SwapRecord>`: loose (unknown fields
//!   ignored), but every known field must be present and well typed; both
//!   login names pass `valid_live_name`; both times satisfy
//!   `0 < t <= now + MAX_SKEW_SECS` (300 s, the ledger's rule).
//! - `read_swap(root, now)`: no symlink, owned by the euid, not writable by
//!   group or others, at most 4096 bytes; anything else is `None`.
//! - `judge(snapshot, record, start, resumed_at) -> Judgement`, pure, in the order
//!   Same, Stale, NoRoom, Resume. `seq` is never compared for order.
//!
//! Every test uses a temporary root; nothing reads the real profile root.

#![cfg(unix)]

use amplihack_utils::rotator_status::swap_record::{
    Judgement, MAX_SWAP_BYTES, STOP_SLACK_SECS, SWAP_FILE, SwapReading, SwapRecord, judge,
    parse_swap, read_swap, rotate_dir, swap_path,
};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

const NOW: i64 = 1_792_592_000;
/// The limit hit (the hook's start) is 10 minutes before `NOW`.
const START: i64 = NOW - 600;

fn record(seq: u64, swapped_at: i64, measured_at: i64, session: u8, week: u8) -> SwapRecord {
    SwapRecord {
        from_org: None,
        to_org: None,
        affinity: None,
        seq,
        live_login: "spare".into(),
        from_login: "work".into(),
        swapped_at,
        reading: SwapReading {
            session,
            week,
            measured_at,
        },
    }
}

/// A record written after the limit with room on the new login.
fn fresh(seq: u64) -> SwapRecord {
    record(seq, START + 60, START + 30, 12, 40)
}

fn json_of(r: &SwapRecord) -> String {
    serde_json::to_string(r).unwrap()
}

fn private_dir(p: &Path) {
    fs::create_dir_all(p).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
}

/// A temporary root with `bytes` at `swap.json`, mode `mode`.
fn root_with(bytes: &[u8], mode: u32) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    private_dir(&root.path().join(".auto-switch.d"));
    let dir = rotate_dir(root.path());
    private_dir(&dir);
    let p = dir.join(SWAP_FILE);
    fs::write(&p, bytes).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
    root
}

// ── Layout ──────────────────────────────────────────────────────────────────

#[test]
fn the_record_lives_beside_the_ledger() {
    assert_eq!(SWAP_FILE, "swap.json");
    assert_eq!(MAX_SWAP_BYTES, 4096);
    assert_eq!(
        rotate_dir(Path::new("/r")),
        Path::new("/r/.auto-switch.d/rotate")
    );
    assert_eq!(
        swap_path(Path::new("/r")),
        Path::new("/r/.auto-switch.d/rotate/swap.json")
    );
}

// ── parse_swap ──────────────────────────────────────────────────────────────

#[test]
fn parse_swap_reads_the_documented_schema() {
    let raw = br#"{"seq":7,"live_login":"spare","from_login":"work","swapped_at":1792591460,
        "reading":{"session":12,"week":40,"measured_at":1792591430}}"#;
    let r = parse_swap(raw, NOW).expect("a well-formed record");
    assert_eq!(r, record(7, 1_792_591_460, 1_792_591_430, 12, 40));
}

#[test]
fn a_record_round_trips_through_serde() {
    let r = fresh(3);
    assert_eq!(parse_swap(json_of(&r).as_bytes(), NOW), Some(r));
}

#[test]
fn the_serialised_key_set_is_exactly_the_schema() {
    // S14: names, numbers and times only; no flatten, no extra keys.
    let v: serde_json::Value = serde_json::from_str(&json_of(&fresh(1))).unwrap();
    let mut top: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    top.sort();
    assert_eq!(
        top,
        ["from_login", "live_login", "reading", "seq", "swapped_at"]
    );
    let mut reading: Vec<_> = v["reading"].as_object().unwrap().keys().cloned().collect();
    reading.sort();
    assert_eq!(reading, ["measured_at", "session", "week"]);
}

#[test]
fn unknown_fields_are_ignored() {
    let mut v: serde_json::Value = serde_json::from_str(&json_of(&fresh(2))).unwrap();
    v["future"] = serde_json::json!({"x": 1});
    v["reading"]["extra"] = serde_json::json!("y");
    assert_eq!(parse_swap(v.to_string().as_bytes(), NOW), Some(fresh(2)));
}

#[test]
fn a_missing_or_wrongly_typed_field_is_none() {
    let base: serde_json::Value = serde_json::from_str(&json_of(&fresh(2))).unwrap();
    for key in ["seq", "live_login", "from_login", "swapped_at", "reading"] {
        let mut v = base.clone();
        v.as_object_mut().unwrap().remove(key);
        assert_eq!(parse_swap(v.to_string().as_bytes(), NOW), None, "no {key}");
    }
    for key in ["session", "week", "measured_at"] {
        let mut v = base.clone();
        v["reading"].as_object_mut().unwrap().remove(key);
        assert_eq!(parse_swap(v.to_string().as_bytes(), NOW), None, "no {key}");
    }
    for (key, bad) in [
        ("seq", serde_json::json!("1")),
        ("seq", serde_json::json!(-1)),
        ("live_login", serde_json::json!(5)),
        ("swapped_at", serde_json::json!("now")),
    ] {
        let mut v = base.clone();
        v[key] = bad.clone();
        assert_eq!(
            parse_swap(v.to_string().as_bytes(), NOW),
            None,
            "{key}={bad}"
        );
    }
    // A percentage is a u8: 256, a negative or a fraction is refused.
    for bad in [
        serde_json::json!(256),
        serde_json::json!(-1),
        serde_json::json!(1.5),
    ] {
        let mut v = base.clone();
        v["reading"]["session"] = bad.clone();
        assert_eq!(parse_swap(v.to_string().as_bytes(), NOW), None, "{bad}");
    }
}

#[test]
fn garbage_and_non_objects_are_none() {
    for raw in [&b""[..], b"null", b"[]", b"\"swap\"", b"{", b"\xff\xfe"] {
        assert_eq!(parse_swap(raw, NOW), None, "{raw:?}");
    }
}

#[test]
fn a_record_over_4096_bytes_is_none() {
    let body = json_of(&fresh(1));
    let mut padded = body.clone().into_bytes();
    padded.resize(MAX_SWAP_BYTES as usize + 1, b' ');
    assert_eq!(parse_swap(&padded, NOW), None);
    let mut exact = body.into_bytes();
    exact.resize(MAX_SWAP_BYTES as usize, b' ');
    assert_eq!(parse_swap(&exact, NOW), Some(fresh(1)));
}

#[test]
fn login_names_must_pass_the_profile_name_rule() {
    // S7: both names, so a log line can never carry text from the file.
    for bad in [
        "",
        "../x",
        "a b",
        "a/b",
        ".hidden",
        "x\nOUTCOME=resumed",
        "a..",
    ] {
        let mut r = fresh(1);
        r.live_login = bad.into();
        assert_eq!(
            parse_swap(json_of(&r).as_bytes(), NOW),
            None,
            "live {bad:?}"
        );
        let mut r = fresh(1);
        r.from_login = bad.into();
        assert_eq!(
            parse_swap(json_of(&r).as_bytes(), NOW),
            None,
            "from {bad:?}"
        );
    }
}

#[test]
fn times_follow_the_ledger_skew_rule() {
    for (swapped_at, measured_at, ok) in [
        (NOW, NOW, true),
        (NOW + 300, NOW + 300, true),
        (NOW + 301, NOW, false),
        (NOW, NOW + 301, false),
        (0, NOW, false),
        (NOW, 0, false),
        (-5, NOW, false),
        (NOW, -5, false),
        (i64::MAX, NOW, false),
    ] {
        let r = record(1, swapped_at, measured_at, 1, 1);
        assert_eq!(
            parse_swap(json_of(&r).as_bytes(), NOW).is_some(),
            ok,
            "swapped_at={swapped_at} measured_at={measured_at}"
        );
    }
}

// ── read_swap ───────────────────────────────────────────────────────────────

#[test]
fn read_swap_reads_a_private_record() {
    let root = root_with(json_of(&fresh(4)).as_bytes(), 0o600);
    assert_eq!(read_swap(root.path(), NOW), Some(fresh(4)));
}

#[test]
fn read_swap_without_a_record_is_none_and_creates_nothing() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(read_swap(root.path(), NOW), None);
    assert!(!root.path().join(".auto-switch.d").exists());
}

#[test]
fn read_swap_refuses_a_record_others_can_write() {
    // S1: group- or world-writable means someone else could have written it.
    for mode in [0o620, 0o602, 0o666] {
        let root = root_with(json_of(&fresh(4)).as_bytes(), mode);
        assert_eq!(read_swap(root.path(), NOW), None, "mode {mode:o}");
    }
    // Readable by others is not a write risk.
    let root = root_with(json_of(&fresh(4)).as_bytes(), 0o644);
    assert_eq!(read_swap(root.path(), NOW), Some(fresh(4)));
}

#[test]
fn read_swap_never_follows_a_symlink() {
    let root = root_with(b"{}", 0o600);
    let elsewhere = root.path().join("elsewhere.json");
    fs::write(&elsewhere, json_of(&fresh(4))).unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o600)).unwrap();
    let p = swap_path(root.path());
    fs::remove_file(&p).unwrap();
    symlink(&elsewhere, &p).unwrap();
    assert_eq!(read_swap(root.path(), NOW), None);
}

#[test]
fn read_swap_refuses_an_oversized_or_malformed_file() {
    let mut big = json_of(&fresh(4)).into_bytes();
    big.resize(MAX_SWAP_BYTES as usize + 10, b' ');
    let root = root_with(&big, 0o600);
    assert_eq!(read_swap(root.path(), NOW), None);
    let root = root_with(b"not json", 0o600);
    assert_eq!(read_swap(root.path(), NOW), None);
}

#[test]
fn read_swap_refuses_a_directory_in_place_of_the_file() {
    let root = tempfile::tempdir().unwrap();
    private_dir(&swap_path(root.path()));
    assert_eq!(read_swap(root.path(), NOW), None);
}

// ── judge ───────────────────────────────────────────────────────────────────

#[test]
fn a_new_fresh_record_with_room_resumes() {
    assert_eq!(judge(None, &fresh(1), START, None), Judgement::Resume);
    assert_eq!(
        judge(Some(&fresh(1)), &fresh(2), START, None),
        Judgement::Resume
    );
}

#[test]
fn an_identical_rewrite_is_same() {
    // An inotify event for a rewrite with the same content is not a swap.
    let noted = record(5, START - 60, START - 90, 12, 40);
    assert_eq!(judge(Some(&noted), &noted, START, None), Judgement::Same);
    // Nor is a noted record dated after the hook's start (a clock ahead).
    assert_eq!(
        judge(Some(&fresh(5)), &fresh(5), START, None),
        Judgement::Same
    );
}

#[test]
fn a_reset_seq_with_fresh_times_resumes() {
    // D4: seq is not trusted to only go up; a record that differs from the
    // snapshot and is newer than the stop is a new swap.
    let snapshot = record(41, START - 100, START - 120, 10, 10);
    assert_eq!(
        judge(Some(&snapshot), &fresh(1), START, None),
        Judgement::Resume
    );
}

#[test]
fn a_replayed_older_record_is_stale() {
    let old = record(3, START - 100, START - 120, 10, 10);
    assert_eq!(judge(Some(&fresh(9)), &old, START, None), Judgement::Stale);
}

#[test]
fn a_record_swapped_before_the_stop_is_stale() {
    // D5: a swap that finished before the stop never resumes. The last
    // second that is surely before the stop is START - STOP_SLACK_SECS - 1.
    let before = START - STOP_SLACK_SECS - 1;
    for (swapped_at, measured_at) in [
        (before, START + 30),
        (before - 60, START + 30),
        (before, START - 30),
    ] {
        let r = record(2, swapped_at, measured_at, 5, 5);
        assert_eq!(
            judge(None, &r, START, None),
            Judgement::Stale,
            "swapped_at={swapped_at} measured_at={measured_at}"
        );
    }
}

#[test]
fn a_swap_in_the_seconds_of_the_stop_resumes() {
    // Crusty round 1 on #2141: times are whole seconds and the stop comes
    // a moment before the hook starts. A swap that finished in the hook's
    // start second, or in the second before it, may have finished after the
    // stop; judging it stale would leave the session waiting for a swap that
    // may never come.
    assert_eq!(STOP_SLACK_SECS, 1);
    for swapped_at in [START, START - STOP_SLACK_SECS] {
        let r = record(2, swapped_at, swapped_at - 30, 5, 5);
        assert_eq!(
            judge(None, &r, START, None),
            Judgement::Resume,
            "swapped_at={swapped_at}"
        );
    }
}

#[test]
fn a_snapshot_swapped_between_the_stop_and_the_hook_start_resumes() {
    // Crusty round 1 on #2141: a swap that lands between the stop and the
    // hook's start is the record the hook notes. It is not `Same`: the
    // session stopped on the login swapped away from.
    for swapped_at in [START, START - STOP_SLACK_SECS] {
        let snap = record(4, swapped_at, swapped_at - 30, 5, 5);
        assert_eq!(
            judge(Some(&snap), &snap, START, None),
            Judgement::Resume,
            "swapped_at={swapped_at}"
        );
        let full = record(4, swapped_at, swapped_at - 30, 100, 5);
        assert_eq!(judge(Some(&full), &full, START, None), Judgement::NoRoom);
    }
    // A snapshot from before the stop is still `Same`.
    let old = record(4, START - STOP_SLACK_SECS - 1, START - 60, 5, 5);
    assert_eq!(judge(Some(&old), &old, START, None), Judgement::Same);
}

#[test]
fn a_stop_between_measurement_and_swap_resumes() {
    // The rotator measures, then swaps tens of seconds later. A session that
    // stops in that gap stopped on the login swapped away from, so the swap
    // resumes it; otherwise it would wait for a swap that never comes.
    for measured_at in [START, START - 1, START - 40] {
        let r = record(2, START + 1, measured_at, 5, 5);
        assert_eq!(
            judge(None, &r, START, None),
            Judgement::Resume,
            "measured_at={measured_at}"
        );
    }
}

#[test]
fn a_swap_the_session_was_resumed_on_never_resumes_it_again() {
    // A resumed session that hits a limit again within a second of the
    // swap must not be woken on that swap a second time: it ran again after
    // the resume, so it stopped after the swap.
    let r = record(3, START, START - 30, 5, 5);
    assert_eq!(judge(Some(&r), &r, START, Some(START)), Judgement::Same);
    assert_eq!(judge(None, &r, START, Some(START)), Judgement::Stale);
    // A swap after that resume still resumes it.
    let next = record(4, START + 1, START - 10, 5, 5);
    assert_eq!(
        judge(Some(&r), &next, START, Some(START)),
        Judgement::Resume
    );
}

#[test]
fn a_full_window_is_no_room() {
    for (s, w) in [(100, 10), (10, 100), (100, 100), (255, 0)] {
        let r = record(2, START + 60, START + 30, s, w);
        assert_eq!(
            judge(None, &r, START, None),
            Judgement::NoRoom,
            "s={s} w={w}"
        );
    }
    let r = record(2, START + 60, START + 30, 99, 99);
    assert_eq!(judge(None, &r, START, None), Judgement::Resume);
}

#[test]
fn judge_checks_same_then_stale_then_no_room() {
    // Same wins over Stale and NoRoom.
    let stale_full = record(2, START - 2, START - 2, 100, 100);
    assert_eq!(
        judge(Some(&stale_full), &stale_full, START, None),
        Judgement::Same
    );
    // Stale wins over NoRoom.
    assert_eq!(judge(None, &stale_full, START, None), Judgement::Stale);
}
