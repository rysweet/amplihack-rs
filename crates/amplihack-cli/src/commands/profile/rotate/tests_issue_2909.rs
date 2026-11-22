//! Issue #2909: the rotator reads any limit-kind block and wakes on events.
//! Pure functions and a temporary ledger; no sleeps, no daemon.
//!
//! Contract:
//! - `spend_blocks::valid_kind(&str) -> bool`: `^[a-z0-9_]{1,32}$`, on bytes.
//! - `spend_blocks::kind_label(&str) -> &'static str`: a fixed label for any
//!   token, so a stored reason never reaches a log line.
//! - `valid_overflow` accepts any such reason; `ProfileName::parse`,
//!   `valid_span` and `MAX_BLOCKS` are unchanged.
//! - `wake::deadline(now: Instant, now_unix: i64, interval: Duration,
//!   &RotateLedger) -> Instant`: the earlier of `now + interval` and the
//!   earliest future reset among `overflow_blocked` and `platform_block`.
//!   A lapsed reset does not contribute; a far reset never lengthens a wait.
//! - `wake::coalesce_floor(interval: Duration) -> Duration`:
//!   `min(interval, 5s)`.
//! - `wake::event_due(started, now, floor) -> Instant`: a ledger write runs
//!   a cycle at once, unless the last cycle ended less than `floor` ago.
//! - `wake::Blocks::fell_due(since, now_unix) -> bool`: a reset that passed
//!   while the last cycle ran makes the next one run at once. Asked of the
//!   blocks that cycle began with as well, because its own save prunes them.
//! - `wake::note_written(&[u8])` and `wake::is_self_write(&Path) -> bool`:
//!   identical bytes to the last write are this rotator's own; anything
//!   else, a missing file included, is not.
//! - `wake::Blocks::of(&RotateLedger)` and `Blocks::differ_at(&other, now)`:
//!   a block recorded while a cycle ran, and kept by that cycle's own save,
//!   differs from the blocks the cycle began with, so it still wakes the
//!   rotator although the file is byte for byte its own write.

use super::super::daemon::wake::{
    Blocks, coalesce_floor, deadline, event_due, is_self_write, note_written,
};
use super::super::ledger::{
    MAX_BLOCK_SECS, MAX_BLOCKS, OverflowBlock, RotateLedger, RotateState, for_older_readers,
};
use super::{active_blocks, kind_label, prune_blocks, valid_kind};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const NOW: i64 = 1_793_389_376;

/// `wake` keeps one process-wide record of the last write.
static SERIAL: Mutex<()> = Mutex::new(());

fn root() -> (tempfile::TempDir, RotateState) {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(tmp.path());
    rs.ensure().unwrap();
    (tmp, rs)
}

fn block(reason: &str, at: i64, until: i64) -> OverflowBlock {
    OverflowBlock {
        at,
        until,
        reason: reason.into(),
    }
}

fn ledger_with(name: &str, b: OverflowBlock) -> RotateLedger {
    let mut l = RotateLedger::default();
    l.overflow_blocked.insert(name.into(), b);
    l
}

