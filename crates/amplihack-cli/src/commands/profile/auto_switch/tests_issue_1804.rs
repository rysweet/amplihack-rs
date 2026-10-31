//! Issue #1804: an API-key profile is never a plan login. Auto-switch reaches
//! one only through the API-key tier, so an unmarked key, a marked key the
//! tier does not use, and a key while kinds cannot be read are never picked
//! and never count as plan capacity. Every selection here goes through real
//! `OverflowInputs`, read from a real root, not a hand-built pool.

use super::candidate::{Pool, select_with_overflow};
use super::capacity::{self, Evidence, refresh};
use super::config::{self, Config};
use super::fsx;
use super::overflow_inputs::OverflowInputs;
use super::rerun::nested_candidate;
use super::state::{CREDS, Ledger, State, UsageFile};
use super::swap::swap_check;
use super::usage::{RateLimits, Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry};
use crate::commands::profile::rotate::GateSource;
use crate::test_support::{EnvGuard, env_lock};
use amplihack_utils::claude_capacity::{self as cap, State as Cap};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};
const OAUTH: &str = r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(|p| p.into_inner())
}

fn private(p: &Path, body: &str) {
    fs::write(p, body).unwrap();
    fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A private root: `work`, `spare` and `cred` hold OAuth logins; `console`
/// and `ignored` are API-key profiles with mode-600 `{}` logins, which pass
/// `fsx::movable_login` like the host's did on 30 October.
struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    state: State,
    profiles: Vec<ProfileName>,
}

fn fx(auto_switch: &str) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    fs::create_dir(&home).unwrap();
    let root = base.join("root");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let mut profiles = Vec::new();
    for (name, login) in [
        ("console", "{}"),
        ("cred", OAUTH),
        ("ignored", "{}"),
        ("spare", OAUTH),
        ("work", OAUTH),
    ] {
        let dir = root.join(name);
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        private(&dir.join(CREDS), login);
        profiles.push(n(name));
    }
    private(&root.join(".auto-switch"), auto_switch);
    let state = State::new(&root).unwrap();
    state.ensure().unwrap();
    Fx {
        _tmp: tmp,
        home,
        root,
        state,
        profiles,
    }
}

fn cfg(overflow: &[&str]) -> Config {
    Config {
        enabled: true,
        threshold: 90,
        exclude: Vec::new(),
        overflow: overflow
            .iter()
            .map(|p| OverflowEntry::new(n(p), 1))
            .collect(),
        kind: Default::default(),
    }
}

fn reading(pct: f64, resets_in: i64, now: i64) -> Reading {
    Reading {
        five_hour: Some(Window {
            used_percentage: pct,
            resets_at: now + resets_in,
        }),
        seven_day: Some(Window {
            used_percentage: 10.0,
            resets_at: now + 86_400,
        }),
        observed_at: now - 10,
        source: ReadingSource::Statusline,
    }
}

/// Every plan login at 100%, four hours from its reset.
fn used_up(now: i64) -> BTreeMap<String, Reading> {
    ["work", "spare", "cred"]
        .into_iter()
        .map(|p| (p.to_string(), reading(100.0, 4 * 3600, now)))
        .collect()
}

impl Fx {
    fn inputs(&self, c: &Config, gate: Option<Gate>, now: i64) -> OverflowInputs {
        let source = match gate {
            Some(_) => GateSource::Environment,
            None => GateSource::Closed("test".to_string()),
        };
        OverflowInputs::load_with_gate(&self.root, c, (gate, source), now)
    }

    /// What auto-switch picks from `current`, every login eligible.
    fn pick(
        &self,
        c: &Config,
        gate: Option<Gate>,
        readings: &BTreeMap<String, Reading>,
        current: &str,
        now: i64,
    ) -> Option<String> {
        let inputs = self.inputs(c, gate, now);
        let pool = Pool {
            profiles: &self.profiles,
            readings,
            threshold: c.threshold,
            exclude: &c.exclude,
            now,
        };
        select_with_overflow(&pool, &inputs.pool(), current, |_| true).map(|p| p.to_string())
    }
}

