//! Issue #1606: credit-backed overflow profiles, end to end through the
//! binary. Every run uses the rotate `Fixture` (temporary HOME and profile
//! root, a fake `claude`, `isolated(..)`): `work` is live at 90/40, `spare`
//! reads 10/20 and `extra` 50/50 unless a test changes them.
//!
//! Contract:
//! - `profile overflow add <name>` marks an existing profile, writing
//!   `overflow=` last in `<root>/.auto-switch`; idempotent. Warns when the
//!   name is also excluded or is the `.default` profile. Prints the cost
//!   note (real money, https://claude.ai/settings/usage).
//! - `profile overflow remove <name>`: idempotent, any valid name.
//! - `profile overflow list`: the names in config order, with any block.
//! - `profile overflow clear <name>`: lifts a spend block under the rotate
//!   state lock (`<root>/.auto-switch.d/lock`), retrying ~10 s, then fails
//!   with "rotator busy, retry". No block: exit 0, ledger untouched.
//! - A corrupt `.auto-switch` is an error and is never rewritten.
//! - `profile rotate` moves onto an overflow profile only when every plan
//!   login is out (`reason=overflow-in`, `last_cycle.overflow = "in"`),
//!   stays on it (`"on"`), and moves off as soon as a plan login has
//!   capacity (`reason=overflow-out`, `"out"`).
//! - A spend-limit reading on an overflow profile stores one block in
//!   `overflow_blocked` with `reason: "spend_limit"` and writes one log line
//!   containing `ALERT` and `spend limit`; Claude's own text is never logged.
//! - `rotate status` prints `overflow:` and `on credits:` lines;
//!   `profile usage --json` rows carry `overflow` and
//!   `overflow_blocked_until`.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SPEND_LIMIT_OUT: &str = "You've hit your org's monthly spend limit · contact your admin\n";

fn auto_switch(f: &Fixture) -> PathBuf {
    f.root.join(".auto-switch")
}

fn stdout(out: &std::process::Output) -> String {
    text(&out.stdout)
}

fn stderr(out: &std::process::Output) -> String {
    text(&out.stderr)
}

fn mark(f: &Fixture, name: &str) {
    f.ok(&["profile", "overflow", "add", name]);
}

/// `extra` is overflow; every plan login (work, spare) is exhausted with no
/// known reset, so the earliest plan reset is unknown (far).
fn all_plan_logins_out(f: &Fixture) {
    mark(f, "extra");
    f.usage("work", 100, 40);
    f.usage("spare", 100, 100);
    f.usage("extra", 100, 100);
}

fn spend_limited(f: &Fixture, p: &str) {
    fs::write(f.canned.join(format!("{}.out", email(p))), SPEND_LIMIT_OUT).unwrap();
}

// --- Marking ---------------------------------------------------------------

#[test]
fn add_writes_overflow_last_and_is_idempotent() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "overflow", "add", "extra"]);
    let note = format!("{}{}", stdout(&out), stderr(&out));
    assert!(note.contains("https://claude.ai/settings/usage"), "{note}");
    assert!(note.to_lowercase().contains("credit"), "{note}");
    assert_eq!(
        fs::read_to_string(auto_switch(&f)).unwrap(),
        "enabled=off\nthreshold=90\nexclude=\noverflow=extra\n"
    );
    f.ok(&["profile", "overflow", "add", "extra"]);
    f.ok(&["profile", "overflow", "add", "spare"]);
    assert_eq!(
        fs::read_to_string(auto_switch(&f)).unwrap(),
        "enabled=off\nthreshold=90\nexclude=\noverflow=extra,spare\n",
        "no duplicate; config order kept"
    );
}

#[test]
fn add_keeps_the_existing_switch_settings() {
    let f = Fixture::new();
    private_file(&auto_switch(&f), "enabled=on\nthreshold=75\nexclude=work\n");
    mark(&f, "extra");
    assert_eq!(
        fs::read_to_string(auto_switch(&f)).unwrap(),
        "enabled=on\nthreshold=75\nexclude=work\noverflow=extra\n"
    );
}

