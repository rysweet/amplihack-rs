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
//! Issue #2236 (D1 to D4): the stop time from the transcript.
//! - `read_stop_time(transcript, start) -> Option<i64>`: the `timestamp` of
//!   the last complete, non-empty line in at most `STOP_TAIL_BYTES` (65 536)
//!   from the end, floored to whole epoch seconds, and only when
//!   `0 < t <= start`. No symlink, a regular file owned by the euid, not
//!   writable by group or others; anything else is `None`.
//! - `after_stop_at(swapped_at, start, stop, resumed_at)`: the threshold is
//!   `min(stop, start - STOP_SLACK_SECS)`, or `start - STOP_SLACK_SECS`
//!   with no stop. `after_stop` is `after_stop_at(.., None, ..)`.
//! - `judge_at(snapshot, record, start, stop, resumed_at)`: `judge` with the
//!   stop; `judge` is `judge_at(.., None, ..)`.
//!
//! Every test uses a temporary root; nothing reads the real profile root.

#![cfg(unix)]

use amplihack_utils::rotator_status::swap_record::{
    Judgement, MAX_SWAP_BYTES, STOP_SLACK_SECS, STOP_TAIL_BYTES, SWAP_FILE, SwapReading,
    SwapRecord, after_stop, after_stop_at, judge, judge_at, noted_after_stop, parse_swap,
    read_stop_time, read_swap, rotate_dir, stopped_on_outgoing, swap_path,
};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

const NOW: i64 = 1_792_592_000;
/// The limit hit (the hook's start) is 10 minutes before `NOW`.
const START: i64 = NOW - 600;

fn record(seq: u64, swapped_at: i64, measured_at: i64, session: u8, week: u8) -> SwapRecord {
    SwapRecord {
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

// ── read_stop_time (#2236) ──────────────────────────────────────────────────

/// `t` as an RFC 3339 UTC time with whole seconds.
fn rfc(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// One transcript line, as Claude Code writes them: other fields around
/// the `timestamp`, which is all the reader may look at.
fn entry(ts: &str) -> String {
    format!(
        r#"{{"type":"assistant","uuid":"u-1","timestamp":"{ts}","message":{{"role":"assistant","content":"You've hit your limit"}}}}"#
    )
}

/// A temporary directory with `bytes` at `t.jsonl`, mode `mode`.
fn transcript_with(bytes: &[u8], mode: u32) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.jsonl");
    fs::write(&p, bytes).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
    (dir, p)
}

/// A line of exactly `len` bytes, newline included, holding `ts`. The
/// padding sits in an ignored field.
fn padded_entry(ts: &str, len: usize) -> String {
    let bare = format!(r#"{{"timestamp":"{ts}","pad":""}}"#);
    let pad = len - bare.len() - 1;
    format!(r#"{{"timestamp":"{ts}","pad":"{}"}}"#, "x".repeat(pad)) + "\n"
}

#[test]
fn the_tail_limit_is_64_kib() {
    assert_eq!(STOP_TAIL_BYTES, 65_536);
    assert_eq!(rfc(START), "2026-10-21T14:03:20Z", "the test epochs");
}

#[test]
fn read_stop_time_returns_the_last_entrys_timestamp() {
    let body = format!(
        "{}\n{}\n{}\n",
        entry(&rfc(START - 400)),
        entry(&rfc(START - 90)),
        entry(&rfc(START - 5))
    );
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 5));
}

#[test]
fn read_stop_time_floors_fractional_seconds() {
    // 2026-10-21T14:03:15.999Z is START - 5 and most of a second.
    let body = entry("2026-10-21T14:03:15.999Z") + "\n";
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 5));
    let body = entry("2026-10-21T14:03:15.001Z") + "\n";
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 5));
}

#[test]
fn read_stop_time_honours_an_rfc3339_offset() {
    // The same instant as START - 5, written with a +01:00 offset.
    let body = entry("2026-10-21T15:03:15+01:00") + "\n";
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 5));
}

