//! Crusty's stale-record case on PR #3009, with a stand-in session and a
//! stand-in rotator.
//!
//! The rotator writes its swap record only when it swaps, so at an exit the
//! record usually predates the run. A record with room from a swap made
//! before the run says nothing about whether the run then went into a usage
//! limit. Without a check, a trial relaunch that goes into a limit would be
//! refused with `no-progress` as if it had crashed while a login had room,
//! and the session would stay stopped until the supervisor's recovery.
//!
//! Before such a refusal the wrapper now asks a running rotator for a
//! measurement taken after the exit. The stand-in rotator is a real process
//! whose arguments include `profile rotate watch`, named by the pid file the
//! real one writes, so the wrapper finds it on the host's `/proc` exactly as
//! it finds a real one. The test writes the ledger and the swap record the
//! way the rotator does, by renaming a new file over the old one.

use super::*;

/// A running process that the rotator's pid check accepts. It blocks
/// reading a pipe the test holds open, and is killed when dropped.
struct Rotator(Child);

impl Rotator {
    fn start(fx: &Fx) -> Self {
        let child = Command::new("/bin/sh")
            .args(["-c", "read line", "rotator", "profile", "rotate", "watch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid_file = rotate_dir(fx).join("rotate.pid");
        fs::write(&pid_file, format!("{}\n", child.id())).unwrap();
        chmod(&pid_file, 0o600);
        Self(child)
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Rotator {
    fn drop(&mut self) {
        self.stop();
    }
}

fn rotate_dir(fx: &Fx) -> PathBuf {
    let rotate = fx.profiles.join(".auto-switch.d").join("rotate");
    fs::create_dir_all(&rotate).unwrap();
    chmod(&fx.profiles.join(".auto-switch.d"), 0o700);
    chmod(&rotate, 0o700);
    rotate
}

/// Writes `name` in the rotate directory as the rotator does: a new private
/// file renamed over the old one.
fn put(fx: &Fx, name: &str, body: &Value) {
    let rotate = rotate_dir(fx);
    let tmp = rotate.join(format!("{name}.new"));
    fs::write(&tmp, body.to_string()).unwrap();
    chmod(&tmp, 0o600);
    fs::rename(&tmp, rotate.join(name)).unwrap();
}

/// The ledger's last cycle: `outcome`, measured at `at`, with the
/// rotator's no-capacity alert set when `alert`.
fn ledger(fx: &Fx, outcome: &str, at: i64, alert: bool) {
    let mut body = json!({
        "version": 1,
        "live": "spare",
        "last_cycle": { "at": at, "measured_at": at, "outcome": outcome }
    });
    if alert {
        body["alert"] = json!({ "since": at });
    }
    put(fx, "ledger.json", &body);
}

/// A swap record made at `at` onto a login at `session` percent.
fn swap_at(fx: &Fx, seq: u64, session: u8, at: i64) {
    let body = json!({
        "seq": seq, "live_login": "spare", "from_login": "main", "swapped_at": at,
        "reading": { "session": session, "week": 10, "measured_at": at }
    });
    put(fx, "swap.json", &body);
}

/// Waits until the wrapper has written a waiting line with `reason`, which
/// it does only once its watch on the rotate directory is armed.
fn waiting_for(fx: &Fx, reason: &str) {
    let want = format!(" standing session waiting session={NAME} reason={reason}");
    assert!(
        wait_until(|| fx.log_lines().iter().any(|l| l.ends_with(&want))),
        "the wrapper never said it was waiting ({reason}): {:?}",
        fx.log_lines()
    );
}

/// The state crusty described: the last swap was hours before the run, its
/// record shows room, and the rotator's last cycle measured before the run.
fn stale_room(fx: &Fx) -> Rotator {
    swap_at(fx, 1, 10, now() - 4 * 3600);
    ledger(fx, "stayed", now() - 60, false);
    Rotator::start(fx)
}

/// Run 1 dies idle and is relaunched at once on the record, which shows
/// room. That relaunch is the trial. Run 2 goes into the usage limit: its
/// request fails and Claude Code writes only its own `<synthetic>` error
/// line, then exits 1. The record still shows room, so before this fix the
/// brake refused here and left the session stopped.
///
/// Now the wrapper waits for the rotator. Its first cycle after the exit
/// finds no capacity, so the wrapper waits for room; the rotator then swaps
/// onto a login with room, and run 3 is started on it.
#[test]
fn a_limit_hit_under_a_swap_record_older_than_the_run_is_waited_for_not_refused() {
    let (fx, dir) = fixture(&["wait:1", "fail:1", "hold:0"]);
    set_standing(&fx, true);
    let _rotator = stale_room(&fx);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    assert!(wait_until(|| exited(&fx, 2)));
    waiting_for(&fx, "room-unconfirmed");
    assert_eq!(runs(&fx).len(), 2, "relaunched before the rotator answered");

    ledger(&fx, "alerted", now() + 1, true);
    waiting_for(&fx, "no-room");
    assert_eq!(runs(&fx).len(), 2, "relaunched into the limit");

    swap_at(&fx, 2, 10, now() + 1);
    run_is_up(&fx, 3);
    release(&fx, 3);
    expect_code(&w.finish(), 0);

    let log = fx.log_lines();
    assert!(
        !log.iter().any(|l| l.contains(" standing session refused ")),
        "a limit is not a crash: {log:?}"
    );
    let lines = relaunch_lines(&log);
    assert_eq!(lines.len(), 2, "{log:?}");
    assert!(
        lines[0].ends_with("reason=exit-status-1 waited=none"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].ends_with("reason=exit-status-1 waited=room"),
        "{}",
        lines[1]
    );
    let r = runs(&fx);
    assert_eq!(r.len(), 3, "{r:?}");
    assert!(r[2].contains("--continue"), "{}", r[2]);
}

/// The brake still stops a start-up crash loop when a rotator runs. The
/// rotator's first cycle after the trial's exit stays on the live login,
/// which has room, so the exit was not a limit and the refusal stands. The
/// alert says the room was read after the exit.
#[test]
fn a_crash_loop_is_refused_once_the_rotator_reads_room_after_the_exit() {
    let (fx, dir) = fixture(&["wait:1", "wait:1", "exit:0"]);
    set_standing(&fx, true);
    let _rotator = stale_room(&fx);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    waiting_for(&fx, "room-unconfirmed");
    ledger(&fx, "stayed", now() + 1, false);
    expect_code(&w.finish(), 1);

    assert_eq!(runs(&fx).len(), 2, "{:?}", runs(&fx));
    let refusal = refusal(&fx, "crash loop with a rotator");
    assert!(refusal.ends_with(" reason=no-progress"), "{refusal}");
    assert_eq!(
        fx.agent(NAME)["alert"].as_str().unwrap_or_default(),
        "not relaunched: the relaunch of an exit that showed nothing itself \
         exited exit-status-1 without a reply from the model in its \
         conversation while the rotator's first reading after the exit found \
         a login with room, so another relaunch would repeat it"
    );
}

/// With no rotator running there is no one to ask. The refusal is made at
/// once, as before, and its alert says it rests on a swap record that no
/// reading after the exit checked, so the supervisor knows a limit was not
/// ruled out. The same holds when the rotator stops before it answers.
#[test]
fn without_a_rotator_to_ask_the_refusal_says_it_rests_on_the_record() {
    for stops in [false, true] {
        let (fx, dir) = fixture(&["wait:1", "wait:1", "exit:0"]);
        set_standing(&fx, true);
        let mut rotator = stale_room(&fx);
        if !stops {
            rotator.stop();
        }
        let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
        run_is_up(&fx, 1);
        release(&fx, 1);
        run_is_up(&fx, 2);
        release(&fx, 2);
        if stops {
            waiting_for(&fx, "room-unconfirmed");
            rotator.stop();
        }
        expect_code(&w.finish(), 1);

        assert_eq!(runs(&fx).len(), 2, "stops={stops}: {:?}", runs(&fx));
        let refusal = refusal(&fx, &format!("stops={stops}"));
        assert!(refusal.ends_with(" reason=no-progress"), "{refusal}");
        assert_eq!(
            fx.agent(NAME)["alert"].as_str().unwrap_or_default(),
            "not relaunched: the relaunch of an exit that showed nothing itself \
             exited exit-status-1 without a reply from the model in its \
             conversation while the last swap record showed a login with room \
             and no rotator reading after the exit could be had, so another \
             relaunch would repeat it",
            "stops={stops}"
        );
    }
}

/// A reading taken after the exit still counts when no rotator is left to
/// ask for another. One that found no capacity says the trial went into a
/// usage limit, so the refusal is `gone`, with the stopped rotator as the
/// cause on the record, and not a `no-progress` refusal that would call
/// the limit a crash.
#[test]
fn a_reading_of_no_capacity_after_the_exit_is_gone_even_with_no_rotator() {
    let (fx, dir) = fixture(&["wait:1", "fail:1", "exit:0"]);
    set_standing(&fx, true);
    swap_at(&fx, 1, 10, now() - 4 * 3600);
    // Dated ahead of the trial's exit, as a cycle that measured after it
    // is; the reader accepts a time up to 300 seconds ahead. No rotator
    // runs: the cycle was its last.
    ledger(&fx, "alerted", now() + 120, true);
    let w = Running::spawn(&fx, wrapped(&fx, &dir, &[], &[]));
    run_is_up(&fx, 1);
    release(&fx, 1);
    run_is_up(&fx, 2);
    release(&fx, 2);
    expect_code(&w.finish(), 1);

    assert_eq!(runs(&fx).len(), 2, "{:?}", runs(&fx));
    let refusal = refusal(&fx, "spent with no rotator");
    assert!(refusal.ends_with(" reason=gone"), "{refusal}");
    let e = fx.agent(NAME);
    assert_eq!(e["state"], "active", "left for the supervisor: {e}");
    assert_eq!(
        e["alert"].as_str().unwrap_or_default(),
        "not relaunched: it exited exit-status-1 while no login had room, and the \
         rotator stopped before one appeared, so the session was left for the \
         supervisor",
        "{e}"
    );
}