#[test]
fn add_of_an_unknown_profile_fails_and_names_it() {
    let f = Fixture::new();
    let out = f.run(&["profile", "overflow", "add", "nosuch"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("nosuch"), "{}", stderr(&out));
    assert!(!auto_switch(&f).exists(), "nothing written");
}

#[test]
fn add_of_an_invalid_name_fails() {
    let f = Fixture::new();
    for bad in ["../x", ".hidden", "a/b"] {
        let out = f.run(&["profile", "overflow", "add", bad]);
        assert!(!out.status.success(), "{bad}");
    }
    assert!(!auto_switch(&f).exists());
}

#[test]
fn remove_is_idempotent_and_accepts_any_valid_name() {
    let f = Fixture::new();
    mark(&f, "extra");
    mark(&f, "spare");
    f.ok(&["profile", "overflow", "remove", "extra"]);
    assert_eq!(
        fs::read_to_string(auto_switch(&f)).unwrap(),
        "enabled=off\nthreshold=90\nexclude=\noverflow=spare\n"
    );
    f.ok(&["profile", "overflow", "remove", "extra"]);
    f.ok(&["profile", "overflow", "remove", "never-existed"]);
    f.ok(&["profile", "overflow", "remove", "spare"]);
    assert_eq!(
        fs::read_to_string(auto_switch(&f)).unwrap(),
        "enabled=off\nthreshold=90\nexclude=\n",
        "an empty list leaves no overflow= line"
    );
}

#[test]
fn list_prints_the_names_in_config_order() {
    let f = Fixture::new();
    let out = f.ok(&["profile", "overflow", "list"]);
    assert_eq!(stdout(&out), "overflow: none\n");
    mark(&f, "spare");
    mark(&f, "extra");
    let s = stdout(&f.ok(&["profile", "overflow", "list"]));
    let (a, b) = (s.find("spare").unwrap(), s.find("extra").unwrap());
    assert!(a < b, "{s}");
}

#[test]
fn add_warns_when_the_profile_is_excluded() {
    let f = Fixture::new();
    private_file(
        &auto_switch(&f),
        "enabled=on\nthreshold=90\nexclude=extra\n",
    );
    let out = f.ok(&["profile", "overflow", "add", "extra"]);
    let e = stderr(&out);
    assert!(e.contains("warning") && e.contains("exclude"), "{e}");
}

#[test]
fn add_warns_when_the_profile_is_the_default() {
    let f = Fixture::new();
    f.ok(&["profile", "switch", "extra"]);
    let out = f.ok(&["profile", "overflow", "add", "extra"]);
    let e = stderr(&out);
    assert!(e.contains("warning") && e.contains("default"), "{e}");
}

#[test]
fn a_corrupt_switch_file_is_an_error_and_is_not_rewritten() {
    let f = Fixture::new();
    let body = "enabled=sometimes\n";
    private_file(&auto_switch(&f), body);
    for args in [
        &["profile", "overflow", "add", "extra"][..],
        &["profile", "overflow", "remove", "extra"],
        &["profile", "overflow", "list"],
    ] {
        let out = f.run(args);
        assert!(!out.status.success(), "{args:?}");
        assert_eq!(
            fs::read_to_string(auto_switch(&f)).unwrap(),
            body,
            "{args:?}"
        );
    }
}

#[test]
fn auto_switch_exclude_keeps_the_overflow_mark() {
    let f = Fixture::new();
    mark(&f, "extra");
    f.ok(&["profile", "auto-switch", "exclude", "work"]);
    let raw = fs::read_to_string(auto_switch(&f)).unwrap();
    assert!(raw.contains("exclude=work\n"), "{raw}");
    assert!(raw.ends_with("overflow=extra\n"), "{raw}");
}

// --- clear -----------------------------------------------------------------

#[test]
fn clear_without_a_block_is_a_no_op() {
    let f = Fixture::new();
    f.ok(&["profile", "overflow", "clear", "extra"]);
    f.ok(&["profile", "overflow", "clear", "not-a-profile"]);
    assert!(f.ledger().is_none(), "clear never creates the ledger");
}

#[test]
fn clear_while_the_state_lock_is_held_fails_busy() {
    use fs4::fs_std::FileExt;
    let f = Fixture::new();
    f.ok(&["profile", "rotate"]); // creates the state directory and ledger
    let before = fs::read(f.rotate_dir().join("ledger.json")).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(f.root.join(".auto-switch.d").join("lock"))
        .unwrap();
    assert!(FileExt::try_lock_exclusive(&lock).unwrap());
    let start = Instant::now();
    let out = f.run(&["profile", "overflow", "clear", "extra"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("rotator busy, retry"),
        "{}",
        stderr(&out)
    );
    assert!(
        start.elapsed() >= Duration::from_secs(5),
        "it retries first"
    );
    assert_eq!(
        fs::read(f.rotate_dir().join("ledger.json")).unwrap(),
        before,
        "never written without the lock"
    );
}

// --- Rotator policy --------------------------------------------------------

#[test]
fn an_overflow_profile_is_not_chosen_while_a_plan_login_has_capacity() {
    let f = Fixture::new();
    mark(&f, "spare"); // 10/20: the best reading, but credit-backed
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        stdout(&out).contains("swap work->extra") && stdout(&out).contains("reason=threshold"),
        "{}",
        stdout(&out)
    );
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    let ledger = f.ledger().unwrap();
    assert!(ledger["last_cycle"].get("overflow").is_none(), "{ledger}");
}

#[test]
fn moves_onto_credits_when_every_plan_login_is_out() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    let out = f.ok(&["profile", "rotate"]);
    let s = stdout(&out);
    assert!(
        s.contains("swap work->extra") && s.contains("reason=overflow-in"),
        "{s}"
    );
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    assert!(
        s.contains("credits: on extra; no plan profile has capacity"),
        "{s}"
    );
    f.assert_each_login_once();
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["live"], "extra");
    assert_eq!(ledger["last_cycle"]["outcome"], "swapped");
    assert_eq!(ledger["last_cycle"]["overflow"], "in");
    assert!(
        ledger.get("alert").is_none(),
        "capacity for the hook: {ledger}"
    );
}

