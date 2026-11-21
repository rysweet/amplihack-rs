//! Issue #2885 (and #2573, item 2): auto-switch asks the same question as the
//! launch guard when it decides whether a profile holds a login of its own.
//! Before, each site treated every macOS profile as holding one, because a
//! Keychain login leaves no file to find. See "Keychain logins" and
//! `auto-switch status` in docs/reference/profile-command.md.
//!
//! Every test names the platform through `keychain::test_seam`; a test build
//! otherwise reads as not macOS, so none starts the system `security`. The
//! fake `security` answers by service name.
//!
//! Contract (`auto_switch::own_login`):
//! - `OwnLogin::{File, Keychain, KeyProfile, KeychainUnreadable(Cause),
//!   None}`.
//! - `OwnLogins::new(&ProfileStore, home: Option<&Path>)`;
//!   `of(&self, &ProfileName) -> OwnLogin`, memoised per name;
//!   `unreadable(&self, &ProfileName) -> Option<Cause>` is `Some` exactly
//!   when `of` is `KeychainUnreadable`.
//!   - `File`: `<root>/<name>/.credentials.json` exists. The Keychain is not
//!     asked, and off macOS nothing else is.
//!   - Without a file, on macOS only, one `Ctx` resolution decides: a
//!     present Keychain item is `Keychain`; an unreadable Keychain is
//!     `KeychainUnreadable`; an API-key profile is `KeyProfile` (it holds
//!     its login outside the profile directory, so it still counts for
//!     selection); anything else is `None`.
//! - `unreadable_note(&ProfileName, Cause) -> String`: one line that starts
//!   with `auto-switch: `, names the profile and the cause and says the
//!   Keychain could not be read.
//! - `cli::login_place(&Ledger, &OwnLogins, &ProfileName) -> String` (now
//!   `pub(super)`; it no longer takes the `State`, which it used only to
//!   find the login file): `live in <dir>`, `parked`, then `home` (`File`),
//!   `keychain`, `keychain unreadable`, `none` (`KeyProfile` and `None`).
//!   A leased or parked profile is not asked about the Keychain.
//! - `rerun::login_ready(&State, &OverflowInputs, &OwnLogins, &ProfileName)`
//!   is true for `File`, `Keychain` and `KeyProfile` (plus the existing
//!   `check_note`), false for `KeychainUnreadable` and `None`.
//! - `overflow_inputs::login_gap(root, name) -> Option<LoginGap>`,
//!   `refuse_no_login(root)` and `takeover::could_take_over_at(store, own,
//!   home)` use the same rule. `could_take_over_at` hands its `home` to the
//!   lookup; the others use the process home.
//! - The reason is never dropped on the way (crusty round 1 of PR #2891).
//!   `LoginGap::{NoLogin, KeychainUnreadable(Cause)}` keeps a Keychain that
//!   could not be read apart from no login. `LoginGap::overflow_warning`
//!   is what `profile overflow add` prints, with one text for each.
//!   `refuse_no_login` returns `Vec<(ProfileName, Cause)>`: the profiles it
//!   blocked because of the Keychain. `capacity::refresh` and
//!   `capacity::refresh_from_rotation` return one `unreadable_note` line for
//!   each of those that is not excluded, for their callers to print or log.
//! - `no_login_of` asks the platform from `Ctx::macos()` and asks no
//!   Keychain question: on macOS it lists nothing from the rotator's login
//!   tests, as before, so no Keychain answer can release the hold on a key.

