//! `profile statusline` without `--owner` (issue #1745): the command in
//! `~/.claude/settings.json`. The mode rule, the `statusline.sh` checks, the
//! root rule, and `tick`: the login from the rotator ledgers only, the
//! reading recorded only when it can be trusted, and the capacity line.

use super::home_line::{Mode, mode as select_mode, root_for, statusline_script, tick};
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
/// The bars alone, when no login can be resolved.
const BARS: &str = "5h ███░░░░░░░ 34%  7d ████░░░░░░ 43%";

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

    /// The readings home mode records, in `home-usage.json`.
    fn usage(&self) -> Option<serde_json::Value> {
        let raw = fs::read(self.state_dir().join("home-usage.json")).ok()?;
        Some(serde_json::from_slice(&raw).unwrap())
    }

    fn tick(&self, input: &[u8]) -> Option<String> {
        tick(Some(&self.root), input, now())
    }
}

// ---------------------------------------------------------------------------
// mode (D3)
// ---------------------------------------------------------------------------

impl Env {
    fn mode(&self, dir: Option<&OsStr>) -> Mode {
        select_mode(dir, Some(&self.home), Some(&self.root))
    }

    fn owned(&self, owner: &str) -> Mode {
        Mode::Owned {
            root: self.root.clone(),
            owner: owner.into(),
        }
    }
}

#[test]
fn unset_or_empty_config_dir_is_home_mode() {
    let e = env();
    assert_eq!(e.mode(None), Mode::Home);
    assert_eq!(e.mode(Some(OsStr::new(""))), Mode::Home);
}

#[test]
fn the_default_directory_is_home_mode() {
    let e = env();
    let dot = e.home.join(".claude");
    assert_eq!(e.mode(Some(dot.as_os_str())), Mode::Home);
    let slash = format!("{}/", dot.display());
    assert_eq!(
        e.mode(Some(OsStr::new(&slash))),
        Mode::Home,
        "a trailing slash is ignored"
    );
    let link = e.home.join("claude-link");
    std::os::unix::fs::symlink(&dot, &link).unwrap();
    assert_eq!(e.mode(Some(link.as_os_str())), Mode::Home);
}

#[test]
fn a_profile_directory_is_that_profiles_run() {
    let e = env();
    let x = e.root.join("X");
    assert_eq!(e.mode(Some(x.as_os_str())), e.owned("X"));
    let slash = format!("{}/", x.display());
    assert_eq!(e.mode(Some(OsStr::new(&slash))), e.owned("X"));
    // A symlink from elsewhere that resolves into the root is the profile.
    let link = e.home.join("x-link");
    std::os::unix::fs::symlink(&x, &link).unwrap();
    assert_eq!(e.mode(Some(link.as_os_str())), e.owned("X"));
}

#[test]
fn a_symlink_in_the_root_that_points_outside_it_is_passthrough() {
    let e = env();
    let outside = e.home.join("elsewhere/Z");
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, e.root.join("Z")).unwrap();
    assert_eq!(
        e.mode(Some(e.root.join("Z").as_os_str())),
        Mode::Passthrough
    );
}

