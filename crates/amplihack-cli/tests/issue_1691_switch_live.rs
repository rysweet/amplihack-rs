//! Issue #1691: `amplihack profile switch NAME` moves NAME's login into
//! `~/.claude` with the rotator's swap, pins rotation to it until
//! `switch --none`, and writes the default pointer. End to end against a
//! temporary HOME and profile root (`isolated(..)` through the rotate
//! fixture) and a stub `claude`; no real login, root or rotator is touched,
//! and no daemon is started. Logins are checked by inode only. Linux-only:
//! the swap needs `renameat2(RENAME_EXCHANGE)`.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;

const RELEASE_HINT: &str = "`amplihack profile switch --none`";
const RELEASED: &str = "Rotation released; the rotator picks the best login on its next cycle.";

fn stdout_lines(out: &std::process::Output) -> Vec<String> {
    text(&out.stdout).lines().map(str::to_string).collect()
}

fn pin_path(f: &Fixture) -> PathBuf {
    f.rotate_dir().join("pin")
}

fn read_opt(p: &std::path::Path) -> Option<Vec<u8>> {
    fs::read(p).ok()
}

/// Every log line is in the documented grammar and prints no secret.
fn assert_log_clean(f: &Fixture) {
    let log = f.log();
    for l in log.lines() {
        assert_log_grammar(l, true);
    }
    assert_text_has_no_secrets(&log);
}

/// The files a refused `switch` must leave alone.
fn state(f: &Fixture) -> Vec<(&'static str, Option<Vec<u8>>)> {
    vec![
        ("ledger", read_opt(&f.rotate_dir().join("ledger.json"))),
        ("journal", read_opt(&f.rotate_dir().join("journal.json"))),
        ("pin", read_opt(&pin_path(f))),
        (".default", read_opt(&f.root.join(".default"))),
        ("~/.claude.json", read_opt(&f.home.join(".claude.json"))),
    ]
}

#[track_caller]
fn assert_refused_cleanly(f: &Fixture, args: &[&str], want: &str) {
    let before = (state(f), f.logins());
    let out = f.run(args);
    assert_eq!(
        out.status.code(),
        Some(1),
        "`{}` must refuse\nstdout: {}\nstderr: {}",
        args.join(" "),
        text(&out.stdout),
        text(&out.stderr)
    );
    let err = text(&out.stderr);
    assert!(err.contains(want), "stderr lacks {want:?}: {err}");
    assert_eq!(
        (state(f), f.logins()),
        before,
        "a refusal changed something"
    );
    assert!(f.probe_envs().is_empty(), "switch never runs claude");
    assert_log_clean(f);
}

/// The live login cannot be moved: exit 1 with `want` on stderr, but
/// `target` is still made the default for new launches (issue #1691, "keeps
/// the `.default` pointer behaviour"). Nothing else changes, `held` names
/// the profile an earlier switch still holds rotation on.
#[track_caller]
fn assert_refused_default_only(f: &Fixture, target: &str, held: Option<&str>, want: &str) {
    let mut before = (state(f), f.logins());
    let out = f.run(&["profile", "switch", target]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "`switch {target}` must refuse\nstdout: {}\nstderr: {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let err = text(&out.stderr);
    assert!(err.contains(want), "stderr lacks {want:?}: {err}");
    let second = match held {
        Some(h) => {
            format!("Live login not moved; rotation still held on {h} until {RELEASE_HINT}.")
        }
        None => "Live login not moved; rotation not held.".to_string(),
    };
    assert_eq!(
        text(&out.stdout),
        format!("Default for new launches: {target}\n{second}\n")
    );
    for (label, v) in &mut before.0 {
        if *label == ".default" {
            *v = Some(format!("{target}\n").into_bytes());
        }
    }
    assert_eq!(
        (state(f), f.logins()),
        before,
        "a refused live move changed something other than .default"
    );
    assert!(f.probe_envs().is_empty(), "switch never runs claude");
    assert_log_clean(f);
}

// --- AC1/AC2: the live login moves -----------------------------------------

#[test]
fn switch_moves_the_live_login_and_says_so_in_three_lines() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "switch", "spare"]);
    assert_eq!(
        text(&out.stdout),
        "Live login moved: work -> spare\n\
         Default for new launches: spare\n\
         Rotation held on spare until `amplihack profile switch --none`.\n"
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    assert_eq!(ino(&f.slot("spare")), None);
    f.assert_each_login_once();
    assert_eq!(f.home_account(), "uuid-spare");

    let ledger = f
        .ledger()
        .expect("the swap writes the rotator's ledger entry");
    assert_eq!(ledger["live"], "spare");
    assert_eq!(ledger["previous"], "work");
    assert!(
        ledger.get("pin").is_none(),
        "the pin is never in the ledger"
    );
    assert!(!f.rotate_dir().join("journal.json").exists());

    assert_eq!(fs::read_to_string(pin_path(&f)).unwrap(), "spare\n");
    assert_eq!(
        fs::metadata(pin_path(&f)).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::read_to_string(f.root.join(".default")).unwrap(),
        "spare\n"
    );
    assert!(f.probe_envs().is_empty(), "switch measures nothing");
    assert!(
        f.log().lines().any(|l| l.ends_with(" switch work->spare")),
        "{}",
        f.log()
    );
    assert_log_clean(&f);
}