#[test]
fn read_stop_time_ignores_a_partial_trailing_line() {
    // Claude Code may be mid-write: the bytes after the last newline are not
    // an entry yet, even when they would parse.
    let body = format!("{}\n{}", entry(&rfc(START - 30)), entry(&rfc(START - 2)));
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 30));
    let body = format!("{}\n{{\"timestamp\":\"20", entry(&rfc(START - 30)));
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 30));
}

#[test]
fn read_stop_time_skips_trailing_empty_lines() {
    let body = format!("{}\n\n\n", entry(&rfc(START - 7)));
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 7));
}

#[test]
fn read_stop_time_reads_only_the_tail_of_a_large_transcript() {
    // 70 KiB of older entries, then the last one.
    let mut body = String::new();
    while body.len() < 70 * 1024 {
        body += &entry(&rfc(START - 3600));
        body.push('\n');
    }
    body += &entry(&rfc(START - 4));
    body.push('\n');
    assert!(body.len() as u64 > STOP_TAIL_BYTES);
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 4));
}

#[test]
fn a_file_of_exactly_the_tail_limit_keeps_its_first_line() {
    let body = padded_entry(&rfc(START - 6), STOP_TAIL_BYTES as usize);
    assert_eq!(body.len() as u64, STOP_TAIL_BYTES);
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), Some(START - 6));
}

#[test]
fn a_read_that_starts_mid_line_drops_that_line() {
    // 65 537 bytes: a leading space, then a 65 536-byte line. The tail read
    // starts one byte in, at what looks like a whole entry but is the rest
    // of a longer line, so it is not trusted.
    let body = format!(
        " {}",
        padded_entry(&rfc(START - 6), STOP_TAIL_BYTES as usize)
    );
    assert_eq!(body.len() as u64, STOP_TAIL_BYTES + 1);
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), None);
}

#[test]
fn a_last_entry_over_the_tail_limit_is_none() {
    let body = format!(
        "{}\n{}",
        entry(&rfc(START - 60)),
        padded_entry(&rfc(START - 6), STOP_TAIL_BYTES as usize + 100)
    );
    let (_d, p) = transcript_with(body.as_bytes(), 0o600);
    assert_eq!(read_stop_time(&p, START), None);
}

#[test]
fn read_stop_time_never_follows_a_symlink() {
    let (d, real) = transcript_with((entry(&rfc(START - 5)) + "\n").as_bytes(), 0o600);
    let link = d.path().join("link.jsonl");
    symlink(&real, &link).unwrap();
    assert_eq!(read_stop_time(&real, START), Some(START - 5));
    assert_eq!(read_stop_time(&link, START), None);
}

#[test]
fn read_stop_time_refuses_a_transcript_others_can_write() {
    let body = entry(&rfc(START - 5)) + "\n";
    for mode in [0o620, 0o602, 0o666] {
        let (_d, p) = transcript_with(body.as_bytes(), mode);
        assert_eq!(read_stop_time(&p, START), None, "mode {mode:o}");
    }
    // Readable by others, or a single link that is not 0600, is accepted:
    // transcripts are not private files.
    for mode in [0o644, 0o640, 0o400] {
        let (_d, p) = transcript_with(body.as_bytes(), mode);
        assert_eq!(read_stop_time(&p, START), Some(START - 5), "mode {mode:o}");
    }
}

#[test]
fn read_stop_time_refuses_a_directory_or_a_fifo() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("t.jsonl");
    fs::create_dir(&dir).unwrap();
    assert_eq!(read_stop_time(&dir, START), None);

    // A FIFO with no writer must neither block the open nor be read.
    let fifo = d.path().join("fifo.jsonl");
    let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert_eq!(read_stop_time(&fifo, START), None);
}

#[test]
fn read_stop_time_without_a_file_is_none() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(read_stop_time(&d.path().join("missing.jsonl"), START), None);
    let (_d, p) = transcript_with(b"", 0o600);
    assert_eq!(read_stop_time(&p, START), None);
    let (_d, p) = transcript_with(b"\n\n", 0o600);
    assert_eq!(read_stop_time(&p, START), None);
}