#[test]
fn a_dry_run_onto_credits_moves_nothing() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    let out = f.ok(&["profile", "rotate", "--dry-run"]);
    assert!(stdout(&out).contains("would"), "{}", stdout(&out));
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
}

#[test]
fn stays_on_credits_then_moves_off_when_a_plan_login_recovers() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));

    let out = f.ok(&["profile", "rotate"]);
    assert!(!stdout(&out).contains("swap "), "{}", stdout(&out));
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["last_cycle"]["outcome"], "stayed");
    assert_eq!(ledger["last_cycle"]["overflow"], "on");

    // spare is back, only just under the thresholds; the hold (default 1h)
    // and upgrade margin do not apply to leaving credits.
    f.usage("spare", 80, 80);
    let out = f.ok(&["profile", "rotate"]);
    let s = stdout(&out);
    assert!(
        s.contains("swap extra->spare") && s.contains("reason=overflow-out"),
        "{s}"
    );
    assert!(
        s.contains("credits: off extra; back on plan profile spare"),
        "{s}"
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["last_cycle"]["overflow"], "out");

    // And it does not flap back.
    let out = f.ok(&["profile", "rotate"]);
    assert!(!stdout(&out).contains("swap "), "{}", stdout(&out));
    assert!(f.ledger().unwrap()["last_cycle"].get("overflow").is_none());
}

#[test]
fn unmarking_mid_run_takes_effect_next_cycle() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    f.ok(&["profile", "overflow", "remove", "extra"]);
    let out = f.ok(&["profile", "rotate"]);
    assert!(!stdout(&out).contains("swap "), "{}", stdout(&out));
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(f.ledger().unwrap()["last_cycle"]["outcome"], "alerted");
}

#[test]
fn a_spend_limit_on_credits_blocks_once_and_alerts_once() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));

    spend_limited(&f, "extra");
    for _ in 0..3 {
        f.ok(&["profile", "rotate"]);
    }
    let ledger = f.ledger().unwrap();
    let block = &ledger["overflow_blocked"]["extra"];
    assert_eq!(block["reason"], "spend_limit", "{ledger}");
    let (at, until) = (
        block["at"].as_i64().unwrap(),
        block["until"].as_i64().unwrap(),
    );
    assert!(until > at, "{ledger}");
    let log = f.log();
    let alerts = log
        .lines()
        .filter(|l| l.contains("ALERT") && l.contains("spend limit"))
        .count();
    assert_eq!(alerts, 1, "{log}");
    assert!(
        !log.contains("You've hit"),
        "Claude's text is never logged: {log}"
    );

    // list shows the block; clear lifts it.
    let s = stdout(&f.ok(&["profile", "overflow", "list"]));
    assert!(s.contains("extra") && s.contains("spend_limit"), "{s}");
    f.ok(&["profile", "overflow", "clear", "extra"]);
    let ledger = f.ledger().unwrap();
    assert!(ledger.get("overflow_blocked").is_none(), "{ledger}");
}

