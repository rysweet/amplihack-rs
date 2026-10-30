//! No-owner mode of `profile statusline` (issue #1745): the command in
//! `~/.claude/settings.json`. The inner-run rule, the `statusline.sh` checks,
//! the root rule, and `tick`: owner from the rotator ledger only, the reading
//! recorded under the state lock whatever the auto-switch setting, and the
//! capacity line.

use super::home_line::{inner_run, root_for, statusline_script, tick};
use super::state::{HostEntry, Ledger, State};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const HOUR: i64 = 3600;
const LINE_34_43: &str = "🔑 X  5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%";

fn now() -> i64 {
    super::fsx::now()
}

fn private(path: &Path, body: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

fn mode(path: &Path, m: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(m)).unwrap();
}

/// Status-line input like Claude Code's.
fn input(five: f64, seven: f64) -> Vec<u8> {
    serde_json::json!({
        "session_id": "s",
        "rate_limits": {
            "five_hour": {"used_percentage": five, "resets_at": now() + HOUR},
            "seven_day": {"used_percentage": seven, "resets_at": now() + 3 * 86_400},
        }
    })
    .to_string()
    .into_bytes()
}

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
}

/// A private profile root holding profiles `X` and `Y` (neither logged in)
/// and a `HOME` whose `~/.claude` holds a login. No ledger yet.
fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let root = tmp.path().join("root");
    fs::create_dir_all(&home).unwrap();
    mode(&home, 0o700);
    fs::create_dir(&root).unwrap();
    mode(&root, 0o700);
    let root = root.canonicalize().unwrap();
    let store = ProfileStore::new(root.clone());
    for n in ["X", "Y"] {
        store.add(&ProfileName::parse(n).unwrap()).unwrap();
    }
    private(&home.join(".claude/.credentials.json"), b"{}");
    Env {
        _tmp: tmp,
        home,
        root,
    }
}

impl Env {
    fn prototype_ledger(&self, body: &str) -> PathBuf {
        let p = self.root.join(".rotator/ledger.json");
        private(&p, body.as_bytes());
        mode(p.parent().unwrap(), 0o700);
        p
    }

    /// `profile rotate`'s ledger, with no `account` so `~/.claude.json`
    /// never comes into play.
    fn rotate_ledger(&self, live: &str) -> PathBuf {
        self.rotate_ledger_body(serde_json::json!({"version": 1, "live": live, "since": 10}))
    }

    fn rotate_ledger_body(&self, body: serde_json::Value) -> PathBuf {
        let p = self.root.join(".auto-switch.d/rotate/ledger.json");
        private(&p, body.to_string().as_bytes());
        mode(&self.root.join(".auto-switch.d"), 0o700);
        mode(p.parent().unwrap(), 0o700);
        p
    }

    fn state_dir(&self) -> PathBuf {
        self.root.join(".auto-switch.d")
    }

    fn usage(&self) -> Option<serde_json::Value> {
        let raw = fs::read(self.state_dir().join("usage.json")).ok()?;
        Some(serde_json::from_slice(&raw).unwrap())
    }

    fn tick(&self, input: &[u8]) -> Option<String> {
        tick(&self.root, &self.home, input, now())
    }
}

// ---------------------------------------------------------------------------
// inner_run
// ---------------------------------------------------------------------------

#[test]
fn unset_or_empty_config_dir_is_not_an_inner_run() {
    let e = env();
    assert!(!inner_run(None, &e.home));
    assert!(!inner_run(Some(OsStr::new("")), &e.home));
}

#[test]
fn the_default_directory_is_not_an_inner_run() {
    let e = env();
    let dot = e.home.join(".claude");
    assert!(!inner_run(Some(dot.as_os_str()), &e.home));
    let slash = format!("{}/", dot.display());
    assert!(
        !inner_run(Some(OsStr::new(&slash)), &e.home),
        "a trailing slash is ignored"
    );
}

#[test]
fn a_symlink_to_the_default_directory_is_not_an_inner_run() {
    let e = env();
    let link = e.home.join("claude-link");
    std::os::unix::fs::symlink(e.home.join(".claude"), &link).unwrap();
    assert!(!inner_run(Some(link.as_os_str()), &e.home));
}

#[test]
fn a_profile_directory_is_an_inner_run() {
    let e = env();
    let x = e.root.join("X");
    assert!(inner_run(Some(x.as_os_str()), &e.home));
    let missing = e.root.join("nowhere");
    assert!(
        inner_run(Some(missing.as_os_str()), &e.home),
        "a directory that does not resolve is still not ~/.claude"
    );
}

