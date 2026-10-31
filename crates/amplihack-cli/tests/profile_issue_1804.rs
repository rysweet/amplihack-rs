//! Issue #1804: the rotator spends per-token money only with consent and
//! only when no plan login has room. End to end through the binary, with
//! the rotate `Fixture` (temporary HOME and profile root, a fake `claude`,
//! `isolated(..)`, no `AMPLIHACK_CLAUDE_PROFILE_ROOT` inherited from the
//! host). `work` is live; `spare` and `extra` are plan logins; `console` is
//! an API-key profile (`{}` login, no `oauthAccount`, no key setting unless
//! a test gives it one).
//!
//! Contract (docs/reference/profile-command.md, "Rotate plan-room swap",
//! "API-key login check", "Log lines for a move onto the API key"):
//! - With every threshold swap short of `--margin`, the rotator swaps to
//!   the plan login with the most weekly room among those with session
//!   room, logged `reason=plan-room`; an API-key profile is never chosen.
//! - An API-key profile that is not marked is never used, and the rotator
//!   writes a `skip:` line naming `overflow add`.
//! - A marked API-key profile whose login check fails is never swapped in:
//!   `~/.claude` is unchanged, no `per-token`, `swap` or `moved` line, a
//!   `key_check_refused <name>: <reason>` line and one ALERT over repeated
//!   cycles; `--dry-run` prints `would_refuse <name>: <reason>`.
//! - A marked API-key profile that passes the check is moved in with a
//!   `per-token` line first, then the swap line, then `moved ~/.claude onto
//!   API key profile <name> (billed per token)` (D4).
//! - `overflow add`, `overflow list` and `overflow remove` use the D6
//!   wording; `overflow add --tier N` on an API-key profile fails with
//!   `PlatformTierFixed` and writes nothing.
//! - `rotate status` labels the API-key profile `(marked)` or
//!   `(not marked)`.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;

const NO_KEY: &str = "no primaryApiKey, ANTHROPIC_API_KEY or apiKeyHelper setting";

fn stdout(out: &std::process::Output) -> String {
    text(&out.stdout)
}

/// Adds `console` and returns the inode of its `{}` login file.
fn add_console(f: &Fixture) -> u64 {
    f.ok(&["profile", "add", "console"]);
    let dir = f.root.join("console");
    private_file(&dir.join(".claude.json"), r#"{"numStartups":3}"#);
    private_file(&dir.join(CREDS), "{}");
    ino(&dir.join(CREDS)).unwrap()
}

/// Every probe or run of `claude` under a profile without an account.
fn console_runs(f: &Fixture) -> usize {
    fs::read_dir(&f.records)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("env.none.")
        })
        .count()
}

/// Every plan login at 100%, resetting in 3 hours: beyond the default
/// 30-minute overflow wait.
fn all_plan_logins_out(f: &Fixture) {
    f.usage_resetting("work", 100, 40, 3);
    f.usage_resetting("spare", 100, 100, 3);
    f.usage_resetting("extra", 100, 100, 3);
}

/// The issue's cycle in miniature: live past `--session-at`, spare with
/// session room and the most weekly room among those, extra with the lowest
/// week but its session nearly spent. No candidate beats the live login by
/// `--margin`.
fn near_limits(f: &Fixture) {
    f.usage_resetting("work", 87, 61, 3);
    f.usage_resetting("spare", 0, 91, 3);
    f.usage_resetting("extra", 98, 17, 3);
}

fn refused(name: &str) -> String {
    format!("key_check_refused {name}: {NO_KEY}")
}

fn alert(name: &str) -> String {
    format!("ALERT: API key profile {name} failed its login check ({NO_KEY}); not moving onto it")
}

/// As `Fixture::assert_each_login_once`, counting `console`'s `{}` login
/// too: every login is in exactly one place and nothing else exists.
fn each_login_once_with_console(f: &Fixture, console: u64) {
    let mut inos: Vec<u64> = f.logins().iter().map(|(_, i)| *i).collect();
    inos.sort();
    let mut want = vec![f.live_ino, f.spare_ino, f.extra_ino, console];
    want.sort();
    assert_eq!(inos, want, "logins: {:?}", f.logins());
}