use super::capacity::{self, Rotated};
use super::cli::login_place;
use super::config::Config;
use super::fsx;
use super::overflow_inputs::{LoginGap, OverflowInputs, login_gap};
use super::own_login::{OwnLogin, OwnLogins, unreadable_note};
use super::rerun::{login_ready, nested_candidate};
use super::state::{CREDS, HostEntry, Ledger, State};
use super::takeover::could_take_over_at;
use super::usage::{Reading, ReadingSource, Window};
use crate::commands::profile::keychain::service_name;
use crate::commands::profile::keychain::{Cause, test_seam};
use crate::commands::profile::kind::Ctx;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::overflow::{Gate, OverflowEntry};
use crate::commands::profile::rotate::GateSource;
use crate::commands::profile::rotate::ledger::{RotateLedger, RotateState};
use crate::commands::profile::store::ProfileStore;
use crate::test_support::{EnvGuard, env_lock, write_executable};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const ACCOUNT: &str = r#"{"oauthAccount":{"accountUuid":"uuid-SENTINEL-KEYCHAIN","emailAddress":"user-SENTINEL-KEYCHAIN@example.com"}}"#;
const OAUTH: &str = r#"{"claudeAiOauth":{"accessToken":"SENTINEL"}}"#;
const GATE: Gate = Gate {
    wait: 1800,
    unknown_reset: false,
    session_at: 85,
    week_at: 90,
};

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    env_lock().lock().unwrap_or_else(|p| p.into_inner())
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// What a profile directory holds.
#[derive(Clone, Copy)]
enum P {
    /// An OAuth login file and an account.
    File,
    /// A `{}` login file: an API-key profile.
    Console,
    /// An account and no login file; the fake answers with this exit status.
    Acct(i32),
    /// As `Acct`, and an API-key setting in `settings.json`.
    AcctKey(i32),
    /// No file and no account.
    Bare,
    /// No file, no account, an API-key setting.
    BareKey,
}

struct Fx {
    _t: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    store: ProfileStore,
    bin: PathBuf,
    log: PathBuf,
    profiles: Vec<ProfileName>,
}