// ---------------------------------------------------------------------------
// statusline.sh checks (S1)
// ---------------------------------------------------------------------------

fn script(home: &Path) -> PathBuf {
    let p = home.join(".amplihack/.claude/tools/statusline.sh");
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, "#!/bin/sh\nprintf 'first\\n'\n").unwrap();
    mode(&p, 0o700);
    for d in [
        ".amplihack",
        ".amplihack/.claude",
        ".amplihack/.claude/tools",
    ] {
        mode(&home.join(d), 0o700);
    }
    p
}

#[test]
fn a_private_script_is_found() {
    let e = env();
    let p = script(&e.home);
    assert_eq!(statusline_script(&e.home), Some(p));
}

#[test]
fn a_missing_script_is_not_found() {
    let e = env();
    assert_eq!(statusline_script(&e.home), None);
}

#[test]
fn a_world_writable_script_is_refused() {
    // World-writable rather than group-writable: a user-private group
    // counts as private, and the test host's group is not known.
    let e = env();
    let p = script(&e.home);
    for m in [0o702, 0o706, 0o777] {
        mode(&p, m);
        assert_eq!(statusline_script(&e.home), None, "{m:o}");
    }
}

#[test]
fn a_symlinked_script_is_refused() {
    let e = env();
    let p = script(&e.home);
    let real = e.home.join("real.sh");
    fs::rename(&p, &real).unwrap();
    std::os::unix::fs::symlink(&real, &p).unwrap();
    assert_eq!(statusline_script(&e.home), None);
}

#[test]
fn a_script_under_a_shared_directory_is_refused() {
    let e = env();
    script(&e.home);
    for d in [".amplihack/.claude/tools", ".amplihack"] {
        let dir = e.home.join(d);
        mode(&dir, 0o707);
        assert_eq!(statusline_script(&e.home), None, "{d}");
        mode(&dir, 0o700);
    }
    assert!(statusline_script(&e.home).is_some());
}

#[test]
fn a_directory_in_place_of_the_script_is_refused() {
    let e = env();
    let p = e.home.join(".amplihack/.claude/tools/statusline.sh");
    fs::create_dir_all(&p).unwrap();
    assert_eq!(statusline_script(&e.home), None);
}

// ---------------------------------------------------------------------------
// --root
// ---------------------------------------------------------------------------

#[test]
fn a_relative_root_is_no_root() {
    for rel in ["profiles", "./profiles", "", "../x"] {
        assert_eq!(root_for(Some(rel)), None, "{rel:?}");
    }
}

#[test]
fn an_absolute_root_is_used_as_given() {
    let e = env();
    assert_eq!(
        root_for(Some(e.root.to_str().unwrap())),
        Some(e.root.clone())
    );
}

// ---------------------------------------------------------------------------
// tick: the owner
// ---------------------------------------------------------------------------

#[test]
fn the_prototype_ledger_names_the_owner_and_the_reading_is_recorded() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let line = e.tick(&input(34.0, 43.0)).expect("a capacity line");
    assert_eq!(line, format!("{LINE_34_43}  · 0 spare"));
    let usage = e.usage().expect("the reading was recorded");
    let x = &usage["accounts"]["X"];
    assert_eq!(x["five_hour"]["used_percentage"], 34.0);
    assert_eq!(x["seven_day"]["used_percentage"], 43.0);
    assert_eq!(x["source"], "statusline");
    assert!(usage["accounts"].get("Y").is_none());
}

#[test]
fn the_rotate_ledger_names_the_owner() {
    let e = env();
    e.rotate_ledger("X");
    let line = e.tick(&input(34.0, 43.0)).expect("a capacity line");
    assert!(line.starts_with(LINE_34_43), "{line}");
    assert!(e.usage().unwrap()["accounts"].get("X").is_some());
}

#[test]
fn recording_does_not_depend_on_the_auto_switch_setting() {
    // Auto-switch is off by default and nothing here turns it on.
    let e = env();
    assert!(!super::config::effective(&e.root).on);
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    e.tick(&input(34.0, 43.0)).unwrap();
    assert!(e.usage().unwrap()["accounts"].get("X").is_some());
}

#[test]
fn without_a_ledger_there_is_no_line_and_no_state() {
    let e = env();
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
    assert!(!e.state_dir().exists(), "no-owner mode creates nothing");
}

#[test]
fn a_damaged_ledger_gives_no_line_and_no_state() {
    let e = env();
    e.prototype_ledger("not json");
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
    assert!(!e.state_dir().exists());
}