// --- The plan-room swap -----------------------------------------------------------------

#[test]
fn t1_near_the_limits_with_no_mark_the_rotator_swaps_to_the_plan_login_with_room() {
    let f = Fixture::new();
    let console = add_console(&f);
    near_limits(&f);
    let out = f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "{log}");
    assert!(
        log.lines()
            .any(|l| l.contains("swap work->spare") && l.ends_with("reason=plan-room")),
        "{log}"
    );
    assert!(
        stdout(&out).contains("reason=plan-room"),
        "{}",
        stdout(&out)
    );
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
    assert!(!log.contains("onto API key profile"), "{log}");
    assert_eq!(console_runs(&f), 0, "the API key is never probed");
    each_login_once_with_console(&f, console);
}

#[test]
fn t2_near_the_limits_with_the_api_key_marked_the_rotator_still_picks_the_plan_login() {
    let f = Fixture::new();
    let console = add_console(&f);
    private_file(
        &f.root.join("console").join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    f.ok(&["profile", "overflow", "add", "console"]);
    near_limits(&f);
    f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "{log}");
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
    assert!(log.contains("reason=plan-room"), "{log}");
    assert!(!log.contains("SENTINEL-1804-KEY"));
}

#[test]
fn the_plan_room_swap_does_not_flap_back_on_the_next_cycle() {
    let f = Fixture::new();
    near_limits(&f);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "{}", f.log());
    // spare is now live at 0/91 (week past --week-at), short of work's
    // headroom by less than --margin. spare wins pass 1 itself (work at 87%
    // is past --session-at), so it stays.
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "{}", f.log());
    assert_eq!(
        f.log().matches("reason=plan-room").count(),
        1,
        "{}",
        f.log()
    );
}

