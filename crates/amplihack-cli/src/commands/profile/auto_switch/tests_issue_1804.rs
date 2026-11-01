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
use super::per_token;
use super::rerun::{Pick, nested_candidate};
use super::state::{CREDS, Ledger, State, UsageFile};
use super::swap::swap_check;
use super::usage::{RateLimits, Reading, ReadingSource, Window};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry};
use crate::commands::profile::rotate::GateSource;
use crate::commands::profile::rotate::ledger::{RotateLedger, RotateState};
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

/// Every plan login at 100%, a far reset, and a marked key: the file reads
/// `ok` only while the move would really happen. The rotator refuses a key
/// that fails its login check, and `ok` would silence the no-capacity warning
/// at the moment nothing can run.
#[test]
fn a_marked_key_that_fails_its_login_check_never_makes_the_file_ok() {
    let _l = lock();
    let f = fx("enabled=on\noverflow=console\n");
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _unset = EnvGuard::unset([
        "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
        "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
    ]);
    let now = fsx::now();
    f.state
        .save_usage(&UsageFile {
            version: 1,
            accounts: used_up(now),
        })
        .unwrap();
    let readings: Vec<(ProfileName, capacity::Rotated)> = ["work", "spare", "cred"]
        .into_iter()
        .map(|p| (n(p), capacity::Rotated::Exhausted))
        .collect();
    let read = || {
        let at = chrono::DateTime::from_timestamp(now, 0).unwrap();
        cap::read_at(&f.root, at).map(|file| file.state)
    };

    capacity::refresh_from_rotation(&f.root, &readings, now, false, false, None);
    assert_eq!(read(), Some(Cap::Exhausted), "refused key, nothing can run");
    capacity::refresh_from_rotation(&f.root, &readings, now, false, true, None);
    assert_eq!(read(), Some(Cap::Ok), "the same state with a passing check");
    fs::remove_file(f.root.join("console").join(CREDS)).unwrap();
    capacity::refresh_from_rotation(&f.root, &readings, now, false, true, None);
    assert_eq!(read(), Some(Cap::Exhausted), "a key, but no login to move");
}