#[test]
fn anything_else_is_passthrough() {
    let e = env();
    let other = e.home.join("other");
    fs::create_dir(&other).unwrap();
    let state = e.root.join(".auto-switch.d");
    fs::create_dir(&state).unwrap();
    let nested = e.root.join("X/sub");
    fs::create_dir(&nested).unwrap();
    for dir in [
        other,
        state,
        nested,
        e.root.clone(),
        e.root.join("Missing"),
        e.home.clone(),
    ] {
        assert_eq!(
            e.mode(Some(dir.as_os_str())),
            Mode::Passthrough,
            "{}",
            dir.display()
        );
    }
    let x = e.root.join("X");
    assert_eq!(
        select_mode(Some(x.as_os_str()), Some(&e.home), None),
        Mode::Passthrough,
        "no root, no profile"
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
    // No auto-switch settings file, so it is off (the environment toggle
    // is not read here: other tests set it).
    let e = env();
    assert!(matches!(
        super::config::read(&e.root),
        super::config::FileState::Missing
    ));
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    e.tick(&input(34.0, 43.0)).unwrap();
    assert!(e.usage().unwrap()["accounts"].get("X").is_some());
}

#[test]
fn without_a_ledger_there_is_no_login_and_no_state() {
    let e = env();
    assert_eq!(
        e.tick(&input(34.0, 43.0)).as_deref(),
        Some(&*format!("{BARS}  · 0 spare"))
    );
    assert!(!e.state_dir().exists(), "home mode creates nothing");
}

#[test]
fn a_damaged_ledger_gives_no_login_and_no_state() {
    let e = env();
    e.prototype_ledger("not json");
    assert_eq!(
        e.tick(&input(34.0, 43.0)).as_deref(),
        Some(&*format!("{BARS}  · 0 spare"))
    );
    assert!(!e.state_dir().exists());
}

#[test]
fn an_untrusted_ledger_gives_no_login() {
    let e = env();
    let p = e.prototype_ledger(r#"{"live":"X","since":1}"#);
    mode(&p, 0o620);
    assert!(e.tick(&input(34.0, 43.0)).unwrap().starts_with(BARS));
}

#[test]
fn an_idle_ledger_gives_no_login() {
    let e = env();
    e.prototype_ledger(r#"{"live":null}"#);
    assert!(e.tick(&input(34.0, 43.0)).unwrap().starts_with(BARS));
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
    assert!(e.tick(&input(34.0, 43.0)).unwrap().starts_with(BARS));
}

#[test]
fn a_ledger_naming_a_profile_without_a_directory_gives_no_line() {
    let e = env();
    e.prototype_ledger(r#"{"live":"Nobody","since":1}"#);
    assert!(e.tick(&input(34.0, 43.0)).unwrap().starts_with(BARS));
    // A symlink to a profile is not the profile's own directory.
    std::os::unix::fs::symlink(e.root.join("X"), e.root.join("Nobody")).unwrap();
    assert!(e.tick(&input(34.0, 43.0)).unwrap().starts_with(BARS));
    assert!(e.usage().is_none(), "nothing recorded");
}

#[test]
fn a_profile_with_its_own_login_is_not_checked_for_one() {
    // SR1: the status line never stats a `.credentials.json`.
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    private(&e.root.join("X/.credentials.json"), b"{}");
    assert!(e.tick(&input(34.0, 43.0)).is_some());
}

// ---------------------------------------------------------------------------
// tick: when a reading is recorded (D2)
// ---------------------------------------------------------------------------

fn save_hosts(e: &Env, dir: &str, live: &str, swapped_at: i64) {
    let state = State::new(&e.root).unwrap();
    state.ensure().unwrap();
    state
        .save_ledger(&Ledger {
            version: 1,
            hosts: BTreeMap::from([(
                dir.to_string(),
                HostEntry {
                    live: live.into(),
                    swapped_at,
                    pre_swap_fingerprint: None,
                },
            )]),
        })
        .unwrap();
}

#[test]
fn nothing_is_recorded_within_the_cooldown_after_the_ledger_changed() {
    let e = env();
    let since = now() - super::usage::COOLDOWN_SECS + 30;
    e.prototype_ledger(&format!(r#"{{"live":"X","since":{since}}}"#));
    let line = e
        .tick(&input(34.0, 43.0))
        .expect("the line is still printed");
    assert!(line.starts_with(LINE_34_43), "{line}");
    assert!(e.usage().is_none(), "nothing recorded");

    let since = now() - super::usage::COOLDOWN_SECS - 1;
    e.prototype_ledger(&format!(r#"{{"live":"X","since":{since}}}"#));
    e.tick(&input(34.0, 43.0)).unwrap();
    assert!(e.usage().unwrap()["accounts"].get("X").is_some());
}

#[test]
fn nothing_is_recorded_without_a_since() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X"}"#);
    let line = e
        .tick(&input(34.0, 43.0))
        .expect("the line is still printed");
    assert!(line.starts_with(LINE_34_43), "{line}");
    assert!(e.usage().is_none());
}

#[test]
fn nothing_is_recorded_when_attribution_ignores_the_reading() {
    // An auto-switch swap into X's directory moments ago.
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    save_hosts(&e, "X", "Y", now() - 10);
    assert!(e.tick(&input(34.0, 43.0)).is_some());
    assert!(e.usage().is_none(), "nothing recorded");
}

#[test]
fn the_reading_is_stored_under_the_attributed_account() {
    // Auto-switch has Y's login in X's directory: the reading is Y's.
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    save_hosts(&e, "X", "Y", now() - 3600);
    e.tick(&input(34.0, 43.0)).unwrap();
    let usage = e.usage().unwrap();
    assert!(usage["accounts"].get("Y").is_some(), "{usage}");
    assert!(usage["accounts"].get("X").is_none(), "{usage}");
}

#[test]
fn nothing_is_recorded_under_a_group_or_world_writable_root() {
    for m in [0o770, 0o707, 0o777] {
        let e = env();
        e.prototype_ledger(r#"{"live":"X","since":1}"#);
        mode(&e.root, m);
        // A group holding only the user counts as private (as for the
        // ledger), so a group-writable root is refused only on hosts where
        // the group has other members.
        let refused = super::fsx::check_root_private(&e.root).is_err();
        let got = e.tick(&input(34.0, 43.0));
        mode(&e.root, 0o700);
        assert!(refused || m == 0o770, "{m:o} must be refused");
        if refused {
            assert_eq!(got.as_deref(), Some(BARS), "{m:o}");
            assert!(!e.state_dir().exists(), "{m:o}");
        }
    }
}

#[test]
fn a_failed_save_prints_no_spare_count() {
    if super::fsx::euid() == 0 {
        return;
    }
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"Y": usage_entry(20.0, 30.0)}}),
    );
    // The lock file exists, but nothing new can be written in the directory.
    drop(State::new(&e.root).unwrap().lock().unwrap());
    mode(&e.state_dir(), 0o500);
    let got = e.tick(&input(34.0, 43.0));
    mode(&e.state_dir(), 0o700);
    assert_eq!(
        got.as_deref(),
        Some(LINE_34_43),
        "no count from unsaved data"
    );
}

// ---------------------------------------------------------------------------
// tick: input
// ---------------------------------------------------------------------------

#[test]
fn input_without_rate_limits_gives_no_bars_and_records_nothing() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    for raw in [&b"{}"[..], b"", b"not json", br#"{"rate_limits":{}}"#] {
        assert_eq!(
            e.tick(raw).as_deref(),
            Some("🔑 X  · 0 spare"),
            "{}",
            String::from_utf8_lossy(raw)
        );
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
    assert_eq!(e.tick(&raw).as_deref(), Some("🔑 X  · 0 spare"));
    assert!(e.usage().is_none());
}

#[test]
fn an_over_limit_reading_shows_a_full_bar_at_one_hundred() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let line = e.tick(&input(130.0, 43.0)).unwrap();
    assert!(
        line.starts_with("🔑 X  5h ██████████ 100%  7d "),
        "the input's bounds check caps the value at 100: {line}"
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

fn toggle(e: &Env, body: &str) {
    private(&e.root.join(".auto-switch"), body.as_bytes());
}

fn add(e: &Env, names: &[&str]) {
    let store = ProfileStore::new(e.root.clone());
    for n in names {
        store.add(&ProfileName::parse(n).unwrap()).unwrap();
    }
}

#[test]
fn excluded_and_overflow_profiles_are_not_spare() {
    let e = env();
    add(&e, &["Z", "W"]);
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {
            "Y": usage_entry(20.0, 30.0),
            "Z": usage_entry(20.0, 30.0),
            "W": usage_entry(20.0, 30.0),
        }}),
    );
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 3 spare"));
    toggle(&e, "enabled=off\nexclude=Z\noverflow=W\n");
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 1 spare"));
}

#[test]
fn a_damaged_auto_switch_file_drops_the_spare_segment() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    toggle(&e, "not a toggle file");
    assert_eq!(e.tick(&input(34.0, 43.0)).as_deref(), Some(LINE_34_43));
}

#[test]
fn an_exhausted_login_is_not_spare() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let mut y = usage_entry(20.0, 30.0);
    y["source"] = "usage_limit_failure".into();
    plant_usage(&e, &serde_json::json!({"version": 1, "accounts": {"Y": y}}));
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 0 spare"));
}

