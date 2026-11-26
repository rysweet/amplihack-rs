//! Issue #2909: the rotator reads any limit-kind block and wakes on events.
//! Pure functions and a temporary ledger; no sleeps, no daemon.
//!
//! Contract:
//! - `spend_blocks::valid_kind(&str) -> bool`: `^[a-z0-9_]{1,32}$`, on bytes.
//! - `valid_overflow` accepts any such reason; `ProfileName::parse`,
//!   `valid_span` and `MAX_BLOCKS` are unchanged.
//! - `wake::deadline(now: Instant, now_unix: i64, interval: Duration,
//!   &RotateLedger) -> Instant`: the earlier of `now + interval` and the
//!   earliest future reset among `overflow_blocked` and `platform_block`.
//!   A lapsed reset does not contribute; a far reset never lengthens a wait.
//! - `wake::Blocks::fell_due(since, now_unix) -> bool`: a reset that passed
//!   while the last cycle ran makes the next one run at once. Asked of the
//!   blocks that cycle began with as well, because its own save prunes them.
//! - `wake::Blocks::of(&RotateLedger)` and `Blocks::differ_at(&other, now)`:
//!   the blocks in force at `now` differ.
//! - `wake::moved(&Blocks, Option<&RotateLedger>, now_unix) -> bool`: a
//!   ledger write runs a cycle when, and only when, the blocks in force
//!   differ from those the last cycle began with, or the ledger could not
//!   be loaded (`None`). Who wrote the file does not matter: a save that
//!   keeps the blocks, whatever its bytes, is not a change, and a block a
//!   cycle's own save kept for another writer is.

use super::super::daemon::wake::{Blocks, deadline, moved};
use super::super::ledger::{
    MAX_BLOCK_SECS, MAX_BLOCKS, OverflowBlock, RotateLedger, RotateState, for_older_readers,
};
use super::super::step_block::note_limit_block;
use super::{active_blocks, prune_blocks, valid_kind};
use crate::commands::profile::name::ProfileName;
use crate::commands::recipe::run::limit_patterns::LimitKind;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

const NOW: i64 = 1_793_389_376;

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

/// The race the daemon-level test met: a step records a limit while a cycle
/// measures, under the lock, and the cycle's own save keeps it. The file is
/// then the rotator's own write, yet the cycle decided without the block,
/// so the blocks it began with must differ from the blocks in force.
#[test]
fn a_block_kept_by_the_rotators_own_save_still_counts_as_a_change() {
    let (_t, rs) = root();
    let began = Blocks::of(&RotateLedger::default());
    // The cycle's save, made after the step's record, carries the block.
    let saved = ledger_with("work", block("session_limit", NOW, NOW + 3 * 3600));
    rs.save_ledger(&saved).unwrap();
    let read = rs.load_ledger().unwrap().expect("a ledger");
    assert!(
        moved(&began, Some(&read), NOW),
        "the block was not there when the cycle began"
    );
}

/// What replaces a record of the rotator's own bytes: every save it makes
/// for its own records keeps the blocks, so none starts a cycle, whatever
/// bytes it wrote and whoever wrote them.
#[test]
fn a_save_that_keeps_the_blocks_is_not_a_change_whatever_its_bytes() {
    let (_t, rs) = root();
    let l = ledger_with("spare", block("week_limit", NOW - 60, NOW + 3600));
    let began = Blocks::of(&l);
    // The rotator's own save of a cycle's records: other fields change.
    let mut after = l.clone();
    after.live = Some("work".into());
    after.swapped_at = Some(NOW);
    rs.save_ledger(&after).unwrap();
    let read = rs.load_ledger().unwrap().expect("a ledger");
    assert!(!moved(&began, Some(&read), NOW), "the rotator's own save");
    // The same ledger written by another process, compact rather than
    // pretty: every byte may differ, and it is still not a change.
    write_raw(&rs, serde_json::to_value(&after).unwrap());
    let read = rs.load_ledger().unwrap().expect("a ledger");
    assert!(
        !moved(&began, Some(&read), NOW),
        "another writer, same blocks"
    );
}