fn write_raw(rs: &RotateState, v: serde_json::Value) {
    fs::write(&rs.ledger, v.to_string()).unwrap();
    fs::set_permissions(&rs.ledger, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn kind_tokens_follow_the_short_token_rule() {
    for ok in [
        "spend_limit",
        "session_limit",
        "week_limit",
        "credit",
        "a",
        "x9_",
    ] {
        assert!(valid_kind(ok), "{ok}");
    }
    assert!(valid_kind(&"a".repeat(32)));
    for bad in [
        "",
        "Spend",
        "a b",
        "a-b",
        "a.b",
        "../x",
        "caf\u{e9}",
        "a\n",
        "a\0",
    ] {
        assert!(!valid_kind(bad), "{bad:?}");
    }
    assert!(!valid_kind(&"a".repeat(33)));
}

#[test]
fn a_kind_label_is_fixed_text_for_any_token() {
    let odd = "token_like_sk_ant_0123";
    assert!(valid_kind(odd));
    let label: &'static str = kind_label(odd);
    assert!(!label.contains(odd), "{label}");
    assert!(!kind_label("spend_limit").is_empty());
    assert_eq!(kind_label(odd), kind_label("another_odd_one"));
}

#[test]
fn a_non_spend_reason_blocks_its_login_and_the_ledger_still_loads() {
    let (_t, rs) = root();
    for reason in [
        "session_limit",
        "week_limit",
        "credit_limit",
        "odd_but_valid",
    ] {
        let l = ledger_with("spare", block(reason, NOW, NOW + 3600));
        write_raw(&rs, serde_json::to_value(&l).unwrap());
        let loaded = rs.load_ledger().expect(reason).expect("a ledger");
        assert!(
            active_blocks(&loaded, NOW + 1).contains_key("spare"),
            "{reason}"
        );
    }
}

#[test]
fn a_reason_outside_the_rule_still_refuses_the_ledger() {
    let (_t, rs) = root();
    for reason in ["", "Has Space", "UPPER", &"a".repeat(33)] {
        let l = ledger_with("spare", block(reason, NOW, NOW + 3600));
        write_raw(&rs, serde_json::to_value(&l).unwrap());
        assert!(rs.load_ledger().is_err(), "{reason:?}");
    }
}

#[test]
fn the_other_block_rules_are_not_loosened() {
    let (_t, rs) = root();
    let refused = |l: RotateLedger| {
        write_raw(&rs, serde_json::to_value(&l).unwrap());
        assert!(rs.load_ledger().is_err());
    };
    refused(ledger_with("../x", block("session_limit", NOW, NOW + 60)));
    refused(ledger_with("spare", block("session_limit", NOW, NOW)));
    refused(ledger_with("spare", block("session_limit", 0, 60)));
    refused(ledger_with(
        "spare",
        block("session_limit", NOW, NOW + MAX_BLOCK_SECS + 1),
    ));
    let mut many = RotateLedger::default();
    for i in 0..=MAX_BLOCKS {
        many.overflow_blocked
            .insert(format!("p{i}"), block("session_limit", NOW, NOW + 60));
    }
    refused(many);
}

#[test]
fn a_block_ends_at_its_reset_and_not_before() {
    let mut l = ledger_with("spare", block("week_limit", NOW, NOW + 600));
    assert!(active_blocks(&l, NOW + 599).contains_key("spare"));
    assert!(active_blocks(&l, NOW + 600).is_empty());
    prune_blocks(&mut l, NOW + 599);
    assert_eq!(l.overflow_blocked.len(), 1);
    prune_blocks(&mut l, NOW + 600);
    assert!(
        l.overflow_blocked.is_empty(),
        "a passed reset releases the login"
    );
}

#[test]
fn the_deadline_is_the_interval_when_nothing_is_recorded() {
    let now = Instant::now();
    let iv = Duration::from_secs(600);
    assert_eq!(deadline(now, NOW, iv, &RotateLedger::default()), now + iv);
}

#[test]
fn a_near_reset_shortens_the_wait_to_that_reset() {
    let now = Instant::now();
    let l = ledger_with("spare", block("session_limit", NOW - 10, NOW + 30));
    assert_eq!(
        deadline(now, NOW, Duration::from_secs(3600), &l),
        now + Duration::from_secs(30)
    );
}

#[test]
fn the_earliest_of_several_resets_wins_and_a_platform_block_counts() {
    let now = Instant::now();
    let mut l = ledger_with("a", block("week_limit", NOW - 10, NOW + 900));
    l.overflow_blocked
        .insert("b".into(), block("session_limit", NOW - 10, NOW + 120));
    l.platform_block = Some(OverflowBlock::key("key_exhausted", NOW - 10, NOW + 45));
    assert_eq!(
        deadline(now, NOW, Duration::from_secs(3600), &l),
        now + Duration::from_secs(45)
    );
}

#[test]
fn a_reset_beyond_the_interval_never_lengthens_the_wait() {
    let now = Instant::now();
    let iv = Duration::from_secs(600);
    let l = ledger_with("spare", block("week_limit", NOW, NOW + 86_400));
    assert_eq!(deadline(now, NOW, iv, &l), now + iv);
}

#[test]
fn a_lapsed_reset_does_not_contribute() {
    let now = Instant::now();
    let iv = Duration::from_secs(600);
    let l = ledger_with("spare", block("week_limit", NOW - 100, NOW - 1));
    assert_eq!(deadline(now, NOW, iv, &l), now + iv);
}

#[test]
fn an_interval_of_one_second_still_ticks_at_one_second() {
    let now = Instant::now();
    let iv = Duration::from_secs(1);
    let l = ledger_with("spare", block("week_limit", NOW, NOW + 500));
    assert_eq!(deadline(now, NOW, iv, &l), now + iv);
}

#[test]
fn the_coalescing_floor_is_the_smaller_of_the_interval_and_five_seconds() {
    assert_eq!(
        coalesce_floor(Duration::from_secs(1)),
        Duration::from_secs(1)
    );
    assert_eq!(
        coalesce_floor(Duration::from_secs(5)),
        Duration::from_secs(5)
    );
    assert_eq!(
        coalesce_floor(Duration::from_secs(600)),
        Duration::from_secs(5)
    );
}

#[test]
fn a_reset_that_fell_due_during_the_last_cycle_runs_the_next_at_once() {
    // The last cycle began 30 seconds ago; spare's reset passed 10 seconds
    // ago, while it ran, so that cycle still held spare out.
    let l = ledger_with("spare", block("session_limit", NOW - 3600, NOW - 10));
    assert!(Blocks::of(&l).fell_due(NOW - 30, NOW));
    // `deadline` looks only forward, which is why this check exists.
    let now = Instant::now();
    let iv = Duration::from_secs(3600);
    assert_eq!(deadline(now, NOW, iv, &l), now + iv);
}

#[test]
fn a_reset_already_past_when_the_last_cycle_began_does_not_cause_another() {
    let l = ledger_with("spare", block("session_limit", NOW - 3600, NOW - 60));
    assert!(!Blocks::of(&l).fell_due(NOW - 30, NOW));
    // Nor does one still ahead, and a platform block counts like any other.
    let mut l = ledger_with("spare", block("session_limit", NOW - 3600, NOW + 60));
    assert!(!Blocks::of(&l).fell_due(NOW - 30, NOW));
    l.platform_block = Some(OverflowBlock::key("key_exhausted", NOW - 3600, NOW - 5));
    assert!(Blocks::of(&l).fell_due(NOW - 30, NOW));
}

/// What a running rotator meets: the cycle that held spare out saves the
/// ledger after spare's reset, and the save prunes the block, so the ledger
/// the next wait reads no longer says spare was ever blocked. The blocks
/// that cycle began with still do, and they decide.
#[test]
fn a_reset_that_fell_due_during_the_last_cycle_counts_after_its_save_pruned_it() {
    let began = ledger_with("spare", block("session_limit", NOW - 3600, NOW - 10));
    let mut saved = began.clone();
    prune_blocks(&mut saved, NOW - 2);
    assert!(saved.overflow_blocked.is_empty(), "the save pruned it");
    assert!(
        !Blocks::of(&saved).fell_due(NOW - 30, NOW),
        "the ledger cannot say"
    );
    assert!(Blocks::of(&began).fell_due(NOW - 30, NOW));
    // Not a second time: the next cycle began after that reset.
    assert!(!Blocks::of(&began).fell_due(NOW, NOW + 30));
    // Nor for a block still in force, nor one already past at the start.
    let ahead = ledger_with("spare", block("session_limit", NOW - 3600, NOW + 60));
    assert!(!Blocks::of(&ahead).fell_due(NOW - 30, NOW));
    let past = ledger_with("spare", block("session_limit", NOW - 3600, NOW - 60));
    assert!(!Blocks::of(&past).fell_due(NOW - 30, NOW));
    let mut key = RotateLedger::default();
    key.platform_block = Some(OverflowBlock::key("key_exhausted", NOW - 3600, NOW - 5));
    assert!(
        Blocks::of(&key).fell_due(NOW - 30, NOW),
        "an API-key block too"
    );
}

#[test]
fn a_ledger_write_long_after_the_last_cycle_runs_a_cycle_at_once() {
    let started = Instant::now();
    let floor = coalesce_floor(Duration::from_secs(600));
    let seen = started + Duration::from_secs(300);
    assert_eq!(event_due(started, seen, floor), seen);
}

#[test]
fn a_ledger_write_straight_after_a_cycle_waits_out_the_rest_of_the_floor() {
    let started = Instant::now();
    let floor = coalesce_floor(Duration::from_secs(600));
    let seen = started + Duration::from_secs(2);
    assert_eq!(
        event_due(started, seen, floor),
        started + Duration::from_secs(5)
    );
}

#[test]
fn identical_bytes_are_a_self_write_and_any_difference_is_not() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_t, rs) = root();
    fs::write(&rs.ledger, b"{\"version\":1}").unwrap();
    note_written(b"{\"version\":1}");
    assert!(is_self_write(&rs.ledger));
    fs::write(&rs.ledger, b"{\"version\":1} ").unwrap();
    assert!(!is_self_write(&rs.ledger), "one byte differs");
}