/// The #1846 case: a login hit its weekly limit with its weekly window at
/// 62%. A later `~/.claude` reading of 30/62 must not make it spare, on the
/// recording path or the one without the lock, until the windows reset.
#[test]
fn a_later_home_reading_does_not_hide_a_usage_limit_failure() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let mut y = usage_entry(20.0, 62.0);
    y["source"] = "usage_limit_failure".into();
    plant_usage(&e, &serde_json::json!({"version": 1, "accounts": {"Y": y}}));
    let mut later = usage_entry(30.0, 62.0);
    later["observed_at"] = now().into();
    plant_home_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"Y": later}}),
    );
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 0 spare"));
    assert!(e.tick(b"{}").unwrap().ends_with("· 0 spare"));

    y["five_hour"]["resets_at"] = (now() - 1).into();
    y["seven_day"]["resets_at"] = (now() - 1).into();
    plant_usage(&e, &serde_json::json!({"version": 1, "accounts": {"Y": y}}));
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 1 spare"));
}

#[test]
fn a_weekly_only_reading_can_be_spare() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let y = serde_json::json!({
        "seven_day": {"used_percentage": 30.0, "resets_at": now() + 3 * 86_400},
        "observed_at": now() - 60,
        "source": "statusline",
    });
    plant_usage(&e, &serde_json::json!({"version": 1, "accounts": {"Y": y}}));
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 1 spare"));
}