#[test]
fn a_malformed_last_line_is_none_and_never_falls_back() {
    let good = entry(&rfc(START - 30));
    for last in [
        "not json".to_string(),
        "{".to_string(),
        "[]".to_string(),
        r#"{"type":"assistant"}"#.to_string(),
        format!(r#"{{"timestamp":{}}}"#, START - 5),
        r#"{"timestamp":null}"#.to_string(),
        r#"{"timestamp":"2026-10-21 14:03:15"}"#.to_string(),
        r#"{"timestamp":"yesterday"}"#.to_string(),
        r#"{"timestamp":"2026-10-21T14:03:15"}"#.to_string(),
    ] {
        let body = format!("{good}\n{last}\n");
        let (_d, p) = transcript_with(body.as_bytes(), 0o600);
        assert_eq!(read_stop_time(&p, START), None, "{last}");
    }
    let (_d, p) = transcript_with(b"\xff\xfe\n", 0o600);
    assert_eq!(read_stop_time(&p, START), None, "not UTF-8");
}

#[test]
fn a_stop_time_outside_zero_to_start_is_none() {
    for (ts, want) in [
        (rfc(START), Some(START)),
        (rfc(START + 1), None),
        (rfc(START + 3600), None),
        ("9999-12-31T23:59:59Z".to_string(), None),
        ("1970-01-01T00:00:00Z".to_string(), None),
        ("1969-12-31T23:59:59Z".to_string(), None),
        ("1970-01-01T00:00:01Z".to_string(), Some(1)),
    ] {
        let (_d, p) = transcript_with((entry(&ts) + "\n").as_bytes(), 0o600);
        assert_eq!(read_stop_time(&p, START), want, "{ts}");
    }
}

#[test]
fn read_stop_time_changes_nothing() {
    let body = entry(&rfc(START - 5)) + "\n";
    let (d, p) = transcript_with(body.as_bytes(), 0o640);
    let before = fs::metadata(&p).unwrap();
    assert_eq!(read_stop_time(&p, START), Some(START - 5));
    let after = fs::metadata(&p).unwrap();
    assert_eq!(fs::read(&p).unwrap(), body.as_bytes());
    assert_eq!(before.permissions(), after.permissions());
    let names: Vec<_> = fs::read_dir(d.path()).unwrap().collect();
    assert_eq!(names.len(), 1, "no file created beside the transcript");
}

// ── after_stop_at and judge_at (#2236) ──────────────────────────────────────

/// The stop the session's transcript records, ten seconds before the
/// hook's late start.
const STOP: i64 = START - 10;

#[test]
fn after_stop_at_without_a_stop_is_after_stop() {
    for swapped_at in [
        START - 600,
        START - 2,
        START - 1,
        START,
        START + 1,
        START + 600,
    ] {
        for resumed_at in [None, Some(START - 600), Some(START - 1), Some(START)] {
            assert_eq!(
                after_stop_at(swapped_at, START, None, resumed_at),
                after_stop(swapped_at, START, resumed_at),
                "swapped_at={swapped_at} resumed_at={resumed_at:?}"
            );
        }
    }
    // Today's threshold: START - STOP_SLACK_SECS.
    assert!(after_stop_at(START - 1, START, None, None));
    assert!(!after_stop_at(START - 2, START, None, None));
}

#[test]
fn a_swap_between_a_known_stop_and_a_late_start_is_after_the_stop() {
    // The hook started ten seconds after the stop; a swap three seconds
    // after the stop finished after it.
    assert!(after_stop_at(STOP + 3, START, Some(STOP), None));
    // A swap in the stop's own second counts as after it (the cheap way).
    assert!(after_stop_at(STOP, START, Some(STOP), None));
    assert!(after_stop_at(START - 1, START, Some(STOP), None));
    // Before the stop is before it.
    assert!(!after_stop_at(STOP - 1, START, Some(STOP), None));
    assert!(!after_stop_at(STOP - 5, START, Some(STOP), None));
}

#[test]
fn a_stop_time_never_narrows_the_slack_threshold() {
    // D2: min(stop, start - STOP_SLACK_SECS). A stop in the start second
    // keeps today's allowance of one second.
    assert!(after_stop_at(START - 1, START, Some(START), None));
    assert!(!after_stop_at(START - 2, START, Some(START), None));
    assert!(after_stop_at(START - 1, START, Some(START - 1), None));
    assert!(!after_stop_at(START - 2, START, Some(START - 1), None));
    // Even a stop handed in past the start (the reader refuses one).
    assert!(after_stop_at(START - 1, START, Some(START + 50), None));
}

#[test]
fn the_last_resume_rule_still_applies_with_a_stop() {
    let swap = STOP + 3;
    assert!(!after_stop_at(swap, START, Some(STOP), Some(swap)));
    assert!(!after_stop_at(swap, START, Some(STOP), Some(swap + 1)));
    assert!(after_stop_at(swap, START, Some(STOP), Some(swap - 1)));
}

#[test]
fn after_stop_at_saturates_at_the_extremes() {
    assert!(after_stop_at(i64::MIN, i64::MIN, None, None));
    assert!(after_stop_at(i64::MIN, i64::MIN, Some(i64::MIN), None));
    assert!(after_stop_at(i64::MAX, i64::MAX, Some(1), None));
}

#[test]
fn judge_without_a_stop_is_judge_at_with_none() {
    let cases = [
        record(2, START - 600, START - 630, 5, 5),
        record(2, START - 2, START - 30, 5, 5),
        record(2, START - 1, START - 30, 5, 5),
        record(2, START, START - 30, 100, 5),
        fresh(3),
    ];
    for r in &cases {
        for snap in [None, Some(r), Some(&cases[0])] {
            for resumed_at in [None, Some(START - 1), Some(START)] {
                assert_eq!(
                    judge(snap, r, START, resumed_at),
                    judge_at(snap, r, START, None, resumed_at),
                    "{r:?} snap={snap:?} resumed_at={resumed_at:?}"
                );
            }
        }
    }
}

#[test]
fn a_snapshot_after_a_known_stop_resumes_where_the_slack_alone_waits() {
    // #2236: the hook started ten seconds after the stop and noted a swap
    // made three seconds after it. Without the stop that snapshot is Same
    // and the session waits up to seven days.
    let snap = record(4, STOP + 3, STOP - 20, 12, 40);
    assert_eq!(judge(Some(&snap), &snap, START, None), Judgement::Same);
    assert_eq!(
        judge_at(Some(&snap), &snap, START, Some(STOP), None),
        Judgement::Resume
    );
    let full = record(4, STOP + 3, STOP - 20, 100, 40);
    assert_eq!(
        judge_at(Some(&full), &full, START, Some(STOP), None),
        Judgement::NoRoom
    );
    // A new record from between the stop and the start resumes too.
    assert_eq!(
        judge_at(None, &snap, START, Some(STOP), None),
        Judgement::Resume
    );
    assert_eq!(judge(None, &snap, START, None), Judgement::Stale);
}

#[test]
fn a_snapshot_before_a_known_stop_is_same_and_a_new_one_stale() {
    let snap = record(4, STOP - 5, STOP - 30, 12, 40);
    assert_eq!(
        judge_at(Some(&snap), &snap, START, Some(STOP), None),
        Judgement::Same
    );
    assert_eq!(
        judge_at(None, &snap, START, Some(STOP), None),
        Judgement::Stale
    );
}

#[test]
fn a_snapshot_dated_after_the_start_is_same_even_with_a_stop() {
    let ahead = record(4, START + 5, START - 20, 12, 40);
    assert_eq!(
        judge_at(Some(&ahead), &ahead, START, Some(STOP), None),
        Judgement::Same
    );
}

#[test]
fn a_swap_the_session_was_resumed_on_is_same_even_with_a_stop() {
    let r = record(3, STOP + 3, STOP - 20, 5, 5);
    assert_eq!(
        judge_at(Some(&r), &r, START, Some(STOP), Some(STOP + 3)),
        Judgement::Same
    );
    assert_eq!(
        judge_at(None, &r, START, Some(STOP), Some(STOP + 3)),
        Judgement::Stale
    );
}

// ── Issue #2235: the login the session stopped on, for the event key ────────

#[test]
fn with_a_stop_a_swap_between_the_stop_and_the_start_is_the_outgoing_login() {
    let r = record(6, START - 10, START - 40, 12, 40);
    assert!(stopped_on_outgoing(&r, START, Some(START - 30), None));
    // The stop and the swap in the same second, and the swap in the start
    // second, both count.
    assert!(stopped_on_outgoing(&r, START, Some(START - 10), None));
    assert!(stopped_on_outgoing(&r, START - 10, Some(START - 30), None));
}

#[test]
fn with_a_stop_the_last_resume_never_changes_the_login() {
    // The same stop delivered again after its resume was recorded: the
    // resume is later than the swap, yet the key must not change.
    let r = record(6, START - 10, START - 40, 12, 40);
    let stop = Some(START - 30);
    assert!(!noted_after_stop(&r, START + 60, stop, Some(START)));
    assert!(stopped_on_outgoing(&r, START + 60, stop, Some(START)));
    assert_eq!(
        stopped_on_outgoing(&r, START, stop, None),
        stopped_on_outgoing(&r, START + 60, stop, Some(START + 1))
    );
}

#[test]
fn with_a_stop_a_swap_before_the_stop_is_the_live_login() {
    // A limit on the new login after the swap: the session stopped on it.
    let r = record(6, START - 10, START - 40, 12, 40);
    assert!(!stopped_on_outgoing(&r, START, Some(START - 9), None));
}

#[test]
fn with_a_stop_in_the_start_second_the_slack_second_is_the_outgoing_login() {
    // #2235: the key uses the same threshold as the resume decision. A
    // swap in the second before a stop in the start second is resumed
    // through the slack allowance, so the event names its outgoing login.
    let r = record(6, START - 1, START - 40, 12, 40);
    assert!(noted_after_stop(&r, START, Some(START), None));
    assert!(stopped_on_outgoing(&r, START, Some(START), None));
    // Two seconds before is outside the allowance for both rules.
    let r = record(6, START - 2, START - 40, 12, 40);
    assert!(!noted_after_stop(&r, START, Some(START), None));
    assert!(!stopped_on_outgoing(&r, START, Some(START), None));
}

#[test]
fn with_a_stop_the_key_agrees_with_the_resume_rule_before_any_resume() {
    // One predicate for one fact: for any stop and swap not later than the
    // start, with no resume yet, the key and the resume decision agree.
    for stop in START - 5..=START {
        for at in START - 8..=START {
            let r = record(6, at, START - 40, 12, 40);
            assert_eq!(
                stopped_on_outgoing(&r, START, Some(stop), None),
                noted_after_stop(&r, START, Some(stop), None),
                "stop {stop}, swapped_at {at}"
            );
        }
    }
}

#[test]
fn with_a_stop_a_swap_dated_after_the_start_is_the_live_login() {
    let r = record(6, START + 5, START - 40, 12, 40);
    assert!(!stopped_on_outgoing(&r, START, Some(START - 30), None));
}

#[test]
fn without_a_stop_it_is_noted_after_stop() {
    for (at, resumed) in [
        (START - 2, None),
        (START - 1, None),
        (START, None),
        (START, Some(START)),
        (START + 1, None),
    ] {
        let r = record(6, at, START - 40, 12, 40);
        assert_eq!(
            stopped_on_outgoing(&r, START, None, resumed),
            noted_after_stop(&r, START, None, resumed),
            "swapped_at {at}, resumed {resumed:?}"
        );
    }
}
