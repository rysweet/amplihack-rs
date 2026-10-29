//! Issue #1700: API-key (Claude Platform) profiles, end to end through the
//! binary. Every run uses the rotate `Fixture` (temporary HOME and profile
//! root, a fake `claude`, `isolated(..)`): `work` is live at 90/40, `spare`
//! 10/20 and `extra` 50/50 unless a test changes them. Each test adds:
//! - `console`: an API-key profile, `.credentials.json` = `{}`, a
//!   `.claude.json` without `oauthAccount`, never probed;
//! - `ghost` (some tests): a subscription profile with no login at all.
//!
//! The fake `claude` writes `env.<email>.<pid>` for every run; a profile
//! without an `oauthAccount` records as `env.none.<pid>`, so "console was
//! never probed" is "no `env.none.*` record".
//!
//! Contract (docs/reference/profile-command.md, "API-key profiles"):
//! - `list` has a `KIND` column; `kind list` / `kind set` as documented.
//! - `usage` shows `api-key (billed per token)` with state `api_key`,
//!   exits 0, and never probes the API-key profile.
//! - `overflow add <api-key profile>` is refused; `overflow clear` needs
//!   exactly one of `<name>` and `--platform`; `overflow list` shows the
//!   `api-key tier` line.
//! - The launch guard refuses a subscription profile with no login of its
//!   own, before anything is spawned; an API-key profile runs.
//! - The rotator moves onto the API key only after plan logins and credits,
//!   by the ordinary journalled rename swap, records `live_kind`, logs the
//!   move, leaves `$HOME/.claude.json` untouched (no `oauthAccount`), and
//!   moves off at the next cycle whatever the hold.
//! - `switch <api-key profile>` moves its `{}` login into `~/.claude` by the
//!   same swap, pins it and prints a one-line per-token note; while the key
//!   is blocked it is refused with `PlatformBlocked` and changes nothing. A
//!   pinned API-key profile is held and never probed until `switch --none`.
//! - No key, token or canary value ever reaches output, log or state.

#![cfg(target_os = "linux")]

use crate::support::profile_rotate::*;
use std::fs;
use std::path::{Path, PathBuf};

// A value the tests search output for, to prove no secret value is ever
// printed. It is deliberately not shaped like an Anthropic key: secret
// scanners treat any `sk-ant-` literal as a leaked credential.
const CANARY: &str = "CANARY-LEAK-PROBE";

fn stdout(out: &std::process::Output) -> String {
    text(&out.stdout)
}

fn stderr(out: &std::process::Output) -> String {
    text(&out.stderr)
}

fn auto_switch(f: &Fixture) -> PathBuf {
    f.root.join(".auto-switch")
}