#[test]
fn a_spend_limit_on_a_candidate_keeps_the_rotator_off_it() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    spend_limited(&f, "extra");
    let out = f.ok(&["profile", "rotate"]);
    assert!(!stdout(&out).contains("swap "), "{}", stdout(&out));
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(
        f.ledger().unwrap()["overflow_blocked"]["extra"]["reason"],
        "spend_limit"
    );
}

// --- Display ---------------------------------------------------------------

#[test]
fn rotate_status_shows_overflow_and_credits() {
    let f = Fixture::new();
    let s = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(s.contains("overflow: -\n"), "{s}");
    assert!(s.contains("on credits: "), "{s}");

    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    let s = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(s.contains("overflow: extra"), "{s}");
    assert!(s.contains("on credits: yes (extra)"), "{s}");
    assert!(s.contains(" overflow=in"), "{s}");
}

#[test]
fn profile_usage_json_marks_overflow_rows() {
    let f = Fixture::new();
    mark(&f, "extra");
    let out = f.ok(&["profile", "usage", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let rows = v.as_array().expect("still a top-level array");
    assert!(!rows.is_empty());
    for row in rows {
        let name = row["profile"].as_str().unwrap();
        assert_eq!(row["overflow"], (name == "extra"), "{row}");
        assert!(row.get("overflow_blocked_until").is_some(), "{row}");
        assert!(row["overflow_blocked_until"].is_null(), "{row}");
    }
}

#[test]
fn profile_usage_table_marks_overflow_and_running_on_credits() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    let s = stdout(&f.ok(&["profile", "usage"]));
    assert!(s.contains("overflow"), "{s}");
    assert!(!s.contains("running on credits"), "{s}");
    f.ok(&["profile", "rotate"]);
    let s = stdout(&f.ok(&["profile", "usage"]));
    assert!(s.contains("running on credits: extra"), "{s}");
}

#[test]
fn a_damaged_ledger_never_moves_onto_credits() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate", "--dry-run"]); // creates the state directory
    fs::create_dir_all(f.rotate_dir()).unwrap();
    private_file(
        &f.rotate_dir().join("ledger.json"),
        "{\"version\":1,\"surprise\":1}",
    );
    let _ = f.run(&["profile", "rotate"]);
    assert_eq!(
        ino(&f.home_login()),
        Some(f.live_ino),
        "fail closed on spend"
    );
    f.assert_each_login_once();
}

#[test]
fn rotate_help_documents_overflow_wait_and_the_subcommand_exists() {
    let f = Fixture::new();
    let s = stdout(&f.ok(&["profile", "rotate", "--help"]));
    assert!(s.contains("--overflow-wait"), "{s}");
    let s = stdout(&f.ok(&["profile", "overflow", "--help"]));
    for sub in ["add", "remove", "list", "clear"] {
        assert!(s.contains(sub), "{sub}: {s}");
    }
}

// --- Evidence for "on credits" and list details (issue #1606, step 7) ------

/// Rotates onto `extra` for plan reasons, then marks it: `extra` is live and
/// marked, but the ledger has no overflow tag, so it is not on credits.
fn live_on_a_marked_profile_with_capacity(f: &Fixture) {
    f.usage("spare", 100, 100);
    f.usage("extra", 10, 20);
    let out = f.ok(&["profile", "rotate"]);
    assert!(
        stdout(&out).contains("reason=threshold"),
        "{}",
        stdout(&out)
    );
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino));
    mark(f, "extra");
}

#[test]
fn rotate_status_needs_a_ledger_tag_to_say_on_credits() {
    let f = Fixture::new();
    live_on_a_marked_profile_with_capacity(&f);
    let s = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(s.contains("overflow: extra"), "{s}");
    assert!(!s.contains("on credits: yes"), "no in/on tag: {s}");
    assert!(s.contains("on credits: no"), "{s}");
}

#[test]
fn rotate_status_says_on_credits_while_staying_on_them() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    f.ok(&["profile", "rotate"]);
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["last_cycle"]["overflow"], "on", "{ledger}");
    let s = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(s.contains("on credits: yes (extra)"), "{s}");
}