#[test]
fn a_missing_or_oversized_file_reads_as_changed() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_t, rs) = root();
    note_written(b"x");
    assert!(!is_self_write(&rs.ledger), "missing");
    fs::write(&rs.ledger, vec![b'x'; 1024 * 1024]).unwrap();
    assert!(!is_self_write(&rs.ledger), "oversized");
}

#[test]
fn a_ledger_this_process_did_not_write_is_not_a_self_write() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_t, rs) = root();
    rs.save_ledger(&RotateLedger::default()).unwrap();
    assert!(
        is_self_write(&rs.ledger),
        "save_ledger records what it wrote"
    );
    let l = ledger_with("spare", block("session_limit", NOW, NOW + 60));
    fs::write(&rs.ledger, serde_json::to_vec(&l).unwrap()).unwrap();
    assert!(
        !is_self_write(&rs.ledger),
        "an external writer is not suppressed"
    );
}

/// The race the daemon-level test met: a step records a limit while a cycle
/// measures, under the lock, and the cycle's own save keeps it. The file is
/// then this process's own write, byte for byte, yet the cycle decided
/// without the block, so the blocks it began with must differ from the
/// blocks in force.
#[test]
fn a_block_kept_by_the_rotators_own_save_still_counts_as_a_change() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (_t, rs) = root();
    let began = Blocks::of(&RotateLedger::default());
    // The cycle's save, made after the step's record, carries the block.
    let saved = ledger_with("work", block("session_limit", NOW, NOW + 3 * 3600));
    rs.save_ledger(&saved).unwrap();
    assert!(
        is_self_write(&rs.ledger),
        "the file is this process's own write"
    );
    assert!(
        began.differ_at(&Blocks::of(&saved), NOW),
        "but the block was not there when the cycle began"
    );
}