#[test]
fn a_new_block_from_another_writer_is_a_change() {
    let (_t, rs) = root();
    let began = Blocks::of(&RotateLedger::default());
    let l = ledger_with("work", block("credit_limit", NOW, NOW + 3600));
    write_raw(&rs, serde_json::to_value(&l).unwrap());
    let read = rs.load_ledger().unwrap().expect("a ledger");
    assert!(moved(&began, Some(&read), NOW + 1));
}

/// The bound that replaces a wall-clock floor. A writer keeps a block in
/// force as it is, so the second step to meet the same limit changes no
/// block and starts no cycle; twenty-five runs on one login cost one
/// cycle, not twenty-five.
#[test]
fn the_same_limit_recorded_again_starts_no_second_cycle() {
    let (t, rs) = root();
    rs.save_ledger(&RotateLedger::default()).unwrap();
    let work = ProfileName::parse("work").unwrap();
    let began = Blocks::of(&rs.load_ledger().unwrap().expect("a ledger"));
    note_limit_block(t.path(), &work, NOW, Some(NOW + 3600), LimitKind::Spend);
    let first = rs.load_ledger().unwrap().expect("a ledger");
    assert!(
        moved(&began, Some(&first), NOW + 1),
        "the first record is a change"
    );
    let next = Blocks::of(&first);
    note_limit_block(
        t.path(),
        &work,
        NOW + 60,
        Some(NOW + 7200),
        LimitKind::Spend,
    );
    let second = rs.load_ledger().unwrap().expect("a ledger");
    assert_eq!(first.overflow_blocked, second.overflow_blocked);
    assert!(
        !moved(&next, Some(&second), NOW + 61),
        "the second record of the same limit is not"
    );
}

/// The ledger as the watch loop reads it on a file event: one that does
/// not load is `None`, a missing one is an empty ledger.
fn as_read(rs: &RotateState) -> Option<RotateLedger> {
    rs.load_ledger().ok().map(Option::unwrap_or_default)
}

#[test]
fn an_unloadable_ledger_runs_a_cycle_and_a_missing_one_reads_as_empty() {
    let empty = Blocks::of(&RotateLedger::default());
    let (_t, rs) = root();
    let missing = as_read(&rs);
    assert_eq!(missing.as_ref(), Some(&RotateLedger::default()));
    assert!(!moved(&empty, missing.as_ref(), NOW), "nothing was blocked");
    let held = Blocks::of(&ledger_with("spare", block("week_limit", NOW, NOW + 60)));
    assert!(
        moved(&held, missing.as_ref(), NOW),
        "a block in force is gone"
    );
    let bad = ledger_with("spare", block("Not A Token", NOW, NOW + 60));
    write_raw(&rs, serde_json::to_value(bad).unwrap());
    let unloadable = as_read(&rs);
    assert!(unloadable.is_none(), "this ledger cannot be loaded");
    assert!(
        moved(&empty, unloadable.as_ref(), NOW),
        "the cycle reports why"
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

/// Issue #3105 reverses the #2890 trade-off this test used to pin: a block
/// of any kind is kept in the ledger written beside a rotator from another
/// binary, because a block a step records for a plan login cannot be derived
/// again, and dropping it let the rotator swap to a spend-blocked login. An
/// older amplihack that refuses the kind refuses the ledger loudly until it
/// is restarted, rather than losing the block in silence. Pinned with the
/// new kinds, so the guarantee does not rest on the old literal.
#[test]
fn an_older_reader_is_given_every_block_of_any_kind() {
    for reason in [
        "spend_limit",
        "session_limit",
        "week_limit",
        "credit_limit",
        "odd_but_valid",
    ] {
        let l = ledger_with("spare", block(reason, NOW, NOW + 600));
        assert_eq!(
            for_older_readers(&l, true).overflow_blocked,
            l.overflow_blocked,
            "{reason}: kept for an older reader"
        );
        assert_eq!(
            for_older_readers(&l, false).overflow_blocked.len(),
            1,
            "{reason}: this binary keeps it"
        );
    }
}