#[test]
fn an_untrusted_ledger_gives_no_line() {
    let e = env();
    let p = e.prototype_ledger(r#"{"live":"X","since":1}"#);
    mode(&p, 0o620);
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
}

#[test]
fn an_idle_ledger_gives_no_line() {
    let e = env();
    e.prototype_ledger(r#"{"live":null}"#);
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
}

#[test]
fn a_ledger_naming_a_profile_with_its_own_login_gives_no_line() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    private(&e.root.join("X/.credentials.json"), b"{}");
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
    assert!(e.usage().is_none(), "nothing recorded");
}

#[test]
fn without_a_login_in_the_default_directory_there_is_no_line() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    fs::remove_file(e.home.join(".claude/.credentials.json")).unwrap();
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
}

#[test]
fn a_rotate_ledger_whose_account_is_not_the_default_login_gives_no_line() {
    // `claude /login` run by hand in `~/.claude` as another account: the
    // rotate ledger is stale, so no owner and nothing recorded.
    let e = env();
    let account =
        serde_json::json!({"oauthAccount": {"accountUuid": "u1", "emailAddress": "me@x"}})
            .to_string();
    private(&e.home.join(".claude.json"), account.as_bytes());
    e.rotate_ledger_body(
        serde_json::json!({"version": 1, "live": "X", "account": "u2", "since": 10}),
    );
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
    assert!(e.usage().is_none(), "nothing recorded");

    e.rotate_ledger_body(
        serde_json::json!({"version": 1, "live": "X", "account": "u1", "since": 10}),
    );
    let line = e.tick(&input(34.0, 43.0)).expect("a capacity line");
    assert!(line.starts_with(LINE_34_43), "{line}");
    assert!(e.usage().unwrap()["accounts"].get("X").is_some());
}

#[test]
fn the_owner_comes_from_the_ledger_not_from_account_matching() {
    // A `.claude.json` in `~` and in profile Y that match: `profile usage`
    // would mark Y live without a ledger. The status line does not.
    let e = env();
    let account =
        serde_json::json!({"oauthAccount": {"accountUuid": "u1", "emailAddress": "me@x"}})
            .to_string();
    private(&e.home.join(".claude.json"), account.as_bytes());
    private(&e.root.join("Y/.claude.json"), account.as_bytes());
    assert_eq!(e.tick(&input(34.0, 43.0)), None);
}

// ---------------------------------------------------------------------------
// tick: input
// ---------------------------------------------------------------------------

#[test]
fn input_without_rate_limits_gives_no_line_and_records_nothing() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    for raw in [&b"{}"[..], b"", b"not json", br#"{"rate_limits":{}}"#] {
        assert_eq!(e.tick(raw), None, "{}", String::from_utf8_lossy(raw));
    }
    assert!(e.usage().is_none());
}

#[test]
fn input_over_one_mib_gives_no_line_and_records_nothing() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let mut raw = input(34.0, 43.0);
    raw.pop();
    raw.extend_from_slice(b",\"pad\":\"");
    raw.extend(std::iter::repeat_n(b'x', (1 << 20) + 1));
    raw.extend_from_slice(b"\"}");
    assert_eq!(e.tick(&raw), None);
    assert!(e.usage().is_none());
}

#[test]
fn an_over_limit_reading_shows_a_full_bar_at_one_hundred() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let line = e.tick(&input(130.0, 43.0)).unwrap();
    assert!(
        line.starts_with("🔑 X  5h ██████████ 100%  7d "),
        "the stdin path caps at 100: {line}"
    );
}

#[test]
fn a_single_window_is_rendered_alone() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let raw = serde_json::json!({"rate_limits": {
        "five_hour": {"used_percentage": 34.0, "resets_at": now() + HOUR}
    }})
    .to_string();
    assert_eq!(
        e.tick(raw.as_bytes()).as_deref(),
        Some("🔑 X  5h ███░░░░░░░ 34%  · 0 spare")
    );
}

// ---------------------------------------------------------------------------
// tick: the spare count
// ---------------------------------------------------------------------------

fn plant_usage(e: &Env, body: &serde_json::Value) -> PathBuf {
    let dir = e.state_dir();
    fs::create_dir_all(&dir).unwrap();
    mode(&dir, 0o700);
    let p = dir.join("usage.json");
    private(&p, body.to_string().as_bytes());
    p
}

fn usage_entry(five: f64, seven: f64) -> serde_json::Value {
    serde_json::json!({
        "five_hour": {"used_percentage": five, "resets_at": now() + HOUR},
        "seven_day": {"used_percentage": seven, "resets_at": now() + 3 * 86_400},
        "observed_at": now() - 60,
        "source": "statusline",
    })
}