#[test]
fn an_unmarked_key_is_never_picked_with_nothing_marked() {
    let f = fx("enabled=on\n");
    let now = 2_000_000_000;
    let got = f.pick(&cfg(&[]), Some(GATE), &used_up(now), "work", now);
    assert_eq!(
        got, None,
        "every plan login is used up and nothing is marked"
    );
}

#[test]
fn an_unmarked_key_is_never_picked_with_a_credit_profile_marked() {
    let f = fx("enabled=on\noverflow=cred\n");
    let now = 2_000_000_000;
    let got = f.pick(&cfg(&["cred"]), Some(GATE), &used_up(now), "work", now);
    assert_eq!(got.as_deref(), Some("cred"), "not console, not ignored");
}

#[test]
fn an_unmarked_key_is_never_picked_with_the_gate_closed() {
    let f = fx("enabled=on\noverflow=cred\n");
    let now = 2_000_000_000;
    let got = f.pick(&cfg(&["cred"]), None, &used_up(now), "work", now);
    assert_eq!(got, None, "a closed gate leaves only plan logins");
}

#[test]
fn of_two_marked_keys_the_ignored_one_is_never_picked_and_the_first_waits_for_the_gate() {
    let f = fx("enabled=on\noverflow=console,ignored\n");
    let c = cfg(&["console", "ignored"]);
    let now = 2_000_000_000;
    let inputs = f.inputs(&c, Some(GATE), now);
    assert_eq!(inputs.order, vec![n("console")]);
    let got = f.pick(&c, Some(GATE), &used_up(now), "work", now);
    assert_eq!(got.as_deref(), Some("console"), "the gate allows it");
    let mut soon = used_up(now);
    soon.insert("spare".into(), reading(100.0, 600, now));
    assert_eq!(
        f.pick(&c, Some(GATE), &soon, "work", now),
        None,
        "a plan reset inside the wait keeps the first key behind the gate"
    );
    assert_eq!(f.pick(&c, None, &used_up(now), "work", now), None);
}

/// With every login used up, the key cannot be what makes the file `ok`.
#[test]
fn an_unmarked_key_never_makes_the_capacity_file_ok() {
    let f = fx("enabled=on\n");
    let c = cfg(&[]);
    let now = 2_000_000_000;
    let evidence: BTreeMap<String, Evidence> = used_up(now)
        .into_iter()
        .map(|(k, r)| (k, Evidence::Usage(r)))
        .collect();
    let inputs = f.inputs(&c, Some(GATE), now);
    let file = capacity::compute(&f.profiles, &evidence, &c, &inputs.pool(), now, "test");
    assert_eq!(file.state, Cap::Exhausted, "over: {:?}", file.over);
    assert_eq!(file.over, vec!["cred", "spare", "work"]);

    let mut low = evidence;
    low.insert(
        "spare".into(),
        Evidence::Usage(reading(95.0, 4 * 3600, now)),
    );
    let file = capacity::compute(&f.profiles, &low, &c, &inputs.pool(), now, "test");
    assert_eq!(file.state, Cap::Low, "over: {:?}", file.over);
}

/// `swap_check` needs the real `/proc` scan and the real gate.
#[test]
fn swap_check_moves_nothing_onto_a_key_with_a_mode_600_login() {
    let _l = lock();
    let f = fx("enabled=on\n");
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _unset = EnvGuard::unset([
        "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
        "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
    ]);
    let now = fsx::now();
    let before = fs::read(f.root.join("work").join(CREDS)).unwrap();
    f.state
        .save_usage(&UsageFile {
            version: 1,
            accounts: used_up(now),
        })
        .unwrap();
    let window = Some(Window {
        used_percentage: 100.0,
        resets_at: now + 4 * 3600,
    });
    let rl = RateLimits {
        five_hour: window,
        seven_day: window,
    };
    swap_check(&f.state, &f.profiles, &n("work"), &rl, &cfg(&[])).unwrap();
    assert!(
        f.state.load_ledger().unwrap().hosts.is_empty(),
        "no login moved"
    );
    assert_eq!(fs::read(f.root.join("work").join(CREDS)).unwrap(), before);
    assert_eq!(fs::read(f.root.join("console").join(CREDS)).unwrap(), b"{}");
}