#[test]
fn a_candidate_is_looked_up_under_the_account_auto_switch_has_live_in_it() {
    // Y's directory holds Z's login: Y is spare by Z's reading.
    let e = env();
    add(&e, &["Z"]);
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {
            "Y": usage_entry(95.0, 95.0),
            "Z": usage_entry(20.0, 30.0),
        }}),
    );
    save_hosts(&e, "Y", "Z", now() - 3600);
    // Y (by Z's reading) and Z itself.
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 2 spare"));
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
fn a_held_lock_records_nothing() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let state = State::new(&e.root).unwrap();
    state.ensure().unwrap();
    let _held = state.lock().unwrap();
    let start = Instant::now();
    assert_eq!(
        e.tick(&input(34.0, 43.0)).as_deref(),
        Some(&*format!("{LINE_34_43}  · 0 spare")),
        "the count is read without the lock"
    );
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
    assert_eq!(
        tick(Some(&link), &input(34.0, 43.0), now()).as_deref(),
        Some(BARS),
        "bars only"
    );
    assert!(!e.state_dir().exists());
}

#[test]
fn a_shared_root_is_refused_and_nothing_is_created() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    mode(&e.root, 0o707);
    let got = e.tick(&input(34.0, 43.0));
    mode(&e.root, 0o700);
    assert_eq!(got.as_deref(), Some(BARS), "bars only");
    assert!(!e.state_dir().exists());
}

#[test]
fn a_missing_root_is_not_created() {
    let e = env();
    let missing = e.home.join("no-such-root");
    assert_eq!(
        tick(Some(&missing), &input(34.0, 43.0), now()).as_deref(),
        Some(BARS)
    );
    assert_eq!(tick(None, &input(34.0, 43.0), now()).as_deref(), Some(BARS));
    assert_eq!(tick(None, b"{}", now()), None, "no segment, no line");
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
// Timing (AC10)
// ---------------------------------------------------------------------------

#[test]
fn the_capacity_work_takes_under_two_hundred_milliseconds() {
    // 22 profiles, every one with a reading, and a `~/.claude.json` of about
    // 1 MB that nothing on this path may read.
    let e = env();
    let names: Vec<String> = (0..20).map(|i| format!("p{i:02}")).collect();
    add(&e, &names.iter().map(String::as_str).collect::<Vec<_>>());
    let pad = "x".repeat(1 << 20);
    let claude_json = serde_json::json!({
        "oauthAccount": {"accountUuid": "u1", "emailAddress": "me@x"},
        "projects": {"pad": pad},
    });
    private(
        &e.home.join(".claude.json"),
        claude_json.to_string().as_bytes(),
    );
    e.rotate_ledger_body(serde_json::json!({
        "version": 1, "live": "X", "account": "u1", "since": 10,
    }));
    let mut accounts = serde_json::Map::new();
    for n in names.iter().map(String::as_str).chain(["Y"]) {
        accounts.insert(n.to_string(), usage_entry(20.0, 30.0));
    }
    plant_usage(&e, &serde_json::json!({"version": 1, "accounts": accounts}));
    let raw = input(34.0, 43.0);

    // The budget is checked against the fastest of several runs, not a single
    // sample. One run on a loaded host (a parallel debug `cargo test
    // --workspace`, other builds at nice 15 / ionice idle) can be delayed by
    // scheduling or by the fsync of `home-usage.json`, which says nothing about
    // the code. The fastest run is the cost of the work itself: if even that
    // exceeds the budget, the path is too slow. Every run's output is checked,
    // so repetition does not weaken what the test proves about the line.
    let state = State::new(&e.root).unwrap();
    let owner = ProfileName::parse("Y").unwrap();
    let expected_home = format!("{LINE_34_43}  · 21 spare");
    let home = fastest_of(TIMING_RUNS, || {
        let line = e.tick(&raw);
        assert_eq!(line.as_deref(), Some(&*expected_home));
    });
    assert!(home < Duration::from_millis(200), "home mode: {home:?}");

    let owned = fastest_of(TIMING_RUNS, || {
        let line = super::home_line::owned_line(&state, &owner, &raw, now());
        assert!(line.is_some_and(|l| l.ends_with("· 21 spare")));
    });
    assert!(
        owned < Duration::from_millis(200),
        "profile mode: {owned:?}"
    );
}

/// Runs for the timing check in
/// `the_capacity_work_takes_under_two_hundred_milliseconds`.
const TIMING_RUNS: usize = 5;

/// The shortest wall-clock time of `runs` calls to `f`.
fn fastest_of(runs: usize, mut f: impl FnMut()) -> Duration {
    (0..runs)
        .map(|_| {
            let start = Instant::now();
            f();
            start.elapsed()
        })
        .min()
        .expect("at least one run")
}

// ---------------------------------------------------------------------------
// tick: readings from `~/.claude` never steer a decision (crusty round 3)
// ---------------------------------------------------------------------------

fn plant_home_usage(e: &Env, body: &serde_json::Value) -> PathBuf {
    let dir = e.state_dir();
    fs::create_dir_all(&dir).unwrap();
    mode(&dir, 0o700);
    let p = dir.join("home-usage.json");
    private(&p, body.to_string().as_bytes());
    p
}

#[test]
fn a_recorded_reading_leaves_usage_json_byte_identical() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let p = plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"X": usage_entry(5.0, 6.0)}}),
    );
    let before = fs::read(&p).unwrap();
    e.tick(&input(34.0, 43.0)).unwrap();
    assert_eq!(fs::read(&p).unwrap(), before, "usage.json was written");
    let home = e.usage().expect("recorded in home-usage.json");
    assert_eq!(home["accounts"]["X"]["five_hour"]["used_percentage"], 34.0);
}