#[test]
fn switch_to_the_live_profile_moves_nothing_and_still_pins() {
    let f = Fixture::new();
    let before = f.logins();
    let out = f.ok(&["profile", "switch", "work"]);
    assert_eq!(
        text(&out.stdout),
        "work is already the live login; nothing moved.\n\
         Default for new launches: work\n\
         Rotation held on work until `amplihack profile switch --none`.\n"
    );
    assert_eq!(f.logins(), before);
    assert_eq!(fs::read_to_string(pin_path(&f)).unwrap(), "work\n");
    assert_eq!(
        fs::read_to_string(f.root.join(".default")).unwrap(),
        "work\n"
    );
    assert!(
        f.log()
            .contains("skip: work is already the live login; nothing moved")
    );
    assert_log_clean(&f);
}

// --- AC3: the rotator does not swap away while pinned -----------------------

#[test]
fn rotate_while_pinned_holds_and_measures_nothing() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "spare"]);
    // spare is now over the session threshold: a free rotator would swap.
    f.usage("spare", 95, 40);
    let before = f.logins();
    let out = f.ok(&["profile", "rotate"]);
    let lines = stdout_lines(&out);
    for l in &lines {
        assert_log_grammar(l, true);
    }
    let held = format!("skip: held by switch to spare; run {RELEASE_HINT} to release");
    assert!(lines.iter().any(|l| l.ends_with(&held)), "{lines:?}");
    assert!(
        !lines
            .iter()
            .any(|l| l.contains(" swap ") || l.contains(" cycle ")),
        "{lines:?}"
    );
    assert_eq!(f.logins(), before, "nothing moved");
    assert!(f.probe_envs().is_empty(), "no usage probe while pinned");
    assert_eq!(
        f.ledger().unwrap()["last_cycle"]["outcome"],
        "switched",
        "a held cycle keeps the switch as the last cycle"
    );

    let status = f.ok(&["profile", "rotate", "status"]);
    let s = text(&status.stdout);
    assert!(s.lines().any(|l| l == "hold: spare"), "{s}");
    assert!(s.contains(" switched"), "{s}");
    assert_log_clean(&f);
}

#[test]
fn a_dry_run_while_pinned_says_it_would_stay() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "spare"]);
    f.usage("spare", 95, 40);
    let ledger_before = read_opt(&f.rotate_dir().join("ledger.json"));
    let out = f.ok(&["profile", "rotate", "--dry-run"]);
    let lines = stdout_lines(&out);
    for l in &lines {
        assert_log_grammar(l, false);
    }
    assert!(
        lines
            .iter()
            .any(|l| l == "would stay (held by switch to spare)"),
        "{lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.starts_with("would swap")),
        "{lines:?}"
    );
    assert!(f.probe_envs().is_empty(), "a held dry run measures nothing");
    assert_eq!(read_opt(&f.rotate_dir().join("ledger.json")), ledger_before);
}