#[test]
fn a_live_login_with_room_is_never_replaced_by_a_marked_api_key() {
    let f = Fixture::new();
    let console = add_console(&f);
    private_file(
        &f.root.join("console").join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    f.ok(&["profile", "overflow", "add", "console"]);
    f.usage_resetting("work", 87, 61, 3);
    f.usage_resetting("spare", 100, 100, 3);
    f.usage_resetting("extra", 100, 100, 3);
    f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert_eq!(ino(&f.home_login()), Some(f.live_ino), "{log}");
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
    assert!(!log.contains("onto API key profile"), "{log}");
}

// --- Consent ----------------------------------------------------------------------------

#[test]
fn an_unmarked_api_key_profile_is_never_used_even_with_every_plan_login_at_100() {
    let f = Fixture::new();
    let console = add_console(&f);
    private_file(
        &f.root.join("console").join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert_eq!(ino(&f.home_login()), Some(f.live_ino), "{log}");
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
    assert!(
        log.contains(
            "skip: API-key profile console is not marked; it is never used. To allow per-token billing, run: amplihack profile overflow add console"
        ),
        "{log}"
    );
    assert!(!log.contains("onto API key profile"), "{log}");
    assert_eq!(console_runs(&f), 0);
    let st = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(st.contains("api key: console (not marked)"), "{st}");
}

#[test]
fn a_dry_run_with_an_unmarked_api_key_never_names_it_as_a_target() {
    let f = Fixture::new();
    add_console(&f);
    all_plan_logins_out(&f);
    let s = stdout(&f.ok(&["profile", "rotate", "--dry-run"]));
    assert!(!s.contains("would swap work->console"), "{s}");
    assert!(!s.contains("overflow-in"), "{s}");
}

// --- The login check (T4 end to end) ---------------------------------------------------

#[test]
fn t4_a_marked_api_key_with_no_key_setting_is_refused_and_alerted_once() {
    let f = Fixture::new();
    let console = add_console(&f);
    f.ok(&["profile", "overflow", "add", "console"]);
    all_plan_logins_out(&f);
    let creds = f.home_login();
    let before = fs::symlink_metadata(&creds).unwrap();
    let home_json = fs::read(f.home.join(".claude.json")).unwrap();

    // Exit status is not part of this contract; the files and log are.
    f.run(&["profile", "rotate"]);
    f.run(&["profile", "rotate"]);

    use std::os::unix::fs::MetadataExt;
    let after = fs::symlink_metadata(&creds).unwrap();
    assert_eq!(after.ino(), before.ino(), "~/.claude is unchanged");
    assert_eq!(
        (after.mtime(), after.mtime_nsec()),
        (before.mtime(), before.mtime_nsec())
    );
    assert_eq!(fs::read(f.home.join(".claude.json")).unwrap(), home_json);
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
    assert!(!f.rotate_dir().join("journal.json").exists());
    each_login_once_with_console(&f, console);

    let log = f.log();
    for l in log.lines() {
        let body = l.split_once(' ').map_or(l, |(_, b)| b);
        assert!(
            !body.starts_with("swap ") && !body.starts_with("moved "),
            "no swap or moved line: {log}"
        );
    }
    assert!(log.contains(&refused("console")), "{log}");
    assert_eq!(log.matches(&alert("console")).count(), 1, "{log}");
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["live"], "work");
    assert_eq!(ledger["last_cycle"]["outcome"], "alerted");
    assert_ne!(ledger["last_cycle"]["overflow"], "in");
}

#[test]
fn a_dry_run_reports_that_it_would_refuse_a_failed_login_check() {
    let f = Fixture::new();
    add_console(&f);
    f.ok(&["profile", "overflow", "add", "console"]);
    all_plan_logins_out(&f);
    let s = stdout(&f.ok(&["profile", "rotate", "--dry-run"]));
    assert!(
        s.contains(&format!("would_refuse console: {NO_KEY}")),
        "{s}"
    );
    assert!(!s.contains("would swap work->console"), "{s}");
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
}

#[test]
fn rotate_status_says_when_the_api_key_is_marked() {
    let f = Fixture::new();
    add_console(&f);
    f.ok(&["profile", "overflow", "add", "console"]);
    let st = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(st.contains("api key: console (marked)"), "{st}");
}

// --- A passing login check (T3 end to end) ---------------------------------------------

#[test]
fn t3_a_marked_api_key_with_a_key_is_moved_in_with_the_per_token_line_first() {
    let f = Fixture::new();
    let console = add_console(&f);
    private_file(
        &f.root.join("console").join(".claude.json"),
        r#"{"numStartups":3,"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    f.ok(&["profile", "overflow", "add", "console"]);
    all_plan_logins_out(&f);
    f.run(&["profile", "rotate"]);

    let log = f.log();
    assert_eq!(ino(&f.home_login()), Some(console), "{log}");
    let bodies: Vec<&str> = log
        .lines()
        .map(|l| l.split_once(' ').map_or(l, |(_, b)| b))
        .collect();
    let at = |pred: &dyn Fn(&str) -> bool| bodies.iter().position(|b| pred(b));
    let reason = at(&|b| {
        b.starts_with(
            "per-token console: no plan login below 100%; overflow_wait elapsed (earliest reset ",
        ) && b.ends_with(
            "); marked overflow tier 10 via profile overflow add; login check passed (primaryApiKey set)",
        )
    })
    .unwrap_or_else(|| panic!("no per-token line: {log}"));
    let swap = at(&|b| b.starts_with("swap work->console") && b.ends_with("reason=overflow-in"))
        .unwrap_or_else(|| panic!("no swap line: {log}"));
    let moved = at(&|b| b == "moved ~/.claude onto API key profile console (billed per token)")
        .unwrap_or_else(|| panic!("no moved line: {log}"));
    assert!(reason < swap && swap < moved, "order: {log}");
    assert!(!log.contains("SENTINEL-1804-KEY"), "the key value leaked");
    let ledger = f.ledger().unwrap();
    assert!(!ledger.to_string().contains("SENTINEL-1804-KEY"));
    assert_eq!(ledger["last_cycle"]["overflow"], "in");
}

#[test]
fn a_dry_run_with_a_key_in_settings_never_prints_the_value() {
    let f = Fixture::new();
    add_console(&f);
    private_file(
        &f.root.join("console").join("settings.json"),
        r#"{"env":{"ANTHROPIC_API_KEY":"SENTINEL-1804-KEY"},"apiKeyHelper":"echo SENTINEL-1804-KEY"}"#,
    );
    f.ok(&["profile", "overflow", "add", "console"]);
    all_plan_logins_out(&f);
    let out = f.ok(&["profile", "rotate", "--dry-run"]);
    let s = stdout(&out);
    assert!(s.contains("would swap work->console"), "{s}");
    assert!(!s.contains("would_refuse"), "{s}");
    assert!(!s.contains("SENTINEL-1804-KEY"), "{s}");
    assert!(!text(&out.stderr).contains("SENTINEL-1804-KEY"));
    assert_eq!(
        ino(&f.home_login()),
        Some(f.live_ino),
        "a dry run moves nothing"
    );
}

// --- overflow add, list and remove (D6) -------------------------------------------------

fn stderr(out: &std::process::Output) -> String {
    text(&out.stderr)
}

#[test]
fn overflow_list_shows_an_unmarked_api_key_as_not_used() {
    let f = Fixture::new();
    add_console(&f);
    let s = stdout(&f.ok(&["profile", "overflow", "list"]));
    assert!(
        s.lines()
            .any(|l| l == "console  api-key tier  not marked (not used)"),
        "{s}"
    );
}

#[test]
fn overflow_add_marks_an_api_key_at_tier_10_with_the_cost_note() {
    let f = Fixture::new();
    add_console(&f);
    let s = stdout(&f.ok(&["profile", "overflow", "add", "console"]));
    assert!(
        s.lines()
            .any(|l| l == "overflow: added console (api-key tier, tier 10)"),
        "{s}"
    );
    assert!(
        s.lines().any(|l| l
            == "note: console is billed per token; it is used only when no plan login is \
                below 100% on both windows, no credit profile can be used, the earliest plan \
                reset is more than --overflow-wait away and its login check passes."),
        "{s}"
    );
    let list = stdout(&f.ok(&["profile", "overflow", "list"]));
    assert!(
        list.lines().any(|l| l == "console  api-key tier  marked"),
        "{list}"
    );
    assert!(!list.contains("not marked"), "{list}");
    let cfg = fs::read_to_string(f.root.join(".auto-switch")).unwrap();
    assert!(
        cfg.lines().any(|l| l == "overflow=console"),
        "stored as the bare name: {cfg}"
    );
}

#[test]
fn overflow_add_with_tier_on_an_api_key_fails_and_writes_nothing() {
    let f = Fixture::new();
    add_console(&f);
    let before = fs::read(f.root.join(".auto-switch")).ok();
    let out = f.run(&["profile", "overflow", "add", "console", "--tier", "3"]);
    assert!(!out.status.success(), "{}", stdout(&out));
    let e = stderr(&out);
    assert!(
        e.contains(
            "profile console is an API-key profile; the API-key tier is fixed at 10, \
             so --tier cannot be set"
        ),
        "{e}"
    );
    assert!(e.contains("without --tier"), "{e}");
    assert_eq!(fs::read(f.root.join(".auto-switch")).ok(), before);
    let list = stdout(&f.ok(&["profile", "overflow", "list"]));
    assert!(
        list.lines()
            .any(|l| l == "console  api-key tier  not marked (not used)"),
        "{list}"
    );
}

#[test]
fn overflow_remove_agrees_with_overflow_list() {
    let f = Fixture::new();
    add_console(&f);
    f.ok(&["profile", "overflow", "add", "console"]);
    let s = stdout(&f.ok(&["profile", "overflow", "remove", "console"]));
    assert!(s.lines().any(|l| l == "overflow: removed console"), "{s}");
    let list = stdout(&f.ok(&["profile", "overflow", "list"]));
    assert!(
        list.lines()
            .any(|l| l == "console  api-key tier  not marked (not used)"),
        "{list}"
    );
    let again = stdout(&f.ok(&["profile", "overflow", "remove", "console"]));
    assert!(
        again
            .lines()
            .any(|l| l == "overflow: console is not marked"),
        "{again}"
    );
}

#[test]
fn rotate_status_reads_overflow_dash_and_not_marked_with_nothing_marked() {
    let f = Fixture::new();
    add_console(&f);
    let st = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(st.lines().any(|l| l == "overflow: -"), "{st}");
    assert!(st.contains("api key: console (not marked)"), "{st}");
}