#[test]
fn profile_usage_needs_a_reached_limit_to_say_running_on_credits() {
    let f = Fixture::new();
    live_on_a_marked_profile_with_capacity(&f);
    let s = stdout(&f.ok(&["profile", "usage"]));
    assert!(s.contains("overflow"), "{s}");
    assert!(!s.contains("running on credits"), "capacity remains: {s}");
}

#[test]
fn rotate_status_names_a_default_that_is_an_overflow_profile() {
    // D7: `.default` bypasses the rotator, so status says so.
    let f = Fixture::new();
    f.ok(&["profile", "switch", "extra"]);
    mark(&f, "extra");
    let s = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(s.contains("default: extra"), "{s}");
    f.ok(&["profile", "overflow", "remove", "extra"]);
    let s = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(!s.contains("default: "), "{s}");
}

#[test]
fn list_marks_a_profile_with_no_directory_as_missing() {
    let f = Fixture::new();
    private_file(
        &auto_switch(&f),
        "enabled=off\nthreshold=90\nexclude=\noverflow=extra,ghost\n",
    );
    let s = stdout(&f.ok(&["profile", "overflow", "list"]));
    let ghost = s
        .lines()
        .find(|l| l.starts_with("ghost"))
        .unwrap_or_else(|| panic!("{s}"));
    assert!(ghost.contains("missing"), "{s}");
    let extra = s
        .lines()
        .find(|l| l.starts_with("extra"))
        .unwrap_or_else(|| panic!("{s}"));
    assert!(!extra.contains("missing"), "{s}");
    assert!(s.find("extra").unwrap() < s.find("ghost").unwrap(), "{s}");
}

#[test]
fn list_with_a_damaged_ledger_warns_and_still_lists() {
    let f = Fixture::new();
    mark(&f, "extra");
    f.ok(&["profile", "rotate", "--dry-run"]);
    fs::create_dir_all(f.rotate_dir()).unwrap();
    private_file(&f.rotate_dir().join("ledger.json"), "{not json");
    let out = f.ok(&["profile", "overflow", "list"]);
    assert!(stdout(&out).contains("extra"), "{}", stdout(&out));
    assert!(stderr(&out).contains("warning"), "{}", stderr(&out));
}

#[test]
fn add_and_remove_of_an_unchanged_mark_do_not_rewrite_the_file() {
    let f = Fixture::new();
    mark(&f, "extra");
    let before = fs::symlink_metadata(auto_switch(&f)).unwrap();
    mark(&f, "extra");
    f.ok(&["profile", "overflow", "remove", "spare"]);
    let after = fs::symlink_metadata(auto_switch(&f)).unwrap();
    use std::os::unix::fs::MetadataExt;
    assert_eq!(before.ino(), after.ino(), "idempotent calls do not write");
}

#[test]
fn a_switch_file_writable_by_others_is_refused_and_not_rewritten() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let body = "enabled=on\nthreshold=90\nexclude=\noverflow=extra\n";
    private_file(&auto_switch(&f), body);
    fs::set_permissions(auto_switch(&f), fs::Permissions::from_mode(0o602)).unwrap();
    let out = f.run(&["profile", "overflow", "add", "spare"]);
    assert!(!out.status.success(), "{}", stdout(&out));
    assert_eq!(fs::read_to_string(auto_switch(&f)).unwrap(), body);
    // And the rotator treats it as no overflow profiles.
    all_plan_logins_out_unmarked(&f);
    f.ok(&["profile", "rotate"]);
    assert_eq!(
        ino(&f.home_login()),
        Some(f.live_ino),
        "fail closed on spend"
    );
}

fn all_plan_logins_out_unmarked(f: &Fixture) {
    f.usage("work", 100, 40);
    f.usage("spare", 100, 100);
    f.usage("extra", 100, 100);
}

#[test]
fn the_spend_limit_alert_carries_no_probe_text() {
    let f = Fixture::new();
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    spend_limited(&f, "extra");
    f.ok(&["profile", "rotate"]);
    let log = f.log();
    for line in log.lines().filter(|l| l.contains("spend limit")) {
        for probe in ["You've hit", "contact your admin", "\u{b7}"] {
            assert!(!line.contains(probe), "{line}");
        }
    }
    let ledger = f.ledger().unwrap().to_string();
    assert!(!ledger.contains("contact your admin"), "{ledger}");
}
