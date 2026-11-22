//! Issue #2857, part 2: a limit wait ends early on a rotator signal only when
//! the login the retry will use has room.
//!
//! What happened on one host on 19 Nov 2026: the rotator swapped waiting runs'
//! logins between accounts that were at 98-100% session, and every swap woke
//! every wait. The first call after each wake returned a 429 that the runner
//! retried six times before the run ended indeterminate. Stopping the rotator
//! woke 31 more runs the same way, each logging `end reason=rotator capacity`.
//!
//! The rule these tests pin: a wait wakes only when the rotator's last cycle
//! says the login now live has room (`last_cycle.live_room == true`), whatever
//! else changed in the rotator's state. One record is judged differently: a
//! person's own `profile switch` (outcome `switched`, #1691) wakes a wait unless
//! it says `live_room: false`, because the rotator measured nothing and has no
//! bit to write for a login a person chose. Four routes end a wait early and all
//! four are gated the same way on a reset or capacity wait:
//!
//! - the rotator's fresh capacity (`end reason=rotator capacity`);
//! - a change of the login fingerprint (`end reason=login changed`);
//! - a change of the named login key (`end reason=login changed`);
//! - the login having already changed while the attempt was failing, which
//!   retries at once without sleeping at all.
//!
//! Each "does not wake" test carries a positive control on an identical
//! fixture whose only difference is `live_room: true`, so a test cannot pass
//! because the rotator was never read. A login-change wait (the organisation
//! spend-limit wait of #2731) is *not* gated: it waits for another
//! organisation's credit, which no capacity reading reports, and is covered by
//! its own cases here. A 429 that wait may then meet is part 1's concern.
//!
//! Issue #2911 changed two things these tests read. A wait with no reset to
//! end on, which is every capacity and every login-change wait, writes a
//! `waiting_for_capacity reason=<why>` line immediately before its `start`,
//! so a login-change wait's sequence opens with two lines rather than one.
//! And the endings that used to finish the step on a limit now retry it:
//! nothing in this file ends a run, and `failed reason=spend_limit` no longer
//! exists. A wait that does not wake simply sleeps to its own deadline and
//! the step retries there.
//!
//! The rotator is a stub ledger. Cases that act while the attempt is failing
//! run on the fixture's virtual clock and sleep nothing. Cases that must act
//! during a wait use the real clock, sped up, and act on a line the run
//! writes (issue #1973), never on a delay. Nothing reads or writes the real
//! `~/.claude`, `~/.amplihack` or a real rotator.

#![cfg(unix)]

use crate::issue_1618_fixture::{
    Fixture, SPEND_TEXT, WAIT_PREFIX, claude_stdout, describe, events, executable, private_file,
};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::{
    OLD_CYCLE, real_clock, real_now, rotator_running, x_is_live,
};
use crate::support::wait::{CEILING, wait_until};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc::{RecvTimeoutError, channel};

/// The reset wait the stock `LIMIT_TEXT` asks for, on the virtual clock.
const RESET_START: &str = "start reason=usage_limit harness=claude \
                           reset=2026-10-18 08:30:00 -07:00 \
                           until=2026-10-18 08:32:00 -07:00 bound=reset+margin";
const SPEND_START: &str = "start reason=spend_limit wait=login_change harness=claude reset=none \
                           until=2026-10-18 14:00:00 -07:00 bound=max_wait";
/// A login-change wait has no reset to end on, so since issue #2911 it says
/// so on a `waiting_for_capacity` line immediately before its `start`. Its
/// reason is `spend_limit`, the limit that is the only thing to start one.
const SPEND_WAITING: &str = "waiting_for_capacity reason=spend_limit harness=claude \
                             until=2026-10-18 14:00:00 -07:00";

/// The stub `claude`: limited for its first `$LIMIT_CALLS` calls with
/// `$LIMIT_MESSAGE`, and, on the calls named in `$LEDGER_ON_CALL`, a
/// space-separated list of `<call>:<login>:<outcome>:<at>:<room>`, it first
/// replaces the rotate ledger the way the rotator does (one private rename).
/// `<room>` is `true`, `false`, or `none` for a ledger with no `live_room`
/// at all, as a rotator from before #2857 writes it.
const STUB: &str = r#"#!/bin/sh
case "$1" in
  --version) echo '2.1.319 (Claude Code)'; exit 0 ;;