/// Adds `console` and returns the inode of its `{}` login file.
fn add_console(f: &Fixture) -> u64 {
    f.ok(&["profile", "add", "console"]);
    let dir = f.root.join("console");
    private_file(&dir.join(".claude.json"), r#"{"numStartups":3}"#);
    private_file(&dir.join(CREDS), "{}");
    ino(&dir.join(CREDS)).unwrap()
}

fn add_ghost(f: &Fixture) {
    f.ok(&["profile", "add", "ghost"]);
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

/// Every plan login (work, spare, extra) is exhausted and resets in 3
/// hours, beyond the default 30-minute overflow wait. No credit profile.
fn all_plan_logins_out(f: &Fixture) {
    f.usage_resetting("work", 100, 40, 3);
    f.usage_resetting("spare", 100, 100, 3);
    f.usage_resetting("extra", 100, 100, 3);
}

// --- list, kind, show ------------------------------------------------------------

#[test]
fn list_has_a_kind_column() {
    let f = Fixture::new();
    add_console(&f);
    let out = stdout(&f.ok(&["profile", "list"]));
    let header = out.lines().nth(1).unwrap_or_default();
    assert!(header.contains("NAME") && header.contains("KIND"), "{out}");
    let row = |p: &str| {
        out.lines()
            .find(|l| l.split_whitespace().any(|w| w == p))
            .unwrap_or_else(|| panic!("no row for {p}: {out}"))
            .to_string()
    };
    assert!(row("console").contains("api-key"), "{out}");
    assert!(row("spare").contains("subscription"), "{out}");
    assert!(
        row("work").contains("subscription"),
        "work is live in ~/.claude: {out}"
    );
}

#[test]
fn kind_list_shows_each_kind_and_its_source() {
    let f = Fixture::new();
    add_console(&f);
    add_ghost(&f);
    let out = stdout(&f.ok(&["profile", "kind", "list"]));
    let line = |p: &str| {
        out.lines()
            .find(|l| l.starts_with(&format!("{p} ")))
            .unwrap_or_else(|| panic!("no line for {p}: {out}"))
            .to_string()
    };
    assert!(line("console").contains("api-key"), "{out}");
    assert!(
        line("console").contains("(detected: login file has no oauth login)"),
        "{out}"
    );
    assert!(line("spare").contains("(detected: oauth login)"), "{out}");
    assert!(line("ghost").contains("(detected: no login)"), "{out}");
    assert!(!auto_switch(&f).exists(), "kind list is read-only");
}

#[test]
fn kind_set_writes_the_override_and_auto_removes_it() {
    let f = Fixture::new();
    add_ghost(&f);
    let out = f.ok(&["profile", "kind", "set", "ghost", "api-key"]);
    let s = stdout(&out);
    assert!(s.contains("kind: ghost is api-key (override)"), "{s}");
    assert!(s.contains("billed per token"), "{s}");
    assert!(s.contains("stored outside the profile directory"), "{s}");
    assert_eq!(
        fs::read_to_string(auto_switch(&f)).unwrap(),
        "enabled=off\nthreshold=90\nexclude=\nkind=ghost:platform\n"
    );
    f.ok(&["profile", "kind", "set", "spare", "subscription"]);
    assert!(
        fs::read_to_string(auto_switch(&f))
            .unwrap()
            .ends_with("kind=ghost:platform,spare:subscription\n")
    );
    let s = stdout(&f.ok(&["profile", "kind", "set", "ghost", "auto"]));
    assert!(
        s.contains(
            "kind: removed the override on ghost; detected subscription (detected: no login)"
        ),
        "{s}"
    );
    let s = stdout(&f.ok(&["profile", "kind", "set", "ghost", "auto"]));
    assert!(s.contains("kind: ghost has no override"), "{s}");
    // `auto` accepts a valid name with no profile directory.
    f.ok(&["profile", "kind", "set", "gone", "auto"]);
}

#[test]
fn kind_set_rejects_an_unknown_kind_or_profile() {
    let f = Fixture::new();
    let out = f.run(&["profile", "kind", "set", "spare", "enterprise"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = f.run(&["profile", "kind", "set", "nosuch", "api-key"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("nosuch"), "{}", stderr(&out));
    assert!(!auto_switch(&f).exists(), "nothing written");
}

#[test]
fn show_prints_the_kind_and_whether_a_key_is_configured() {
    let f = Fixture::new();
    add_console(&f);
    private_file(
        &f.root.join("console/settings.json"),
        &format!(r#"{{"env":{{"ANTHROPIC_API_KEY":"{CANARY}"}}}}"#),
    );
    f.ok(&["profile", "switch", "console"]);
    let s = stdout(&f.ok(&["profile", "show"]));
    assert!(s.contains("name: console"), "{s}");
    assert!(
        // `switch` moved its `{}` login into `~/.claude` (issue #1691).
        s.contains("kind: api-key (detected: live in ~/.claude (no oauth login))"),
        "{s}"
    );
    assert!(s.contains("API key configured: yes"), "{s}");
    assert!(!s.contains(CANARY), "{s}");
}

#[test]
fn an_oauth_profile_with_a_key_setting_is_a_subscription_with_a_warning() {
    let f = Fixture::new();
    private_file(
        &f.root.join("spare/settings.json"),
        &format!(r#"{{"apiKeyHelper":"echo {CANARY}"}}"#),
    );
    let out = f.ok(&["profile", "list"]);
    assert!(
        stderr(&out).contains(
            "warning: profile spare has an oauth login and an API-key setting is also configured; Claude may bill per token."
        ),
        "{}",
        stderr(&out)
    );
    f.ok(&["profile", "kind", "set", "spare", "subscription"]);
    let out = f.ok(&["profile", "list"]);
    assert!(
        !stderr(&out).contains("warning: profile spare"),
        "explicit kind silences it"
    );
}

// --- usage -----------------------------------------------------------------------

#[test]
fn usage_shows_the_api_key_row_without_probing_it() {
    let f = Fixture::new();
    add_console(&f);
    let out = f.ok(&["profile", "usage"]);
    let s = stdout(&out);
    let row = s
        .lines()
        .find(|l| l.starts_with("console"))
        .unwrap_or_else(|| panic!("{s}"));
    assert!(row.contains("api_key"), "{row}");
    assert!(row.contains("api-key (billed per token)"), "{row}");
    for bad in ["unparsed", "timeout", "failed", "not_logged_in"] {
        assert!(!row.contains(bad), "{row}");
    }
    assert_eq!(console_runs(&f), 0, "never probed");
    // API-key rows come after every subscription row.
    let names: Vec<&str> = s
        .lines()
        .skip(1)
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(names.last(), Some(&"console"), "{s}");
}

#[test]
fn usage_json_carries_the_kind() {
    let f = Fixture::new();
    add_console(&f);
    let out = f.ok(&["profile", "usage", "--json"]);
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    let console = rows.iter().find(|r| r["profile"] == "console").unwrap();
    assert_eq!(console["kind"], "api-key");
    assert_eq!(console["state"], "api_key");
    assert!(console["session"].is_null());
    assert!(console["api_key_blocked"].is_null());
    let spare = rows.iter().find(|r| r["profile"] == "spare").unwrap();
    assert_eq!(spare["kind"], "subscription");
    assert_eq!(console_runs(&f), 0);
}

// --- overflow ------------------------------------------------------------------------

#[test]
fn an_api_key_profile_cannot_be_marked_as_overflow() {
    let f = Fixture::new();
    add_console(&f);
    let out = f.run(&["profile", "overflow", "add", "console"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains(
            "profile console is an API-key profile; API-key profiles are always the last overflow tier and cannot be marked"
        ),
        "{}",
        stderr(&out)
    );
    assert!(!auto_switch(&f).exists(), "nothing written");
}

#[test]
fn overflow_list_shows_the_api_key_tier_after_the_marks() {
    let f = Fixture::new();
    add_console(&f);
    f.ok(&["profile", "overflow", "add", "extra"]);
    let s = stdout(&f.ok(&["profile", "overflow", "list"]));
    assert!(s.starts_with("extra  tier 1\nconsole  api-key tier"), "{s}");
}

#[test]
fn overflow_clear_needs_exactly_one_of_a_name_and_platform() {
    let f = Fixture::new();
    let neither = f.run(&["profile", "overflow", "clear"]);
    assert_eq!(neither.status.code(), Some(2), "{}", stderr(&neither));
    let both = f.run(&["profile", "overflow", "clear", "extra", "--platform"]);
    assert_eq!(both.status.code(), Some(2), "{}", stderr(&both));
    let s = stdout(&f.ok(&["profile", "overflow", "clear", "--platform"]));
    assert!(s.contains("overflow: the API key is not blocked"), "{s}");
}

// --- Launch guard ----------------------------------------------------------------------

const GUARD: &str = "error: profile ghost is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. Run 'amplihack profile login ghost', or run 'amplihack profile kind set ghost api-key' if per-token billing is intended.";

fn runs(f: &Fixture) -> usize {
    fs::read_dir(&f.records).unwrap().count()
}

#[test]
fn run_refuses_a_subscription_profile_with_no_login_before_spawning() {
    let f = Fixture::new();
    add_ghost(&f);
    let before = runs(&f);
    let out = f.run(&["profile", "run", "ghost", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(GUARD), "{}", stderr(&out));
    assert_eq!(runs(&f), before, "claude never started");
}

#[test]
fn an_inherited_profile_with_no_login_is_refused_too() {
    let f = Fixture::new();
    add_ghost(&f);
    let before = runs(&f);
    let out = f
        .cmd(&["claude", "--subprocess-safe", "--", "-p", "hi"])
        .env("AMPLIHACK_CLAUDE_PROFILE", "ghost")
        .output()
        .unwrap();
    assert_no_secrets(&out);
    assert!(!out.status.success());
    assert!(stderr(&out).contains(GUARD), "{}", stderr(&out));
    assert_eq!(runs(&f), before, "claude never started");
}

#[test]
fn switch_refuses_a_subscription_profile_with_no_login() {
    let f = Fixture::new();
    add_ghost(&f);
    let out = f.run(&["profile", "switch", "ghost"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(GUARD), "{}", stderr(&out));
    let s = stdout(&f.ok(&["profile", "show"]));
    assert!(!s.contains("name: ghost"), "default unchanged: {s}");
}

#[test]
fn a_damaged_login_file_is_refused_never_treated_as_an_api_key() {
    let f = Fixture::new();
    add_ghost(&f);
    private_file(&f.root.join("ghost").join(CREDS), "not json");
    let out = f.run(&["profile", "run", "ghost", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains(GUARD), "{}", stderr(&out));
}

#[test]
fn an_api_key_profile_runs() {
    let f = Fixture::new();
    add_console(&f);
    f.ok(&["profile", "run", "console", "--", "-p", "reply with OK"]);
    assert_eq!(console_runs(&f), 1);
    let rec = f.probe_envs().join("\n");
    assert!(
        rec.contains(&format!(
            "CLAUDE_CONFIG_DIR={}",
            f.root.join("console").display()
        )),
        "{rec}"
    );
}

#[test]
fn an_override_to_api_key_lets_a_login_less_profile_run() {
    let f = Fixture::new();
    add_ghost(&f);
    f.ok(&["profile", "kind", "set", "ghost", "api-key"]);
    f.ok(&["profile", "run", "ghost", "--", "-p", "hi"]);
}

#[test]
fn a_corrupt_toggle_file_refuses_profiles_without_an_oauth_login() {
    let f = Fixture::new();
    add_console(&f);
    private_file(
        &auto_switch(&f),
        "enabled=off\nkind=console:platform,console:platform\n",
    );
    let out = f.run(&["profile", "run", "console", "--", "-p", "hi"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(console_runs(&f), 0);
    // An OAuth profile still runs.
    f.ok(&["profile", "run", "spare", "--", "-p", "hi"]);
}

// --- Rotation ---------------------------------------------------------------------------

#[test]
fn a_dry_run_onto_the_api_key_moves_nothing_and_probes_nothing() {
    let f = Fixture::new();
    add_console(&f);
    all_plan_logins_out(&f);
    let s = stdout(&f.ok(&["profile", "rotate", "--dry-run"]));
    assert!(s.contains("console"), "{s}");
    assert!(s.contains("overflow-in"), "{s}");
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(console_runs(&f), 0);
}

#[test]
fn credits_are_used_before_the_api_key() {
    let f = Fixture::new();
    add_console(&f);
    all_plan_logins_out(&f);
    f.ok(&["profile", "overflow", "add", "extra"]);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino), "{}", f.log());
}

#[test]
fn the_wait_gate_keeps_work_off_the_api_key() {
    let f = Fixture::new();
    let console = add_console(&f);
    f.usage_resetting("work", 100, 40, 3);
    f.usage_resetting("spare", 100, 100, 3);
    // extra is back in 1 hour, within the 2-hour wait given below.
    f.usage_resetting("extra", 100, 100, 1);
    f.ok(&["profile", "rotate", "--overflow-wait", "7200"]);
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
}

#[test]
fn rotate_moves_onto_the_api_key_and_off_it_at_the_next_cycle() {
    let f = Fixture::new();
    let console = add_console(&f);
    all_plan_logins_out(&f);
    let home_json = fs::read(f.home.join(".claude.json")).unwrap();

    let out = f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert!(
        log.contains("moved ~/.claude onto API key profile console (billed per token)"),
        "{log}"
    );
    assert!(
        stdout(&out).contains("reason=overflow-in"),
        "{}",
        stdout(&out)
    );
    // The `{}` was renamed into ~/.claude; work's login went home.
    assert_eq!(ino(&f.home_login()), Some(console));
    assert_eq!(ino(&f.root.join("work").join(CREDS)), Some(f.live_ino));
    assert!(!f.root.join("console").join(CREDS).exists());
    assert!(
        !f.rotate_dir().join("journal.json").exists(),
        "journal done"
    );
    assert_eq!(
        fs::read(f.home.join(".claude.json")).unwrap(),
        home_json,
        "no oauthAccount: $HOME/.claude.json is left byte for byte"
    );
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["live"], "console");
    assert_eq!(ledger["live_kind"], "platform");
    assert_eq!(ledger["last_cycle"]["overflow"], "in");
    assert!(
        ledger.get("account").is_none_or(|a| a.is_null()),
        "{ledger}"
    );
    assert_eq!(console_runs(&f), 0, "the API key is never probed");

    // Status and usage say plainly that work is billed per token.
    let st = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(
        st.contains("~/.claude is on API key profile console: work is billed per token (since "),
        "{st}"
    );
    assert!(st.contains("on overflow: yes (console)"), "{st}");
    assert!(st.contains("api key: console"), "{st}");
    let us = stdout(&f.ok(&["profile", "usage"]));
    assert!(
        us.contains("live: work is running on the API key (billed per token)"),
        "{us}"
    );
    assert_eq!(console_runs(&f), 0);

    // A second cycle with nothing better stays, tagged `on`.
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(console));
    assert_eq!(f.ledger().unwrap()["last_cycle"]["overflow"], "on");

    // spare has capacity again: off at once, though the 1-hour hold has
    // not passed.
    f.usage("spare", 10, 20);
    f.ok(&["profile", "rotate"]);
    let log = f.log();
    assert!(
        log.contains("moved ~/.claude off API key profile console to spare"),
        "{log}"
    );
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino));
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
    let ledger = f.ledger().unwrap();
    assert_eq!(ledger["live"], "spare");
    assert!(
        ledger.get("live_kind").is_none_or(|k| k.is_null()),
        "{ledger}"
    );
    assert_eq!(f.home_account(), "uuid-spare");
    assert_eq!(console_runs(&f), 0);
}

#[test]
fn rotate_moves_off_the_api_key_to_credits_when_no_plan_login_has_capacity() {
    let f = Fixture::new();
    let console = add_console(&f);
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(console));
    f.ok(&["profile", "overflow", "add", "extra"]);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.extra_ino), "{}", f.log());
    assert!(
        f.log()
            .contains("moved ~/.claude off API key profile console to extra"),
        "{}",
        f.log()
    );
}

#[test]
fn a_subscription_profile_without_an_oauth_login_is_never_swapped_in() {
    let f = Fixture::new();
    let console = add_console(&f);
    f.ok(&["profile", "kind", "set", "console", "subscription"]);
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    f.ok(&["profile", "rotate"]);
    assert_eq!(
        ino(&f.home_login()),
        Some(f.live_ino),
        "~/.claude keeps an OAuth login"
    );
    assert_eq!(ino(&f.root.join("console").join(CREDS)), Some(console));
    assert!(!f.rotate_dir().join("journal.json").exists());
}

#[test]
fn an_api_key_profile_without_a_login_file_is_never_swapped_in() {
    let f = Fixture::new();
    add_console(&f);
    fs::remove_file(f.root.join("console").join(CREDS)).unwrap();
    // Still an API key, by its setting name.
    private_file(
        &f.root.join("console/settings.json"),
        &format!(r#"{{"apiKeyHelper":"echo {CANARY}"}}"#),
    );
    all_plan_logins_out(&f);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.live_ino));
    assert!(
        !f.root.join("console").join(CREDS).exists(),
        "an empty login file is never created"
    );
}

#[test]
fn a_blocked_api_key_is_not_used_and_clear_platform_lifts_the_block() {
    let f = Fixture::new();
    add_console(&f);
    all_plan_logins_out(&f);
    write_platform_block(&f, "key_exhausted");
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.live_ino), "{}", f.log());
    let list = stdout(&f.ok(&["profile", "overflow", "list"]));
    assert!(
        list.contains("console  api-key tier  blocked until "),
        "{list}"
    );
    assert!(list.contains("(key_exhausted)"), "{list}");
    let s = stdout(&f.ok(&["profile", "overflow", "clear", "--platform"]));
    assert!(
        s.contains("overflow: cleared the API-key block (key_exhausted)"),
        "{s}"
    );
    assert!(f.ledger().unwrap().get("platform_block").is_none());
}

/// A key block in the ledger, as `platform_block::record` writes it.
fn write_platform_block(f: &Fixture, reason: &str) {
    // `--dry-run` writes nothing, so the state directories are made here,
    // private as the rotator requires.
    use std::os::unix::fs::PermissionsExt;
    let rotate_dir = f.rotate_dir();
    for d in [rotate_dir.parent().unwrap(), rotate_dir.as_path()] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let now = chrono::Utc::now().timestamp();
    let mut l = f
        .ledger()
        .unwrap_or_else(|| serde_json::json!({"version": 1}));
    l["platform_block"] =
        serde_json::json!({"at": now - 10, "until": now + 86_400, "reason": reason});
    private_file(
        &f.rotate_dir().join("ledger.json"),
        &serde_json::to_string(&l).unwrap(),
    );
}

// --- switch onto an API-key profile (D8) ----------------------------------------------

const SWITCH_NOTE: &str = "note: console is an API-key profile; work in ~/.claude is billed per \
                           token until you run: amplihack profile switch --none";

/// The files a refused `switch` must leave alone.
fn switch_state(f: &Fixture) -> Vec<(&'static str, Option<Vec<u8>>)> {
    let read = |p: PathBuf| fs::read(p).ok();
    vec![
        ("ledger", read(f.rotate_dir().join("ledger.json"))),
        ("journal", read(f.rotate_dir().join("journal.json"))),
        ("pin", read(f.rotate_dir().join("pin"))),
        (".default", read(f.root.join(".default"))),
        ("~/.claude.json", read(f.home.join(".claude.json"))),
    ]
}

#[test]
fn switch_onto_an_api_key_profile_renames_its_login_in_pins_and_says_it_bills() {
    let f = Fixture::new();
    let console = add_console(&f);
    let out = f.ok(&["profile", "switch", "console"]);
    let all = format!("{}{}", stdout(&out), stderr(&out));
    assert!(all.contains(SWITCH_NOTE), "{all}");
    assert!(
        stdout(&out).contains("Live login moved: work -> console"),
        "{}",
        stdout(&out)
    );
    // The `{}` file itself, by inode, is in ~/.claude, and work's login is
    // back in work's directory: renamed, never copied.
    assert_eq!(ino(&f.home_login()), Some(console), "{}", f.log());
    assert_eq!(ino(&f.slot("work")), Some(f.live_ino));
    assert_eq!(ino(&f.slot("console")), None, "moved, not copied");
    assert_eq!(f.where_is(console).as_deref(), Some("~/.claude"));
    assert!(
        !f.rotate_dir().join("journal.json").exists(),
        "the journal is closed once the swap completes"
    );
    assert_eq!(
        fs::read_to_string(f.rotate_dir().join("pin")).unwrap(),
        "console\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join(".default")).unwrap(),
        "console\n"
    );
    let l = f.ledger().unwrap();
    assert_eq!(l["live"], "console", "{l}");
    assert_eq!(l["live_kind"], "platform", "{l}");
    assert_eq!(console_runs(&f), 0, "switch never runs claude for the key");
    assert_text_has_no_secrets(&all);
}

#[test]
fn switch_refuses_an_api_key_profile_while_the_key_is_blocked_and_changes_nothing() {
    let f = Fixture::new();
    add_console(&f);
    write_platform_block(&f, "key_exhausted");
    let before = (switch_state(&f), f.logins());
    let out = f.run(&["profile", "switch", "console"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("error: profile console: the API key is blocked (key_exhausted) until "),
        "{err}"
    );
    assert!(
        err.contains("hint: amplihack profile overflow clear --platform"),
        "{err}"
    );
    assert!(!stdout(&out).contains(SWITCH_NOTE), "{}", stdout(&out));
    // No pin, no default pointer, no swap: a pin would hold the login on a
    // key that cannot work, and the pointer would launch it directly.
    assert_eq!(
        (switch_state(&f), f.logins()),
        before,
        "a refused switch changed something"
    );
    assert_eq!(console_runs(&f), 0);
}

#[test]
fn switch_refuses_a_blocked_api_key_profile_when_the_live_login_is_unusable_and_changes_nothing() {
    let f = Fixture::new();
    add_console(&f);
    // No login in ~/.claude: the switch would otherwise write `.default`
    // alone, and a launch of the default would use the blocked key.
    fs::remove_file(f.home_login()).unwrap();
    write_platform_block(&f, "key_exhausted");
    let before = (switch_state(&f), f.logins());
    let out = f.run(&["profile", "switch", "console"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let err = stderr(&out);
    assert!(
        err.contains("the API key is blocked (key_exhausted) until "),
        "{err}"
    );
    assert!(
        err.contains("hint: amplihack profile overflow clear --platform"),
        "{err}"
    );
    assert_eq!(
        (switch_state(&f), f.logins()),
        before,
        "a refused switch changed something"
    );
    assert_eq!(console_runs(&f), 0);
}

#[test]
fn a_pinned_api_key_profile_is_held_unprobed_until_switch_none() {
    let f = Fixture::new();
    let console = add_console(&f);
    f.ok(&["profile", "switch", "console"]);
    // spare has plan capacity, but the pin wins: the user chose to pay.
    f.usage("spare", 10, 20);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(console), "{}", f.log());
    assert_eq!(console_runs(&f), 0, "a held cycle never probes the key");
    let status = stdout(&f.ok(&["profile", "rotate", "status"]));
    assert!(
        status
            .contains("~/.claude is on API key profile console: work is billed per token (since "),
        "{status}"
    );
    // Released: the next cycle moves off the key at once, to a plan login.
    f.ok(&["profile", "switch", "--none"]);
    f.ok(&["profile", "rotate"]);
    assert_eq!(ino(&f.home_login()), Some(f.spare_ino), "{}", f.log());
    assert_eq!(ino(&f.slot("console")), Some(console), "{}", f.log());
    assert!(
        f.log()
            .contains("moved ~/.claude off API key profile console to spare"),
        "{}",
        f.log()
    );
    assert_eq!(console_runs(&f), 0);
    let l = f.ledger().unwrap();
    assert!(l.get("live_kind").is_none(), "{l}");
}

// --- Canary --------------------------------------------------------------------------------

/// Every file under `dir`, recursively, without following symlinks.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            files(&p, out);
        } else if ft.is_file() {
            out.push(p);
        }
    }
}