#[test]
fn an_unreadable_pin_holds_rotation_and_names_no_bytes() {
    let f = Fixture::new();
    fs::create_dir_all(f.rotate_dir()).unwrap();
    for d in [f.root.join(".auto-switch.d"), f.rotate_dir()] {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(pin_path(&f), b"\x1b[31mSENTINEL-TOKEN\n").unwrap();
    fs::set_permissions(pin_path(&f), fs::Permissions::from_mode(0o600)).unwrap();
    let before = f.logins();
    let out = f.ok(&["profile", "rotate"]);
    let s = text(&out.stdout);
    assert!(
        s.contains(&format!("skip: pin unreadable; run {RELEASE_HINT}")),
        "{s}"
    );
    assert_eq!(f.logins(), before, "fails closed: nothing moves");
    assert!(f.probe_envs().is_empty());
    assert_eq!(f.ledger().unwrap()["last_cycle"]["outcome"], "held");

    let dry = f.ok(&["profile", "rotate", "--dry-run"]);
    assert!(
        stdout_lines(&dry)
            .iter()
            .any(|l| l == "would stay (pin unreadable)"),
        "{}",
        text(&dry.stdout)
    );
    assert_log_clean(&f);
}

#[test]
fn a_pin_naming_a_removed_profile_is_ignored() {
    let f = Fixture::new();
    f.ok(&["profile", "add", "gone"]);
    f.ok(&["profile", "switch", "work"]);
    // Point the pin at a profile, then remove that profile by hand.
    fs::write(pin_path(&f), "gone\n").unwrap();
    fs::remove_dir_all(f.root.join("gone")).unwrap();
    let out = f.ok(&["profile", "rotate"]);
    let s = text(&out.stdout);
    assert!(
        s.contains("skip: pin names missing profile gone; ignored"),
        "{s}"
    );
    assert!(
        s.contains("swap work->spare"),
        "rotation runs normally: {s}"
    );
    assert!(pin_path(&f).exists(), "left for `switch --none` to remove");
    assert_log_clean(&f);
}

// --- AC4: `switch --none` hands control back --------------------------------

#[test]
fn none_releases_the_pin_and_rotation_measures_again() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "spare"]);
    let out = f.ok(&["profile", "switch", "--none"]);
    assert_eq!(
        text(&out.stdout),
        format!("Default profile cleared.\n{RELEASED}\n")
    );
    assert!(!pin_path(&f).exists());
    assert!(!f.root.join(".default").exists());
    assert_eq!(
        ino(&f.home_login()),
        Some(f.spare_ino),
        "--none moves no login"
    );

    let again = f.ok(&["profile", "switch", "--none"]);
    assert_eq!(text(&again.stdout), "No default profile was set.\n");

    f.usage("spare", 95, 40);
    let rot = f.ok(&["profile", "rotate"]);
    let s = text(&rot.stdout);
    assert!(s.contains("swap spare->extra"), "{s}");
    assert!(!f.probe_envs().is_empty(), "the released rotator measures");
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    f.assert_each_login_once();
    assert_log_clean(&f);
}