/// Each writer uses the check of whoever would make the move.
#[test]
fn the_capacity_check_follows_who_would_move_onto_the_key() {
    let f = fx("enabled=on\noverflow=console\n");
    let c = cfg(&["console"]);
    let inputs = f.inputs(&c, Some(GATE), 2_000_000_000);
    assert!(inputs.platform.is_some(), "console is the marked key");
    let home = f.home.as_path();
    let key = |path: &Path| {
        fs::write(path, r#"{"primaryApiKey":"k"}"#).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    };
    assert!(!inputs.key_counts(&f.root, Some(home), false), "no key");
    key(&f.root.join("console").join(".claude.json"));
    assert!(inputs.key_counts(&f.root, Some(home), false), "own files");
    assert!(!inputs.key_counts(&f.root, Some(home), true), "not in HOME");
    assert!(!inputs.key_counts(&f.root, None, true), "no HOME");
    key(&home.join(".claude.json"));
    assert!(inputs.key_counts(&f.root, Some(home), true), "key in HOME");
}

/// Selection and the rotator need a login file, so the capacity file must
/// not call a profile usable overflow without one: both writers, and the
/// rotator's live profile, whose file is in `~/.claude`, still counts.
#[test]
fn capacity_counts_no_overflow_profile_without_a_login_file() {
    let f = fx("enabled=on\noverflow=console\n");
    let c = cfg(&["console"]);
    let console = n("console");
    let now = 2_000_000_000;
    let blocked = |f: &Fx| {
        let mut inputs = f.inputs(&c, Some(GATE), now);
        inputs.refuse_no_login(&f.root);
        inputs.blocked.contains(&"console".to_string())
    };
    assert!(!blocked(&f), "its login file is there");
    assert!(super::can_take_login(&f.root, &console));

    fs::remove_file(f.root.join("console").join(CREDS)).unwrap();
    assert!(blocked(&f), "selection would pass it over");
    assert!(!super::can_take_login(&f.root, &console));

    let rs = RotateState::new(&f.root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some("console".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    assert!(!blocked(&f), "live: the file is in ~/.claude");
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
    assert!(inputs.check_note(&f.root, &n("console")).is_some());
    swap_check(&f.state, &f.profiles, &n("work"), &rl, &c).unwrap();
    assert!(
        f.state.load_ledger().unwrap().hosts.is_empty(),
        "no login moved"
    );
    assert_eq!(fs::read(f.root.join("console").join(CREDS)).unwrap(), b"{}");
}

// --- The per-token reason line -----------------------------------------------------------

/// The body of each `per-token` line in the rotate log.
fn per_token_lines(f: &Fx) -> Vec<String> {
    let log = RotateState::new(&f.root).log;
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' ').map(|(_, b)| b))
        .filter(|b| b.starts_with("per-token "))
        .map(str::to_string)
        .collect()
}

fn pick(f: &Fx, c: &Config, r: &BTreeMap<String, Reading>, current: &str) -> Option<Pick> {
    let skip = [current.to_string()];
    nested_candidate(
        &f.state,
        &Ledger::default(),
        &f.profiles,
        r,
        c,
        current,
        &skip,
    )
}

/// The `rotate::overflow_gate` variables, unset so that the default gate
/// applies.
fn default_gate() -> EnvGuard {
    EnvGuard::unset([
        "AMPLIHACK_ROTATE_OVERFLOW_WAIT",
        "AMPLIHACK_ROTATE_OVERFLOW_UNKNOWN_RESET",
        "AMPLIHACK_ROTATE_SESSION_AT",
        "AMPLIHACK_ROTATE_WEEK_AT",
    ])
}

#[test]
fn a_rerun_onto_credits_gives_its_reason_and_claims_no_probe() {
    let _l = lock();
    let f = fx("enabled=on\noverflow=cred:3\n");
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _gate = default_gate();
    let now = fsx::now();
    let mut c = cfg(&["cred"]);
    c.overflow[0].tier = 3;
    let mut r = used_up(now);
    r.remove("cred");
    let got = pick(&f, &c, &r, "work").expect("cred is the only way on");
    assert_eq!(got.profile, n("cred"));
    let line = got.per_token.expect("a move onto credits has a reason");
    assert!(
        line.starts_with(
            "per-token cred: no measured plan login below 100%, 0 not measured; earliest reset "
        ) && line.ends_with(
            " beyond overflow_wait; marked overflow tier 3; \
                 no spend block; auto-switch from work"
        ),
        "{line}"
    );
    assert!(!line.contains("probe"), "auto-switch runs no probe: {line}");
}

#[test]
fn a_rerun_onto_a_marked_key_gives_the_login_check_it_passed() {
    let _l = lock();
    let f = fx("enabled=on\noverflow=console\n");
    private(
        &f.root.join("console").join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _gate = default_gate();
    let now = fsx::now();
    let got = pick(&f, &cfg(&["console"]), &used_up(now), "work").expect("the key passes");
    assert_eq!(got.profile, n("console"));
    let line = got.per_token.expect("a move onto the key has a reason");
    assert!(
        line.contains("marked overflow tier 10")
            && line.contains("login check passed (primaryApiKey set)"),
        "{line}"
    );
    assert!(!line.contains("SENTINEL"), "{line}");
}

#[test]
fn a_rerun_off_the_key_says_it_passed_no_wait() {
    let _l = lock();
    let f = fx("enabled=on\noverflow=cred,console\n");
    private(
        &f.root.join("console").join(".claude.json"),
        r#"{"primaryApiKey":"SENTINEL-1804-KEY"}"#,
    );
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _gate = default_gate();
    let now = fsx::now();
    let mut r = used_up(now);
    r.remove("cred");
    let got = pick(&f, &cfg(&["cred", "console"]), &r, "console").expect("credits cost less");
    assert_eq!(got.profile, n("cred"));
    let line = got.per_token.expect("credits still bill per token");
    assert!(
        line.contains("; leaving the API key, credits cost less; ")
            && line.ends_with("; no spend block; auto-switch from console"),
        "{line}"
    );
}

#[test]
fn a_rerun_onto_a_plan_login_has_no_reason_line() {
    let _l = lock();
    let f = fx("enabled=on\noverflow=cred\n");
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _gate = default_gate();
    let now = fsx::now();
    let mut r = used_up(now);
    r.insert("spare".to_string(), reading(10.0, 4 * 3600, now));
    let got = pick(&f, &cfg(&["cred"]), &r, "work").expect("spare has room");
    assert_eq!(got.profile, n("spare"));
    assert_eq!(got.per_token, None);
}

#[test]
fn a_recorded_reason_is_in_the_rotate_log() {
    let f = fx("enabled=on\n");
    per_token::record(&f.root, Some("per-token cred: why"));
    per_token::record(&f.root, None);
    assert_eq!(per_token_lines(&f), ["per-token cred: why"]);
}

#[test]
fn swap_check_onto_credits_writes_the_reason_to_the_rotate_log() {
    let _l = lock();
    let f = fx("enabled=on\noverflow=cred\n");
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _gate = default_gate();
    let now = fsx::now();
    let mut accounts = used_up(now);
    accounts.remove("cred");
    f.state
        .save_usage(&UsageFile {
            version: 1,
            accounts,
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
    swap_check(&f.state, &f.profiles, &n("work"), &rl, &cfg(&["cred"])).unwrap();
    assert!(
        f.state.load_ledger().unwrap().hosts.contains_key("work"),
        "the login moved"
    );
    let lines = per_token_lines(&f);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with(
            "per-token cred: no measured plan login below 100%, 0 not measured; earliest reset "
        ) && lines[0].ends_with("; no spend block; auto-switch from work"),
        "{lines:?}"
    );
}

/// The rotator's capacity write takes the kinds from the home it was given,
/// never from `$HOME`: `ghost` has no login of its own, so the login in
/// `~/.claude` decides what it is.
#[test]
fn the_rotators_capacity_write_reads_the_home_it_is_given() {
    let _l = lock();
    let f = fx("enabled=on\n");
    let elsewhere = f.home.join("elsewhere");
    for h in [&f.home, &elsewhere] {
        fs::create_dir_all(h.join(".claude")).unwrap();
    }
    // `$HOME` is on an OAuth login, the given home on an API key.
    private(&elsewhere.join(".claude").join(CREDS), OAUTH);
    private(&f.home.join(".claude").join(CREDS), "{}");
    let _env = EnvGuard::set([("HOME", elsewhere.to_str().unwrap())]);
    let ghost = f.root.join("ghost");
    fs::create_dir(&ghost).unwrap();
    fs::set_permissions(&ghost, fs::Permissions::from_mode(0o700)).unwrap();
    let rs = RotateState::new(&f.root);
    rs.ensure().unwrap();
    rs.save_ledger(&RotateLedger {
        live: Some("ghost".into()),
        ..RotateLedger::default()
    })
    .unwrap();
    let readings: Vec<(ProfileName, capacity::Rotated)> = ["ghost", "work", "spare", "cred"]
        .into_iter()
        .map(|p| (n(p), capacity::Rotated::Exhausted))
        .collect();
    capacity::refresh_from_rotation(
        &f.root,
        &readings,
        fsx::now(),
        false,
        true,
        Some(f.home.as_path()),
    );
    let file = fs::read_to_string(f.root.join(".auto-switch.d/capacity.json")).unwrap();
    assert!(file.contains("\"work\""), "{file}");
    assert!(!file.contains("ghost"), "ghost is an API key here: {file}");
}