#[test]
fn no_key_value_ever_reaches_output_log_or_state() {
    let f = Fixture::new();
    add_console(&f);
    let dir = f.root.join("console");
    private_file(
        &dir.join("settings.json"),
        &format!(r#"{{"env":{{"ANTHROPIC_API_KEY":"{CANARY}"}},"apiKeyHelper":"echo {CANARY}"}}"#),
    );
    private_file(
        &dir.join(".claude.json"),
        &format!(r#"{{"primaryApiKey":"{CANARY}","numStartups":1}}"#),
    );
    private_file(&dir.join(CREDS), &format!(r#"{{"extra":"{CANARY}"}}"#));
    add_ghost(&f);
    private_file(
        &f.root.join("ghost").join(CREDS),
        &format!("garbage {CANARY}"),
    );
    all_plan_logins_out(&f);

    let mut outputs = Vec::new();
    for args in [
        vec!["profile", "list"],
        vec!["profile", "kind", "list"],
        vec!["profile", "usage"],
        vec!["profile", "usage", "--json"],
        vec!["profile", "overflow", "list"],
        vec!["profile", "overflow", "add", "console"],
        vec!["profile", "rotate", "--dry-run"],
        vec!["profile", "rotate"],
        vec!["profile", "rotate", "status"],
        vec!["profile", "auto-switch", "status"],
        vec!["profile", "run", "ghost", "--", "-p", "hi"],
        vec!["profile", "switch", "ghost"],
        vec!["profile", "kind", "set", "console", "subscription"],
        vec!["profile", "rotate"],
        vec!["profile", "kind", "set", "console", "auto"],
    ] {
        let out = f.run(&args);
        outputs.push((args.join(" "), stdout(&out), stderr(&out)));
    }
    let switch = f.run(&["profile", "switch", "console"]);
    outputs.push((
        "profile switch console".into(),
        stdout(&switch),
        stderr(&switch),
    ));
    let show = f.run(&["profile", "show"]);
    outputs.push(("profile show".into(), stdout(&show), stderr(&show)));

    for (cmd, o, e) in &outputs {
        assert!(!o.contains(CANARY) && !e.contains(CANARY), "{cmd}: {o}{e}");
        assert_text_has_no_secrets(o);
        assert_text_has_no_secrets(e);
    }
    // amplihack's own files: everything under the root except the profile
    // directories themselves, which hold the fixture's canaries.
    let mut all = Vec::new();
    files(&f.root.join(".auto-switch.d"), &mut all);
    all.push(auto_switch(&f));
    for p in all {
        let body = fs::read(&p).unwrap_or_default();
        let body = String::from_utf8_lossy(&body);
        assert!(!body.contains(CANARY), "{}: {body}", p.display());
        assert!(!body.contains("SENTINEL"), "{}: {body}", p.display());
    }
}