#[test]
fn none_under_a_busy_rotate_lock_fails_and_changes_nothing() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "spare"]);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root.join(".auto-switch.d").join("lock"))
        .expect("switch created the state lock");
    // SAFETY: flock on a valid, open descriptor owned by `lock`.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let before = state(&f);
    let out = f.run(&["profile", "switch", "--none"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        text(&out.stderr).contains("lock busy"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(state(&f), before, "neither .default nor the pin changed");
    drop(lock);
}

// --- AC6: refusals ----------------------------------------------------------

#[test]
fn switch_to_an_unknown_profile_changes_nothing() {
    let f = Fixture::new();
    let before = (state(&f), f.logins());
    let out = f.run(&["profile", "switch", "nosuch"]);
    assert!(!out.status.success());
    assert_eq!((state(&f), f.logins()), before);
    assert!(
        !f.rotate_dir().join("rotate.log").exists() || f.log().is_empty(),
        "refused before any lock or log: {}",
        f.log()
    );
}

#[test]
fn switch_to_a_profile_without_a_stored_login_is_refused() {
    // Issue #1700 (D9): a subscription profile with no login of its own is
    // refused by the kind guard before the switch runs, so not even
    // `.default` changes: switching to it would bill the API key.
    let f = Fixture::new();
    f.ok(&["profile", "add", "fresh"]);
    let before = (state(&f), f.logins());
    let out = f.run(&["profile", "switch", "fresh"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(
        err.contains(
            "profile fresh is a subscription profile with no login of its own; \
             launching it would bill the Claude Platform API key per token."
        ),
        "{err}"
    );
    assert!(
        err.contains(
            "hint: amplihack profile login <name>, or: amplihack profile kind set <name> api-key"
        ),
        "{err}"
    );
    assert_eq!(text(&out.stdout), "", "nothing switched");
    assert_eq!((state(&f), f.logins()), before, "nothing changed");
}

#[test]
fn switch_with_no_login_in_home_is_refused() {
    let f = Fixture::new();
    fs::remove_file(f.home_login()).unwrap();
    assert_refused_default_only(&f, "spare", None, "no login in ~/.claude");
}

#[test]
fn a_refused_swap_is_passed_through() {
    let f = Fixture::new();
    // Claude holds its refresh lock in ~/.claude: the swap refuses.
    fs::create_dir(f.home.join(".claude").join(".oauth_refresh.lock")).unwrap();
    assert_refused_default_only(&f, "spare", None, "refresh lock");
}

#[test]
fn a_profile_open_in_a_session_still_becomes_the_default() {
    // The usual way a profile is used: a session runs with CLAUDE_CONFIG_DIR
    // set to it. Its login cannot be moved, but new launches can default to
    // it, as before #1691. An earlier hold on work stays and is named.
    let f = Fixture::new();
    f.ok(&["profile", "switch", "work"]);
    let mut session = std::process::Command::new("sleep")
        .arg("600")
        .env("CLAUDE_CONFIG_DIR", f.root.join("spare"))
        .spawn()
        .expect("spawn sleep");
    assert_refused_default_only(&f, "spare", Some("work"), "(session running)");
    let _ = session.kill();
    let _ = session.wait();
    assert_eq!(fs::read_to_string(pin_path(&f)).unwrap(), "work\n");
}

// --- D5: a failure after the swap is reported truthfully --------------------

#[test]
fn a_failed_pin_write_after_the_swap_says_the_login_moved() {
    let f = Fixture::new();
    fs::create_dir_all(pin_path(&f)).unwrap();
    for d in [f.root.join(".auto-switch.d"), f.rotate_dir()] {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::write(pin_path(&f).join("blocker"), "x").unwrap();
    let out = f.run(&["profile", "switch", "spare"]);
    assert_eq!(out.status.code(), Some(1));
    let err = text(&out.stderr);
    assert!(
        err.contains("the live login moved to spare, but writing the rotation hold failed"),
        "{err}"
    );
    assert!(
        err.contains("run `amplihack profile switch spare` again"),
        "{err}"
    );
    assert!(
        !err.contains("nothing was moved"),
        "the login did move: {err}"
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    f.assert_each_login_once();

    fs::remove_file(pin_path(&f).join("blocker")).unwrap();
    fs::remove_dir(pin_path(&f)).unwrap();
    let rerun = f.ok(&["profile", "switch", "spare"]);
    assert!(
        text(&rerun.stdout).starts_with("spare is already the live login; nothing moved.\n"),
        "{}",
        text(&rerun.stdout)
    );
    assert_eq!(fs::read_to_string(pin_path(&f)).unwrap(), "spare\n");
    assert_eq!(
        fs::read_to_string(f.root.join(".default")).unwrap(),
        "spare\n"
    );
}

// --- AC7: stopped partway through the swap ----------------------------------

#[cfg(debug_assertions)]
#[test]
fn a_switch_stopped_mid_swap_writes_no_pin_and_is_recovered_by_the_next() {
    let f = Fixture::new();
    let out = f
        .cmd(&["profile", "switch", "spare"])
        .env(ABORT_ENV, "rotate-S1")
        .output()
        .unwrap();
    assert_no_secrets(&out);
    assert_eq!(out.status.code(), Some(86), "{}", text(&out.stderr));
    f.assert_each_login_once();
    assert!(f.rotate_dir().join("journal.json").exists(), "journal left");
    assert!(!pin_path(&f).exists(), "no pin before the swap finished");
    assert!(!f.root.join(".default").exists(), "no pointer either");

    let again = f.ok(&["profile", "switch", "spare"]);
    assert!(
        text(&again.stdout).starts_with("spare is already the live login; nothing moved.\n"),
        "recovery finished the swap: {}",
        text(&again.stdout)
    );
    assert!(
        f.log()
            .contains("recovery: finished an interrupted swap work->spare"),
        "{}",
        f.log()
    );
    assert!(!f.rotate_dir().join("journal.json").exists());
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    f.assert_each_login_once();
    assert_eq!(fs::read_to_string(pin_path(&f)).unwrap(), "spare\n");
    assert_log_clean(&f);
}

// --- Help text --------------------------------------------------------------

#[test]
fn switch_help_describes_the_live_swap_and_the_release() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "switch", "--help"]);
    let s = text(&out.stdout).to_lowercase();
    assert!(s.contains("--none"), "{s}");
    assert!(s.contains("~/.claude"), "help says the login moves: {s}");
    assert!(
        s.contains("rotat"),
        "help says rotation is held/released: {s}"
    );
}

// --- D6: a held state lock refuses `switch NAME` -----------------------------

/// Takes the auto-switch state lock in this test process, as a running
/// rotator cycle would. The lock file is created private, as amplihack does.
fn hold_state_lock(f: &Fixture) -> fs::File {
    let dir = f.root.join(".auto-switch.d");
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("lock"))
        .unwrap();
    fs::set_permissions(dir.join("lock"), fs::Permissions::from_mode(0o600)).unwrap();
    // SAFETY: flock on a valid, open descriptor owned by `lock`.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    lock
}

#[test]
fn switch_under_a_held_state_lock_is_refused_and_changes_nothing() {
    let f = Fixture::new();
    let lock = hold_state_lock(&f);
    // `assert_refused_cleanly` byte-compares the ledger, journal, pin,
    // .default and ~/.claude.json, and compares every login by inode.
    assert_refused_cleanly(&f, &["profile", "switch", "spare"], "lock busy");
    assert_eq!(ino(&f.home_login()), Some(f.live_ino), "work stays live");
    drop(lock);
    // Once the lock is free the same command succeeds: the refusal left
    // nothing behind that blocks it.
    f.ok(&["profile", "switch", "spare"]);
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
}

#[test]
fn switch_under_a_held_lock_after_an_earlier_switch_keeps_the_old_hold() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "spare"]);
    let lock = hold_state_lock(&f);
    assert_refused_cleanly(&f, &["profile", "switch", "extra"], "lock busy");
    assert_eq!(fs::read_to_string(pin_path(&f)).unwrap(), "spare\n");
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    drop(lock);
}