esac
step="${STEP:-0}"
n=$(( $(cat "$RECORD_DIR/count.$step" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$RECORD_DIR/count.$step"
{
  printf 'PWD=%s\n' "$(pwd -P)"
  printf 'HOME=%s\n' "$HOME"
  printf 'CLAUDE_CONFIG_DIR=%s\n' "${CLAUDE_CONFIG_DIR-<unset>}"
  printf 'PROFILE=%s\n' "${AMPLIHACK_CLAUDE_PROFILE-<unset>}"
} > "$RECORD_DIR/call.$step.$n"
echo "attempt $n output"
for spec in $LEDGER_ON_CALL; do
  call="${spec%%:*}"; r="${spec#*:}"
  login="${r%%:*}"; r="${r#*:}"
  outcome="${r%%:*}"; r="${r#*:}"
  at="${r%%:*}"; room="${r#*:}"
  if [ "$call" = "$n" ]; then
    case "$room" in none) extra="" ;; *) extra=",\"live_room\":$room" ;; esac
    umask 077
    printf '{"version":1,"live":"%s","alert":null,"last_cycle":{"at":%s,"measured_at":%s,"outcome":"%s"%s}}' \
      "$login" "$at" "$at" "$outcome" "$extra" > "$LEDGER_DIR/ledger.json.new"
    chmod 600 "$LEDGER_DIR/ledger.json.new"
    mv "$LEDGER_DIR/ledger.json.new" "$LEDGER_DIR/ledger.json"
  fi
done
limit="${LIMIT_CALLS:-0}"
if [ "$n" -le "$limit" ]; then
  printf '%s\n' "$LIMIT_MESSAGE" >&2
  exit "${LIMIT_EXIT:-1}"
fi
exit 0
"#;

fn rotate_dir(f: &Fixture) -> PathBuf {
    f.profiles.join(".auto-switch.d").join("rotate")
}

/// A step on `$HOME/.claude`, X live, with the ledger-writing stub.
fn home_step(f: &Fixture) -> Command {
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    executable(&f.tools.join("claude"), STUB);
    x_is_live(f);
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LEDGER_DIR", rotate_dir(f));
    c
}

/// `<call>:<login>:<outcome>:<at>:<room>` for `LEDGER_ON_CALL`, stamped a few
/// seconds after the virtual clock's start so it is fresh for the wait.
fn on_call(call: u32, login: &str, outcome: &str, room: &str) -> String {
    format!(
        "{call}:{login}:{outcome}:{}:{room}",
        crate::issue_1618_fixture::TEST_NOW + 5
    )
}

/// Replaces the rotate ledger in one rename, private, as the rotator does.
/// `at` is both the time the cycle was recorded and the time it measured.
fn write_ledger(profiles: &Path, live: &str, outcome: &str, at: i64, room: Option<bool>) {
    let dir = profiles.join(".auto-switch.d").join("rotate");
    for d in [profiles.join(".auto-switch.d"), dir.clone()] {
        fs::create_dir_all(&d).unwrap();
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let room = room.map_or(String::new(), |r| format!(r#","live_room":{r}"#));
    let body = format!(
        r#"{{"version":1,"live":"{live}","alert":null,"last_cycle":{{"at":{at},"measured_at":{at},"outcome":"{outcome}"{room}}}}}"#
    );
    let tmp = dir.join("ledger.json.new");
    private_file(&tmp, &body);
    fs::rename(&tmp, dir.join("ledger.json")).unwrap();
}

fn assert_ends(out: &Output, f: &Fixture, last_two: [&str; 2]) {
    let ev = events(out);
    assert_eq!(ev[ev.len() - 2..], last_two, "{ev:?}\n{}", describe(out));
    assert_eq!(f.count("0"), 2, "one wait, one retry: {}", describe(out));
}

// --- the login already moved while the attempt was failing --------------------

/// The case on a shared-login host: the swap lands during the failing
/// attempt, and the old code then retried at once, without sleeping, onto
/// whatever login the rotator had put in place.
#[test]
fn a_swap_onto_a_full_login_while_the_attempt_fails_does_not_skip_the_wait() {
    let f = with_profiles();
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("LEDGER_ON_CALL", on_call(1, "Y", "swapped", "false"));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        [RESET_START, "end reason=reset reached", "resume attempt=2"],
        "the swap put the step on a login at its limit: it waits for its reset: {}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 2);
    assert_eq!(claude_stdout(&out), "attempt 2 output\n");
}

#[test]
fn a_swap_with_no_live_room_recorded_does_not_skip_the_wait_either() {
    // A cycle from a rotator older than #2857 says nothing about room. Absent
    // is no room: the safe direction costs the wait its early wake and no more.
    let f = with_profiles();
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("LEDGER_ON_CALL", on_call(1, "Y", "swapped", "none"));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ends(&out, &f, ["end reason=reset reached", "resume attempt=2"]);
}

/// The positive control for the two cases above: the same swap onto a login
/// that has room still moves the step on at once, as #2731 requires.
#[test]
fn a_swap_onto_a_login_with_room_while_the_attempt_fails_still_retries_at_once() {
    let f = with_profiles();
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("LEDGER_ON_CALL", on_call(1, "Y", "swapped", "true"));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        [RESET_START, "end reason=login changed", "resume attempt=2"],
        "{}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 2);
}

/// A person who runs `profile switch` while the step's attempt is failing has
/// chosen the login. The record is `switched` and carries no room bit, because
/// the rotator measured nothing, and it still moves the step on at once: the
/// escape hatch #1691 gave someone unsticking a waiting run. The rotator's own
/// `swapped` with no bit does not, as
/// `a_swap_with_no_live_room_recorded_does_not_skip_the_wait_either` shows.
#[test]
fn a_manual_switch_while_the_attempt_fails_still_retries_at_once() {
    let f = with_profiles();
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("LEDGER_ON_CALL", on_call(1, "Y", "switched", "none"));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(
        events(&out),
        [RESET_START, "end reason=login changed", "resume attempt=2"],
        "{}",
        describe(&out)
    );
    assert_eq!(f.count("0"), 2);
}

/// The carve-out is for the absence of a reading. A switch whose record says
/// there is no room does not move the step; it waits for its reset.
#[test]
fn a_manual_switch_that_recorded_no_room_while_the_attempt_fails_does_not_skip_the_wait() {
    let f = with_profiles();
    let mut c = home_step(&f);
    c.env("LIMIT_CALLS", "1")
        .env("LEDGER_ON_CALL", on_call(1, "Y", "switched", "false"));
    let out = f.run(&mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ends(&out, &f, ["end reason=reset reached", "resume attempt=2"]);
}

/// A record that is not a capacity cycle moves no step, whatever the ledger
/// holds while the attempt is failing. `rotate stop` writes none of these: it
/// sends a signal and records no cycle, and a cycle abandoned on that signal
/// returns before it saves. `idle` is written when auto-switch is paused, and
/// `stopped` is never written at all; it stands here for an outcome a reader
/// does not know. `alerted`, `held` and `skipped` are the other outcomes that
/// are not capacity.
#[test]
fn a_rotator_that_stops_while_the_attempt_fails_does_not_skip_the_wait() {
    for outcome in ["idle", "stopped", "alerted", "held", "skipped"] {
        let f = with_profiles();
        let mut c = home_step(&f);
        c.env("LIMIT_CALLS", "1")
            .env("LEDGER_ON_CALL", on_call(1, "Y", outcome, "none"));
        let out = f.run(&mut c);
        assert!(out.status.success(), "{outcome}: {}", describe(&out));
        assert_ends(&out, &f, ["end reason=reset reached", "resume attempt=2"]);
    }
}

/// The room bit is on the cycle record, not the login. A record that is not a
/// cycle with capacity never satisfies it, even if something wrote `true`
/// into it: `alerted` and a failed-probe `stayed-unread` are the two the
/// issue's reader must reject.
#[test]
fn a_true_live_room_on_a_record_that_is_not_capacity_moves_no_step() {
    for outcome in ["alerted", "stayed-unread", "idle", "stopped"] {
        let f = with_profiles();
        let mut c = home_step(&f);
        c.env("LIMIT_CALLS", "1")
            .env("LEDGER_ON_CALL", on_call(1, "Y", outcome, "true"));
        let out = f.run(&mut c);
        assert!(out.status.success(), "{outcome}: {}", describe(&out));
        let ev = events(&out);
        assert!(
            !ev.iter().any(|l| l == "end reason=login changed"),
            "{outcome}: a record that is not capacity opened the gate: {ev:?}"
        );
    }
}

// --- the organisation spend-limit wait is not gated (#2731) --------------------

/// `Plan::capacity_may_end()` is false for this wait: it waits for another
/// organisation's credit, which no capacity reading reports, so a login change
/// to a login that reads full is still the thing it waits for. The room gate
/// does not hold that change back, because no reading speaks to organisation
/// credit; gating it would bring #2731 back. Part 1 is what makes this safe:
/// if the login it wakes on is full, the 429 that follows re-enters a wait
/// instead of ending the run.
///
/// Since issue #2911 the wait announces itself with a `waiting_for_capacity
/// reason=spend_limit` line before its `start`, and the login change retries
/// the step rather than being one of the endings that could finish it.
#[test]
fn a_spend_limit_wait_still_ends_on_a_login_change_to_a_login_without_room() {
    for room in ["false", "none"] {
        let f = with_profiles();
        let _rotator = rotator_running(&f);
        let mut c = home_step(&f);
        c.env("LIMIT_MESSAGE", SPEND_TEXT)
            .env("LIMIT_CALLS", "1")
            .env("LEDGER_ON_CALL", on_call(1, "Y", "swapped", room));
        let out = f.run(&mut c);
        assert!(out.status.success(), "room={room}: {}", describe(&out));
        assert_eq!(
            events(&out),
            [
                SPEND_WAITING,
                SPEND_START,
                "end reason=login changed",
                "resume attempt=2"
            ],
            "room={room}: {}",
            describe(&out)
        );
    }
}

// --- the wait is running: the rotator's state changes under it -------------------

/// A reset 15 minutes away on the real clock, so the wait's own end is the
/// reset and nothing in these tests ends it by timing. At the scale used
/// here that is about ten seconds of real time if nothing wakes the wait.
fn long_wait_step(f: &Fixture) -> Command {
    let mut c = home_step(f);
    let reset = real_now() + 900;
    c.env(
        "LIMIT_MESSAGE",
        format!("Claude AI usage limit reached|{reset}"),
    )
    .env("LIMIT_CALLS", "1");
    real_clock(&mut c);
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "100");
    c
}

/// Runs `c`, firing `stages` in order, stage `k` on the `2 + 2k`-th `progress`
/// line the wait writes. The first two progress lines come after the login
/// fingerprint's baseline has settled (a change before then is absorbed into
/// it), which is why the first stage waits for them; each later stage waits
/// two more, a virtual two minutes. Each stage then waits for the next whole
/// second so what it stamps is after the wait's start. A stage the run ends
/// before reaching is not fired, and the test's own assertions say why.
fn run_in_stages(f: &Fixture, c: &mut Command, stages: Vec<Box<dyn FnOnce() + Send>>) -> Output {
    let total = stages.len();
    let (tx, rx) = channel::<i64>();
    let writer = std::thread::spawn(move || {
        for (k, stage) in stages.into_iter().enumerate() {
            match rx.recv_timeout(CEILING) {
                Ok(at) => {
                    wait_until("the second after the progress line", || real_now() > at);
                    stage();
                }
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    panic!("stage {k} was never reached within {CEILING:?}")
                }
            }
        }
    });
    let progress = format!("{WAIT_PREFIX}progress ");
    let (mut seen, mut sent) = (0usize, 0usize);
    let mut tx = Some(tx);
    let out = f.run_watching(c, |line| {
        if !line.starts_with(&progress) {
            return;
        }
        seen += 1;
        if sent < total && seen >= 2 + 2 * sent {
            if let Some(tx) = &tx {
                tx.send(real_now()).expect("the writer is waiting");
            }
            sent += 1;
            if sent == total {
                tx = None;
            }
        }
    });
    drop(tx);
    if let Err(panic) = writer.join() {
        std::panic::resume_unwind(panic);
    }
    out
}

fn progress_lines(out: &Output) -> usize {
    crate::issue_1618_fixture::wait_lines(out)
        .iter()
        .filter(|l| l.starts_with("progress "))
        .count()
}

type Stage = Box<dyn FnOnce() + Send>;

/// A swap during the wait onto a login at 98-100%: what the rotator did over
/// four minutes on 19 Nov 2026, and what woke 26 runs.
#[test]
fn a_swap_onto_a_full_login_during_the_wait_does_not_end_it() {
    let f = with_profiles();
    let mut c = long_wait_step(&f);
    let profiles = f.profiles.clone();
    let swap: Stage = Box::new(move || {
        write_ledger(&profiles, "Y", "swapped", real_now(), Some(false));
    });
    let out = run_in_stages(&f, &mut c, vec![swap]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ends(&out, &f, ["end reason=reset reached", "resume attempt=2"]);
    let ev = events(&out);
    assert!(
        !ev.iter()
            .any(|l| l.contains("rotator capacity") || l.contains("login changed")),
        "a full login woke the wait: {ev:?}"
    );
}

/// The positive control for the case above, on the same fixture: the same swap
/// onto a login with room ends the wait on the rotator's capacity, as before.
#[test]
fn a_swap_onto_a_login_with_room_during_the_wait_still_ends_it() {
    let f = with_profiles();
    let mut c = long_wait_step(&f);
    let profiles = f.profiles.clone();
    let swap: Stage = Box::new(move || {
        write_ledger(&profiles, "Y", "swapped", real_now(), Some(true));
    });
    let out = run_in_stages(&f, &mut c, vec![swap]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ends(
        &out,
        &f,
        ["end reason=rotator capacity", "resume attempt=2"],
    );
}

/// The 20:37:54Z stop. `rotate stop` writes nothing to the ledger, so the
/// ledger a stop leaves is whatever the last cycle recorded, and a wait wakes
/// only when that cycle has capacity and says room. Here the ledger names
/// another login and holds a fresh record that does not: the idle record that
/// a paused rotator writes, a final cycle on a login that reads full, and a
/// final cycle from a rotator that writes no room bit at all.
#[test]
fn stopping_the_rotator_does_not_end_a_wait() {
    let cases: [(&str, &str, Option<bool>); 3] = [
        ("idle", "idle", None),
        (
            "a final cycle on a login that reads full",
            "stayed",
            Some(false),
        ),
        ("a final cycle with no room bit", "stayed", None),
    ];
    for (what, outcome, room) in cases {
        let f = with_profiles();
        let mut c = long_wait_step(&f);
        let profiles = f.profiles.clone();
        let stop: Stage = Box::new(move || {
            write_ledger(&profiles, "Y", outcome, real_now(), room);
        });
        let out = run_in_stages(&f, &mut c, vec![stop]);
        assert!(out.status.success(), "{what}: {}", describe(&out));
        assert_ends(&out, &f, ["end reason=reset reached", "resume attempt=2"]);
        let ev = events(&out);
        assert!(
            !ev.iter()
                .any(|l| l.contains("rotator capacity") || l.contains("login changed")),
            "{what}: the stop woke the wait: {ev:?}"
        );
    }
}

/// The login key and the fingerprint alone, with no fresh capacity in play:
/// the ledger names another login with an old cycle, as a swap the rotator
/// recorded long ago would. With room recorded on it the login change ends the
/// wait; without, the wait does not move.
#[test]
fn a_login_change_alone_ends_the_wait_only_when_the_new_login_has_room() {
    for (room, wakes) in [(Some(true), true), (Some(false), false), (None, false)] {
        let f = with_profiles();
        let mut c = long_wait_step(&f);
        let profiles = f.profiles.clone();
        let swap: Stage = Box::new(move || {
            write_ledger(&profiles, "Y", "stayed", OLD_CYCLE, room);
        });
        let out = run_in_stages(&f, &mut c, vec![swap]);
        assert!(out.status.success(), "room={room:?}: {}", describe(&out));
        let want = if wakes {
            ["end reason=login changed", "resume attempt=2"]
        } else {
            ["end reason=reset reached", "resume attempt=2"]
        };
        assert_ends(&out, &f, want);
    }
}

/// A person's `profile switch` during a wait, with no room bit in its record.
/// It ends the wait, as it did before this rule: fresh, it is the rotator's
/// capacity; with an old stamp, only the login changed. Both are the exits the
/// rotator's own `swapped` with no bit is held back from
/// (`a_login_change_alone_ends_the_wait_only_when_the_new_login_has_room`).
#[test]
fn a_manual_switch_still_ends_a_wait_without_a_room_reading() {
    for (fresh, want) in [
        (true, "end reason=rotator capacity"),
        (false, "end reason=login changed"),
    ] {
        let f = with_profiles();
        let mut c = long_wait_step(&f);
        let profiles = f.profiles.clone();
        let switch: Stage = Box::new(move || {
            let at = if fresh { real_now() } else { OLD_CYCLE };
            write_ledger(&profiles, "Y", "switched", at, None);
        });
        let out = run_in_stages(&f, &mut c, vec![switch]);
        assert!(out.status.success(), "fresh={fresh}: {}", describe(&out));
        assert_ends(&out, &f, [want, "resume attempt=2"]);
    }
}

/// The same switch whose record positively says there is no room: the wait
/// does not move, whether the stamp is fresh or old.
#[test]
fn a_manual_switch_that_recorded_no_room_does_not_end_a_wait() {
    for fresh in [true, false] {
        let f = with_profiles();
        let mut c = long_wait_step(&f);
        let profiles = f.profiles.clone();
        let switch: Stage = Box::new(move || {
            let at = if fresh { real_now() } else { OLD_CYCLE };
            write_ledger(&profiles, "Y", "switched", at, Some(false));
        });
        let out = run_in_stages(&f, &mut c, vec![switch]);
        assert!(out.status.success(), "fresh={fresh}: {}", describe(&out));
        assert_ends(&out, &f, ["end reason=reset reached", "resume attempt=2"]);
        let ev = events(&out);
        assert!(
            !ev.iter()
                .any(|l| l.contains("rotator capacity") || l.contains("login changed")),
            "fresh={fresh}: a switch that said no room woke the wait: {ev:?}"
        );
    }
}

/// A login full when it arrived and with room later: the wait that did not
/// wake for the first record wakes for the second. A gate that closed for good
/// on the first change would pass every "does not wake" case above and fail
/// this one, so it is the case that says the rule is *room*, not *quiet*.
///
/// The reason is `login changed`, not `rotator capacity`. Both ledger records
/// name the same login, so the fingerprint moved once, at the first record, and
/// the room gate held that change back. The second record opens the gate, and
/// the held-back change is read before the capacity check. Either way the wait
/// ends at the first poll after room is recorded and not before.
#[test]
fn a_wait_that_did_not_wake_for_a_full_login_wakes_when_room_appears() {
    let f = with_profiles();
    let mut c = long_wait_step(&f);
    let (p1, p2) = (f.profiles.clone(), f.profiles.clone());
    let full: Stage = Box::new(move || {
        write_ledger(&p1, "Y", "swapped", real_now(), Some(false));
    });
    let room: Stage = Box::new(move || {
        write_ledger(&p2, "Y", "stayed", real_now(), Some(true));
    });
    let out = run_in_stages(&f, &mut c, vec![full, room]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ends(&out, &f, ["end reason=login changed", "resume attempt=2"]);
    assert!(
        progress_lines(&out) >= 4,
        "the wait ran past the full login's record before the room record woke it: {}",
        describe(&out)
    );
}

/// An organisation spend-limit wait, acted on during the wait: still ended by
/// a login change to a login that reads full (#2731, see the case above for
/// the one that acts while the attempt fails).
#[test]
fn a_spend_limit_wait_ends_on_a_login_change_during_the_wait_whatever_the_room() {
    let f = with_profiles();
    let _rotator = rotator_running(&f);
    let mut c = home_step(&f);
    c.env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("LIMIT_CALLS", "1")
        .env("AMPLIHACK_USAGE_LIMIT_MAX_WAIT_SECS", "3000");
    real_clock(&mut c);
    c.env("AMPLIHACK_USAGE_LIMIT_WAIT_TEST_SCALE", "100");
    let profiles = f.profiles.clone();
    let swap: Stage = Box::new(move || {
        write_ledger(&profiles, "Y", "swapped", OLD_CYCLE, Some(false));
    });
    let out = run_in_stages(&f, &mut c, vec![swap]);
    assert!(out.status.success(), "{}", describe(&out));
    assert_ends(&out, &f, ["end reason=login changed", "resume attempt=2"]);
}