#[test]
fn a_recorded_reading_never_creates_usage_json() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    e.tick(&input(34.0, 43.0)).unwrap();
    assert!(e.usage().is_some());
    assert!(!e.state_dir().join("usage.json").exists());
}

/// The case crusty raised: `~/.claude` holds some other account (a hand-run
/// `claude /login`, or a session that kept its old token past a swap), and
/// auto-switch has no `hosts` entry for the ledger's login, so attribution
/// files the reading under that login. Nothing in home mode can tell. The
/// passive reading that `profile usage` and the rotator's `measure` take
/// (`usage_passive::fresh` → `fresh_reading`) must not use it.
#[test]
fn a_misfiled_home_reading_is_never_a_passive_reading() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    // The figures are another account's; home mode files them under X.
    e.tick(&input(99.0, 99.0)).unwrap();
    let home = e.usage().expect("the reading was recorded");
    assert_eq!(home["accounts"]["X"]["five_hour"]["used_percentage"], 99.0);
    assert_eq!(
        super::fresh_reading(&e.root, "X", 3600, now()),
        None,
        "a home reading stood in for a measurement of X"
    );
    // A checked reading in usage.json is still used, and the home reading,
    // although newer, does not replace it.
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"X": usage_entry(5.0, 6.0)}}),
    );
    e.tick(&input(99.0, 99.0)).unwrap();
    let fresh = super::fresh_reading(&e.root, "X", 3600, now()).expect("the checked reading");
    assert_eq!(fresh.session.unwrap().used_percentage, 5.0);
    assert_eq!(fresh.week.unwrap().used_percentage, 6.0);
}

#[test]
fn a_home_reading_counts_toward_the_spare_count() {
    // Y was the live login and its reading was taken in `~/.claude`; now X
    // is live and Y is a candidate.
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    plant_home_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"Y": usage_entry(20.0, 30.0)}}),
    );
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 1 spare"));
}

#[test]
fn the_newer_of_the_two_readings_decides_the_spare_count() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let mut older = usage_entry(20.0, 30.0);
    older["observed_at"] = (now() - 600).into();
    let newer = usage_entry(100.0, 100.0);
    plant_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"Y": older}}),
    );
    plant_home_usage(
        &e,
        &serde_json::json!({"version": 1, "accounts": {"Y": newer}}),
    );
    assert!(e.tick(&input(34.0, 43.0)).unwrap().ends_with("· 0 spare"));
}

#[test]
fn a_damaged_home_usage_file_drops_the_spare_segment_and_is_left_alone() {
    let e = env();
    e.prototype_ledger(r#"{"live":"X","since":1}"#);
    let p = plant_home_usage(&e, &serde_json::json!({"version": 1, "accounts": 5}));
    let before = fs::read(&p).unwrap();
    assert_eq!(e.tick(&input(34.0, 43.0)).as_deref(), Some(LINE_34_43));
    assert_eq!(
        fs::read(&p).unwrap(),
        before,
        "a damaged file is not overwritten"
    );
}