// --- D-Release: `--none` is one locked operation ----------------------------

#[test]
fn none_without_a_pin_still_refuses_under_a_busy_lock() {
    // The pin check, the pin removal and the `.default` removal share the
    // lock a switch holds while it writes both, so they cannot interleave.
    let f = Fixture::new();
    fs::write(f.root.join(".default"), "work\n").unwrap();
    fs::set_permissions(f.root.join(".default"), fs::Permissions::from_mode(0o600)).unwrap();
    let lock = hold_state_lock(&f);
    let out = f.run(&["profile", "switch", "--none"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(!pin_path(&f).exists());
    assert_eq!(
        fs::read_to_string(f.root.join(".default")).unwrap(),
        "work\n",
        "a refused --none leaves the default"
    );
    drop(lock);
    let out = f.ok(&["profile", "switch", "--none"]);
    assert_eq!(text(&out.stdout), "Default profile cleared.\n");
    assert_eq!(
        ino(&f.home_login()),
        Some(f.live_ino),
        "--none moves nothing"
    );
}
#[test]
fn rotate_after_none_is_free_to_swap_again() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "spare"]);
    f.usage("spare", 95, 40);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "held");
    f.ok(&["profile", "switch", "--none"]);
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        text(&out.stdout).contains("swap spare->extra"),
        "{}",
        text(&out.stdout)
    );
    assert_ne!(f.ledger().unwrap()["last_cycle"]["outcome"], "held");
}

// --- The switch needs no `claude` on PATH ------------------------------------

#[test]
fn switch_works_with_no_claude_on_path() {
    let f = Fixture::new();
    let empty = f.tmp.path().join("empty-bin");
    fs::create_dir_all(&empty).unwrap();
    let out = f
        .cmd(&["profile", "switch", "spare"])
        .env("PATH", &empty)
        .env_remove("AMPLIHACK_CLAUDE_BINARY_PATH")
        .output()
        .unwrap();
    assert_no_secrets(&out);
    assert!(
        out.status.success(),
        "switch must not look for claude\nstdout: {}\nstderr: {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(
        text(&out.stdout).starts_with("Live login moved: work -> spare\n"),
        "{}",
        text(&out.stdout)
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    f.assert_each_login_once();
    assert!(f.probe_envs().is_empty());

    // A held one-shot rotate also stops before looking for claude.
    let rot = f
        .cmd(&["profile", "rotate"])
        .env("PATH", &empty)
        .env_remove("AMPLIHACK_CLAUDE_BINARY_PATH")
        .output()
        .unwrap();
    assert!(rot.status.success(), "{}", text(&rot.stderr));
    let last = &f.ledger().unwrap()["last_cycle"];
    assert_eq!(
        last["outcome"], "switched",
        "the held cycle kept it: {last}"
    );
    assert_eq!(
        last["measured_at"], last["at"],
        "the only measurement on record is the switch itself: {last}"
    );
}

// --- D4: no foreign-rotator warning when no rotator runs ---------------------

#[test]
fn a_switch_with_no_rotator_running_prints_no_hold_warning() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "switch", "spare"]);
    let err = text(&out.stderr);
    assert!(!err.contains("ignores the switch hold"), "{err}");
}