#[test]
fn a_recorded_profile_with_room_is_spare() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"Y": usage_entry(20.0, 30.0)}}),
    );
    assert_eq!(
        e.tick(&input(34.0, 43.0)).as_deref(),
        Some(&*format!("{LINE_34_43}  · 1 spare"))
    );
}

#[test]
fn a_reading_for_a_removed_profile_is_not_spare() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {
            "Y": usage_entry(20.0, 30.0),
            "Gone": usage_entry(1.0, 1.0),
        }}),
    );
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 1 spare"));
}

#[test]
fn a_damaged_usage_file_drops_the_spare_segment_and_is_left_alone() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let p = plant_usage(&e, &serde_json::json!({"version": 1, "accounts": 5}));
    let before = fs::read(&p).unwrap();
    assert_eq!(e.tick(&input(34.0, 43.0)).as_deref(), Some(LINE_34_43));
    assert_eq!(
        fs::read(&p).unwrap(),
        before,
        "a damaged file is not overwritten"
    );
}

#[test]
fn a_traversal_key_in_usage_json_drops_the_spare_segment() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"../x": usage_entry(1.0, 1.0)}}),
    );
    assert_eq!(e.tick(&input(34.0, 43.0)).as_deref(), Some(LINE_34_43));
}

#[test]
fn a_held_lock_drops_the_spare_segment_and_records_nothing() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let state = State::new(&e.root).unwrap();
    state.ensure().unwrap();
    let _held = state.lock().unwrap();
    let start = Instant::now();
    assert_eq!(e.tick(&input(34.0, 43.0)).as_deref(), Some(LINE_34_43));
    assert!(start.elapsed() < Duration::from_millis(500));
    assert!(e.usage().is_none());
}

// ---------------------------------------------------------------------------
// tick: what is never written (S2)
// ---------------------------------------------------------------------------

fn snapshot(paths: &[PathBuf]) -> Vec<Option<Vec<u8>>> {
    paths.iter().map(|p| fs::read(p).ok()).collect()
}

#[test]
fn journal_ledgers_and_hosts_are_left_byte_identical() {
    let e = env();
    let proto = e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let rotate = e.rotate_ledger("X");
    let state = State::new(&e.root).unwrap();
    state.ensure().unwrap();
    let hosts = Ledger {
        version: 1,
        hosts: BTreeMap::from([(
            "Y".to_string(),
            HostEntry {
                live: "X".into(),
                swapped_at: now() - 10,
                pre_swap_fingerprint: None,
            },
        )]),
    };
    state.save_ledger(&hosts).unwrap();
    let journal = e.state_dir().join("journal.json");
    private(&journal, br#"{"left":"alone"}"#);
    let watched = [proto, rotate, e.state_dir().join("ledger.json"), journal];
    let before = snapshot(&watched);
    assert!(e.tick(&input(34.0, 43.0)).is_some());
    assert_eq!(snapshot(&watched), before);
}

#[test]
fn a_symlinked_root_is_refused_and_nothing_is_created() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let link = e.home.join("root-link");
    std::os::unix::fs::symlink(&e.root, &link).unwrap();
    assert_eq!(tick(&link, &e.home, &input(34.0, 43.0), now()), None);
    assert!(!e.state_dir().exists());
}

#[test]
fn a_shared_root_is_refused_and_nothing_is_created() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    mode(&e.root, 0o707);
    let got = e.tick(&input(34.0, 43.0));
    mode(&e.root, 0o700);
    assert_eq!(got, None);
    assert!(!e.state_dir().exists());
}

#[test]
fn a_missing_root_is_not_created() {
    let e = env();
    let missing = e.home.join("no-such-root");
    assert_eq!(tick(&missing, &e.home, &input(34.0, 43.0), now()), None);
    assert!(!missing.exists());
}

#[test]
fn the_line_has_no_control_characters() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let line = e.tick(&input(34.0, 43.0)).unwrap();
    assert!(!line.chars().any(char::is_control), "{line:?}");
    assert!(!line.contains(e.root.to_str().unwrap()), "no paths");
}

// ---------------------------------------------------------------------------
// Timing (D9)
// ---------------------------------------------------------------------------

#[test]
fn a_tick_takes_under_two_hundred_milliseconds() {
    let e = env();
    e.rotate_ledger("X");
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"Y": usage_entry(20.0, 30.0)}}),
    );
    let raw = input(34.0, 43.0);
    let start = Instant::now();
    let line = e.tick(&raw);
    let took = start.elapsed();
    assert!(line.is_some());
    assert!(took < Duration::from_millis(200), "{took:?}");
}