#[test]
fn nested_candidate_never_returns_a_key() {
    let _l = lock();
    let f = fx("enabled=on\n");
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let now = fsx::now();
    let got = nested_candidate(
        &f.state,
        &Ledger::default(),
        &f.profiles,
        &used_up(now),
        &cfg(&[]),
        "work",
        &["work".to_string()],
    );
    assert_eq!(got, None);
}

/// With the env toggle on, a corrupt `.auto-switch` leaves auto-switch on
/// with default settings, so kinds really can be unknown here.
#[test]
fn corrupt_kinds_select_nothing_and_write_no_capacity_file() {
    let _l = lock();
    let f = fx("this is not a setting\n");
    let _on = EnvGuard::set([
        (config::TOGGLE_ENV, "1"),
        ("HOME", f.home.to_str().unwrap()),
    ]);
    let now = fsx::now();
    let mut readings = used_up(now);
    let c = Config::default();
    assert_eq!(f.pick(&c, Some(GATE), &readings, "work", now), None);
    readings.insert("spare".into(), reading(10.0, 4 * 3600, now));
    assert_eq!(
        f.pick(&c, Some(GATE), &readings, "work", now),
        None,
        "not even a plan login, since any of them may be a key"
    );
    let usage = UsageFile {
        version: 1,
        accounts: used_up(now),
    };
    refresh(&f.state, &usage, &Ledger::default(), "test");
    assert!(!cap::path(&f.root).exists(), "no capacity file");
}

/// A session on an unmarked key is not overflow, so it is treated as a plan
/// session: it goes to a plan login with capacity, never to another key,
/// marked or not, and with no way off it stays on the key. Nothing else
/// moves it: the status line sees no rate limits on a key, and the pre-check
/// does not count it as overflow, so it stays until the user moves it.
#[test]
fn a_session_on_an_unmarked_key_never_moves_onto_another_key() {
    let f = fx("enabled=on\noverflow=console\n");
    let now = 2_000_000_000;
    let got = f.pick(
        &cfg(&["console"]),
        Some(GATE),
        &used_up(now),
        "ignored",
        now,
    );
    assert_eq!(got, None, "console is a key, marked or not");
    let g = fx("enabled=on\n");
    let got = g.pick(&cfg(&[]), Some(GATE), &used_up(now), "console", now);
    assert_eq!(got, None, "ignored is unmarked too");
    let mut readings = used_up(now);
    readings.insert("spare".into(), reading(10.0, 4 * 3600, now));
    let got = g.pick(&cfg(&[]), Some(GATE), &readings, "console", now);
    assert_eq!(got.as_deref(), Some("spare"), "a plan login is a way off");
}

/// A marked key is still never swapped into a running session: its
/// `{}` login is no login on its own, so the session would be logged out.
#[test]
fn swap_check_never_swaps_a_marked_key_into_a_session() {
    let _l = lock();
    let f = fx("enabled=on\noverflow=console\n");
    // A key the login check accepts, so only the swap rule can refuse.
    private(
        &f.root.join("console").join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _unset = EnvGuard::unset([
        "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
        "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
    ]);
    let now = fsx::now();
    let c = cfg(&["console"]);
    assert_eq!(
        f.pick(&c, Some(GATE), &used_up(now), "work", now)
            .as_deref(),
        Some("console"),
        "the selection itself reaches the key"
    );
    f.state
        .save_usage(&UsageFile {
            version: 1,
            accounts: used_up(now),
        })
        .unwrap();
    let window = Some(Window {
        used_percentage: 100.0,
        resets_at: now + 4 * 3600,
    });
    let rl = RateLimits {
        five_hour: window,
        seven_day: window,
    };
    let inputs = OverflowInputs::load(&f.root, &c, now);
    assert!(inputs.gate.is_some(), "the gate must be open for this test");
    assert!(inputs.key_check_passes(&f.root, &n("console")));
    swap_check(&f.state, &f.profiles, &n("work"), &rl, &c).unwrap();
    assert!(
        f.state.load_ledger().unwrap().hosts.is_empty(),
        "no login moved"
    );
    assert_eq!(fs::read(f.root.join("console").join(CREDS)).unwrap(), b"{}");
}