fn fx(spec: &[(&str, P)]) -> Fx {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let base = t.path().canonicalize().unwrap();
    let home = base.join("home");
    let root = base.join("root");
    let bin = base.join("bin");
    let log = base.join("security.log");
    for d in [&home, &bin] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = ProfileStore::new(root.clone());
    for (name, _) in spec {
        store.add(&n(name)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let mut cases = String::new();
    for (name, p) in spec {
        let dir = store.resolve(&n(name)).unwrap();
        let (account, file, key, code) = match *p {
            P::File => (true, Some(OAUTH), false, None),
            P::Console => (false, Some("{}"), false, None),
            P::Acct(c) => (true, None, false, Some(c)),
            P::AcctKey(c) => (true, None, true, Some(c)),
            P::Bare => (false, None, false, None),
            P::BareKey => (false, None, true, None),
        };
        if account {
            private(&dir.join(".claude.json"), ACCOUNT);
        }
        if let Some(body) = file {
            private(&dir.join(CREDS), body);
        }
        if key {
            private(
                &dir.join("settings.json"),
                r#"{"apiKeyHelper":"echo CANARY-LEAK-PROBE"}"#,
            );
        }
        if let Some(code) = code {
            cases.push_str(&format!("  '{}') exit {code} ;;\n", service_name(&dir)));
        }
    }
    let script = format!(
        "#!/bin/sh\nprintf '%s|%s\\n' \"$HOME\" \"$*\" >> '{}'\ncase \"$3\" in\n{cases}  *) exit 44 ;;\nesac\n",
        log.display()
    );
    write_executable(&bin.join("security"), &script);
    let profiles = spec.iter().map(|(name, _)| n(name)).collect();
    Fx {
        _t: t,
        home,
        root,
        store,
        bin,
        log,
        profiles,
    }
}

impl Fx {
    fn lines(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn starts(&self) -> usize {
        self.lines().len()
    }

    fn own(&self) -> OwnLogins<'_> {
        OwnLogins::new(&self.store, Some(&self.home))
    }

    fn state(&self) -> State {
        let state = State::new(&self.root).unwrap();
        state.ensure().unwrap();
        state
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

fn reading(pct: f64, now: i64) -> Reading {
    Reading {
        five_hour: Some(Window {
            used_percentage: pct,
            resets_at: now + 3600,
        }),
        seven_day: Some(Window {
            used_percentage: 10.0,
            resets_at: now + 86_400,
        }),
        observed_at: now - 10,
        source: ReadingSource::Statusline,
    }
}

// --- OwnLogins::of: the outcome table ------------------------------------------------

/// One fixture, every row: the outcome, and how many times `security` was
/// started for it.
fn table() -> Vec<(&'static str, P, OwnLogin, usize)> {
    vec![
        ("filed", P::File, OwnLogin::File, 0),
        ("console", P::Console, OwnLogin::File, 0),
        ("present", P::Acct(0), OwnLogin::Keychain, 1),
        ("absent", P::Acct(44), OwnLogin::None, 1),
        (
            "locked",
            P::Acct(51),
            OwnLogin::KeychainUnreadable(Cause::Exit(51)),
            1,
        ),
        // An API-key setting and no item: the profile is an API-key profile,
        // holds its login outside the directory and still counts.
        ("keyed", P::AcctKey(44), OwnLogin::KeyProfile, 1),
        // A present item beats the key setting, as the guard decides.
        ("both", P::AcctKey(0), OwnLogin::Keychain, 1),
        // A locked Keychain is never taken as an API key.
        (
            "lockedkey",
            P::AcctKey(51),
            OwnLogin::KeychainUnreadable(Cause::Exit(51)),
            1,
        ),
        // No account: the item cannot be tied to the profile, so no start.
        ("bare", P::Bare, OwnLogin::None, 0),
        ("barekey", P::BareKey, OwnLogin::KeyProfile, 0),
    ]
}

#[test]
fn on_macos_each_profile_is_classified_by_the_same_rule_as_the_guard() {
    let rows = table();
    let spec: Vec<(&str, P)> = rows.iter().map(|(name, p, ..)| (*name, *p)).collect();
    for (name, _, want, starts) in &rows {
        // A fresh fixture and log per row, so the start count is the row's.
        let f = fx(&spec);
        let _seam = test_seam(&f.bin);
        let own = f.own();
        assert_eq!(own.of(&n(name)), *want, "{name}");
        assert_eq!(f.starts(), *starts, "{name}: security starts");
        assert_eq!(own.of(&n(name)), *want, "{name}: memoised");
        assert_eq!(f.starts(), *starts, "{name}: not asked twice");
        assert_eq!(
            own.unreadable(&n(name)),
            match want {
                OwnLogin::KeychainUnreadable(c) => Some(*c),
                _ => None,
            },
            "{name}"
        );
    }
}

#[test]
fn an_api_key_override_is_a_key_profile_and_never_asks_the_keychain() {
    let f = fx(&[("work", P::Acct(51))]);
    private(
        &f.root.join(".auto-switch"),
        "enabled=off\nkind=work:api-key\n",
    );
    let _seam = test_seam(&f.bin);
    assert_eq!(f.own().of(&n("work")), OwnLogin::KeyProfile);
    assert_eq!(f.starts(), 0);
}

#[test]
fn a_subscription_override_asks_the_keychain() {
    let f = fx(&[("work", P::AcctKey(0))]);
    private(
        &f.root.join(".auto-switch"),
        "enabled=off\nkind=work:subscription\n",
    );
    let _seam = test_seam(&f.bin);
    assert_eq!(f.own().of(&n("work")), OwnLogin::Keychain);
}

#[test]
fn off_macos_the_login_file_alone_decides_and_security_is_never_started() {
    let rows = table();
    let spec: Vec<(&str, P)> = rows.iter().map(|(name, p, ..)| (*name, *p)).collect();
    let f = fx(&spec);
    // No seam: the ambient platform in a test build is not macOS.
    let own = f.own();
    for (name, p, ..) in &rows {
        let want = if matches!(p, P::File | P::Console) {
            OwnLogin::File
        } else {
            // Never `KeyProfile` here: that answer exists only on macOS.
            OwnLogin::None
        };
        assert_eq!(own.of(&n(name)), want, "{name}");
    }
    assert_eq!(f.starts(), 0);
}

#[test]
fn the_login_file_beats_the_keychain_and_the_keychain_is_not_asked() {
    let f = fx(&[("filed", P::File)]);
    let _seam = test_seam(&f.bin);
    assert_eq!(f.own().of(&n("filed")), OwnLogin::File);
    assert_eq!(f.starts(), 0);
}

// --- unreadable_note ---------------------------------------------------------------------

#[test]
fn the_unreadable_note_is_one_line_naming_the_profile_and_no_account_value() {
    for cause in [
        Cause::NoHome,
        Cause::NotStarted,
        Cause::Exit(51),
        Cause::Signal,
        Cause::Timeout,
    ] {
        let note = unreadable_note(&n("work"), cause);
        assert!(note.starts_with("auto-switch: "), "{note}");
        assert!(note.contains("work"), "{note}");
        assert!(
            note.contains("Keychain") && note.contains("could not be read"),
            "{note}"
        );
        assert!(note.contains(&cause.to_string()), "{note}");
        assert!(!note.contains('\n'), "{note}");
        assert!(!note.contains("SENTINEL"), "{note}");
    }
}

// --- the status column --------------------------------------------------------------------

#[test]
fn the_status_login_column_has_two_new_values_after_the_old_ones() {
    let f = fx(&[
        ("filed", P::File),
        ("present", P::Acct(0)),
        ("locked", P::Acct(51)),
        ("absent", P::Acct(44)),
        ("keyed", P::AcctKey(44)),
        ("leased", P::Acct(0)),
        ("parked", P::Acct(0)),
    ]);
    let _seam = test_seam(&f.bin);
    let mut ledger = Ledger {
        version: 1,
        hosts: BTreeMap::new(),
    };
    let host = |live: &str| HostEntry {
        live: live.into(),
        swapped_at: 0,
        pre_swap_fingerprint: None,
    };
    // `leased` is live in `filed`'s directory; `parked` hosts another login.
    ledger.hosts.insert("filed".into(), host("leased"));
    ledger.hosts.insert("parked".into(), host("filed"));
    let own = f.own();
    let place = |p: &str| login_place(&ledger, &own, &n(p));
    assert_eq!(place("leased"), "live in filed");
    assert_eq!(place("parked"), "parked");
    assert_eq!(f.starts(), 0, "a leased or parked profile is not asked");
    let own = f.own();
    let ledger = Ledger {
        version: 1,
        hosts: BTreeMap::new(),
    };
    let place = |p: &str| login_place(&ledger, &own, &n(p));
    assert_eq!(place("filed"), "home");
    assert_eq!(place("present"), "keychain");
    assert_eq!(place("locked"), "keychain unreadable");
    assert_eq!(place("absent"), "none");
    // An API-key profile with no file shows `none` and still counts: the
    // column says where a login file or item is, and it has neither.
    assert_eq!(place("keyed"), "none");
}

#[test]
fn off_macos_the_status_login_column_is_home_or_none() {
    let f = fx(&[("filed", P::File), ("present", P::Acct(0))]);
    let ledger = Ledger {
        version: 1,
        hosts: BTreeMap::new(),
    };
    let own = f.own();
    assert_eq!(login_place(&ledger, &own, &n("filed")), "home");
    assert_eq!(login_place(&ledger, &own, &n("present")), "none");
    assert_eq!(f.starts(), 0);
}

// --- the selection sites --------------------------------------------------------------------

#[test]
fn login_ready_follows_the_same_rule() {
    let _l = lock();
    let f = fx(&[
        ("filed", P::File),
        ("present", P::Acct(0)),
        ("locked", P::Acct(51)),
        ("absent", P::Acct(44)),
        ("keyed", P::AcctKey(44)),
    ]);
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _seam = test_seam(&f.bin);
    let state = f.state();
    let inputs = OverflowInputs::load_at(&f.root, &cfg(&[]), fsx::now(), Some(&f.home));
    let own = f.own();
    for (name, ready) in [
        ("filed", true),
        ("present", true),
        ("locked", false),
        ("absent", false),
        ("keyed", true),
    ] {
        assert_eq!(
            login_ready(&state, &inputs, &own, &n(name)),
            ready,
            "{name}"
        );
    }
}

#[test]
fn login_gap_and_refuse_no_login_follow_the_same_rule() {
    let _l = lock();
    let f = fx(&[
        ("filed", P::File),
        ("present", P::Acct(0)),
        ("locked", P::Acct(51)),
        ("absent", P::Acct(44)),
        ("bare", P::Bare),
        ("keyed", P::AcctKey(44)),
    ]);
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _seam = test_seam(&f.bin);
    // A Keychain that could not be read is its own reason, never "no login".
    for (name, gap) in [
        ("filed", None),
        ("present", None),
        (
            "locked",
            Some(LoginGap::KeychainUnreadable(Cause::Exit(51))),
        ),
        ("absent", Some(LoginGap::NoLogin)),
        ("bare", Some(LoginGap::NoLogin)),
        ("keyed", None),
    ] {
        assert_eq!(login_gap(&f.root, &n(name)), gap, "{name}");
    }
    // `refuse_no_login` takes exactly the profiles `login_gap` names.
    let mut inputs = OverflowInputs {
        order: f.profiles.clone(),
        blocked: Vec::new(),
        gate: Some(GATE),
        platform: None,
        not_plan: Vec::new(),
        no_login: Vec::new(),
        kinds_unknown: false,
    };
    let unreadable = inputs.refuse_no_login(&f.root);
    let mut blocked = inputs.blocked.clone();
    blocked.sort();
    assert_eq!(blocked, ["absent", "bare", "locked"]);
    // The cause comes back with the one profile the Keychain decided, so
    // the capacity writers can say it.
    assert_eq!(unreadable, vec![(n("locked"), Cause::Exit(51))]);
    // Every recorded command is the documented one: the arguments only, as
    // `HOME` here is the process home.
    for line in f.lines() {
        assert!(
            line.contains("|find-generic-password -s Claude Code-credentials-"),
            "{line}"
        );
        assert!(!line.contains(" -w") && !line.contains(" -g"), "{line}");
    }
}

#[test]
fn the_rotators_live_profile_still_holds_its_login_in_home() {
    let _l = lock();
    let f = fx(&[("absent", P::Acct(44)), ("locked", P::Acct(51))]);
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _seam = test_seam(&f.bin);
    assert_eq!(login_gap(&f.root, &n("absent")), Some(LoginGap::NoLogin));
    let rs = RotateState::new(&f.root);
    rs.ensure().unwrap();
    let live = |name: &str| {
        rs.save_ledger(&RotateLedger {
            live: Some(name.into()),
            ..RotateLedger::default()
        })
        .unwrap();
    };
    live("absent");
    assert_eq!(
        login_gap(&f.root, &n("absent")),
        None,
        "its file is in ~/.claude"
    );
    // The same holds whatever the Keychain says: the login is in
    // `~/.claude`, so an unreadable Keychain is no reason to name here.
    live("locked");
    assert_eq!(login_gap(&f.root, &n("locked")), None);
    let mut inputs = OverflowInputs {
        order: vec![n("locked")],
        blocked: Vec::new(),
        gate: Some(GATE),
        platform: None,
        not_plan: Vec::new(),
        no_login: Vec::new(),
        kinds_unknown: false,
    };
    assert!(inputs.refuse_no_login(&f.root).is_empty());
    assert!(inputs.blocked.is_empty());
}

// --- the Keychain cause reaches the user (crusty round 1) -----------------------------------

#[test]
fn overflow_add_has_one_warning_for_no_login_and_another_for_the_keychain() {
    assert_eq!(
        LoginGap::NoLogin.overflow_warning(&n("work")),
        "work has no login file, so it cannot be used as overflow until it has one: neither the rotator nor auto-switch can move onto it, and the capacity file does not count it"
    );
    for cause in [
        Cause::NoHome,
        Cause::NotStarted,
        Cause::Exit(51),
        Cause::Signal,
        Cause::Timeout,
    ] {
        let text = LoginGap::KeychainUnreadable(cause).overflow_warning(&n("work"));
        assert!(
            text.starts_with(&format!(
                "work has no login file and the macOS Keychain could not be read ({cause}), "
            )),
            "{text}"
        );
        assert!(text.contains("Unlock the login Keychain"), "{text}");
        // Not the other remedy: nothing here says the profile needs a login.
        assert!(!text.contains("has no login file, so"), "{text}");
        assert!(!text.contains("until it has one"), "{text}");
        assert!(!text.contains('\n'), "{text}");
        assert!(!text.contains("SENTINEL"), "{text}");
    }
}

/// `work` is a plan login; every other profile in `spec` is marked as
/// overflow, and `exclude` is the exclusion list as the file writes it.
fn capacity_fx(spec: &[(&str, P)], exclude: &str) -> Fx {
    let f = fx(spec);
    let marked: Vec<&str> = spec
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| *name != "work")
        .collect();
    private(
        &f.root.join(".auto-switch"),
        &format!(
            "enabled=on\nexclude={exclude}\noverflow={}\n",
            marked.join(",")
        ),
    );
    f
}

/// What each capacity writer hands back for `f`: the rotator's, then the
/// status line's and `profile usage`'s.
fn capacity_notes(f: &Fx) -> [Vec<String>; 2] {
    let now = fsx::now();
    let readings = [(n("work"), Rotated::Exhausted)];
    let state = f.state();
    let (usage, ledger) = (state.load_usage(), state.load_ledger().unwrap());
    [
        capacity::refresh_from_rotation(&f.root, &readings, now, false, true, Some(&f.home)),
        capacity::refresh(&state, &usage, &ledger, "usage"),
    ]
}

#[test]
fn both_capacity_writers_name_an_overflow_profile_whose_keychain_is_unreadable() {
    let _l = lock();
    let f = capacity_fx(
        &[
            ("work", P::File),
            ("locked", P::Acct(51)),
            ("absent", P::Acct(44)),
            ("shut", P::Acct(51)),
        ],
        "shut",
    );
    let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
    let _seam = test_seam(&f.bin);
    // One line, for the profile the Keychain decided. `absent` has no login
    // and is left out without a word, as before; `shut` is excluded, so it
    // never counted and its Keychain is nobody's concern.
    let want = vec![unreadable_note(&n("locked"), Cause::Exit(51))];
    for (writer, notes) in ["rotate", "usage"].into_iter().zip(capacity_notes(&f)) {
        assert_eq!(notes, want, "{writer}");
    }
}

#[test]
fn the_capacity_writers_say_nothing_when_the_keychain_answers_or_auto_switch_is_off() {
    let _l = lock();
    let answered = capacity_fx(
        &[
            ("work", P::File),
            ("present", P::Acct(0)),
            ("absent", P::Acct(44)),
        ],
        "",
    );
    let off = capacity_fx(&[("work", P::File), ("locked", P::Acct(51))], "");
    private(
        &off.root.join(".auto-switch"),
        "enabled=off\noverflow=locked\n",
    );
    for (case, f) in [("answered", &answered), ("off", &off)] {
        let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
        let _seam = test_seam(&f.bin);
        for notes in capacity_notes(f) {
            assert!(notes.is_empty(), "{case}: {notes:?}");
        }
    }
}

#[test]
fn selection_chooses_a_keychain_login_and_passes_over_the_rest() {
    let _l = lock();
    let now = fsx::now();
    for (spare, expect) in [
        (P::Acct(0), Some("spare")),
        (P::Acct(44), None),
        (P::Acct(51), None),
        (P::File, Some("spare")),
    ] {
        let f = fx(&[("work", P::File), ("spare", spare)]);
        let _env = EnvGuard::set([("HOME", f.home.to_str().unwrap())]);
        let _seam = test_seam(&f.bin);
        let state = f.state();
        let ledger = state.load_ledger().unwrap();
        let readings: BTreeMap<String, Reading> = [
            ("work".to_string(), reading(100.0, now)),
            ("spare".to_string(), reading(10.0, now)),
        ]
        .into();
        let got = nested_candidate(
            &state,
            &ledger,
            &f.profiles,
            &readings,
            &cfg(&[]),
            "work",
            &[],
        );
        assert_eq!(
            got.as_ref().map(|p| p.profile.as_str()),
            expect,
            "exit status or file of spare: {}",
            f.lines().join(" / ")
        );
    }
}

#[test]
fn the_takeover_check_follows_the_same_rule_and_hands_over_its_home() {
    let _l = lock();
    for (other, takes) in [
        (P::Acct(0), true),
        (P::Acct(44), false),
        (P::Acct(51), false),
    ] {
        let f = fx(&[("work", P::File), ("other", other)]);
        // The process home is somewhere else: only the argument may reach
        // the lookup.
        let elsewhere = f.home.parent().unwrap().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        let _env = EnvGuard::set([("HOME", elsewhere.to_str().unwrap())]);
        let _seam = test_seam(&f.bin);
        private(&f.root.join(".auto-switch"), "enabled=on\n");
        assert_eq!(
            could_take_over_at(Some(&f.store), Some("work"), Some(&f.home)),
            takes
        );
        for line in f.lines() {
            assert!(
                line.starts_with(&format!("{}|", f.home.display())),
                "HOME was not the argument: {line}"
            );
        }
        assert!(f.starts() >= 1, "the Keychain was asked");
    }
}

// --- no_login_of asks no Keychain question ----------------------------------------------------

#[test]
fn on_macos_the_rotators_login_tests_list_nothing_and_no_keychain_is_asked() {
    // `spare` has no login file and no account: on Linux the rotator's login
    // test lists it. On macOS the list stays empty, as before, so a Keychain
    // answer can never release the hold on the API key.
    let f = fx(&[
        ("console", P::Console),
        ("work", P::File),
        ("spare", P::Bare),
    ]);
    let c = cfg(&["console"]);
    let now = 2_000_000_000;
    let load =
        || OverflowInputs::load_with_gate(&f.root, &c, (Some(GATE), GateSource::Environment), now);
    assert_eq!(load().no_login, vec![n("spare")], "off macOS");
    let _seam = test_seam(&f.bin);
    assert!(Ctx::load(&f.store, None).macos());
    assert!(load().no_login.is_empty(), "on macOS");
    assert_eq!(f.starts(), 0);
}