#[test]
fn the_same_blocks_a_prune_and_a_lapsed_block_are_not_a_change() {
    let l = ledger_with("work", block("session_limit", NOW - 60, NOW + 600));
    assert!(!Blocks::of(&l).differ_at(&Blocks::of(&l), NOW), "unchanged");
    // A prune removes only a block whose reset has passed: out of both.
    let mut pruned = l.clone();
    pruned.overflow_blocked.clear();
    assert!(
        !Blocks::of(&l).differ_at(&Blocks::of(&pruned), NOW + 600),
        "a lapsed block is in force in neither"
    );
    assert!(
        Blocks::of(&l).differ_at(&Blocks::of(&pruned), NOW),
        "a block removed while in force (profile overflow clear) is a change"
    );
}

#[test]
fn a_new_or_moved_platform_block_is_a_change() {
    let mut l = RotateLedger::default();
    let before = Blocks::of(&l);
    l.platform_block = Some(OverflowBlock::key("key_exhausted", NOW, NOW + 600));
    assert!(
        before.differ_at(&Blocks::of(&l), NOW),
        "a new API-key block"
    );
    let mut later = l.clone();
    later.platform_block = Some(OverflowBlock::key("key_exhausted", NOW, NOW + 900));
    assert!(
        Blocks::of(&l).differ_at(&Blocks::of(&later), NOW),
        "a block whose reset moved"
    );
}

/// An older amplihack refuses a ledger whose `overflow_blocked` reason is
/// anything but `spend_limit`, so while one runs the field must still be
/// left out whatever kind of limit it holds. Pinned with the new kinds, so
/// the guarantee does not rest on the old literal.
#[test]
fn an_older_reader_never_sees_a_block_of_any_kind() {
    for reason in [
        "session_limit",
        "week_limit",
        "credit_limit",
        "odd_but_valid",
    ] {
        let l = ledger_with("spare", block(reason, NOW, NOW + 600));
        assert!(
            for_older_readers(&l, true).overflow_blocked.is_empty(),
            "{reason}"
        );
        assert_eq!(
            for_older_readers(&l, false).overflow_blocked.len(),
            1,
            "{reason}: this binary keeps it"
        );
    }
}
