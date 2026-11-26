//! Issue #2885: kind resolution and the launch guard on macOS, where a
//! profile signed in through the login Keychain has no login file. See
//! "Keychain logins" and "Kind detection" in docs/reference/profile-command.md.
//!
//! Every test names the platform itself: in a test build the ambient platform
//! reads as not macOS unless a scoped `keychain::test_seam` is held, so these
//! run the same on Linux CI and on a Mac, and none starts the system
//! `security`. The fake `security` answers by service name, so a test also
//! proves that each profile is asked about its own directory.
//!
//! Contract (`profile::kind`, `profile::ProfileError`):
//! - `Resolved` gains `keychain: Option<Keychain>` (`None`: not consulted)
//!   and `keychain_login()`, true for `Some(Keychain::Present)`.
//!   `own_oauth` keeps its meaning: the login file is OAuth.
//! - `resolve_with(&Evidence, Option<Keychain>) -> Resolved`;
//!   `resolve_from(e)` is `resolve_with(e, None)` and gives the same nine
//!   sources as before.
//! - Rule 3: with the login file missing and the Keychain consulted,
//!   `Present` is subscription, `detected: oauth login in the macOS
//!   Keychain`; `Unreadable(_)` is subscription, `detected: macOS Keychain
//!   unreadable`, whatever `key_setting` says; `Absent` falls through.
//! - `guard` for a subscription profile with no OAuth login file:
//!   `held` passes; else a present Keychain item passes; else an unreadable
//!   Keychain is `ProfileError::KeychainUnreadable { name: String, cause:
//!   Cause }`; else `SubscriptionNoLogin`.
//! - `Ctx::macos() -> bool`; `Ctx` consults the Keychain per profile, in
//!   `resolve`, `resolve_kind`, `kind` and `try_resolve_all` alike, only
//!   when the platform is macOS, no override says `api-key`, the login file
//!   is missing and `<dir>/.claude.json` has an identifiable `oauthAccount`.

use super::ProfileError;
use super::keychain::{Cause, Keychain, service_name, test_seam};
use super::kind::{
    Ctx, Evidence, Kind, KindSource, Resolved, guard, kind_of, launch_guard, platform_profiles,
    resolve, resolve_from, resolve_with,
};
use super::name::ProfileName;
use super::rotate::identify::LoginKind;
use super::store::ProfileStore;
use crate::test_support::write_executable;
use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const PRESENT: &str = "detected: oauth login in the macOS Keychain";
const UNREADABLE: &str = "detected: macOS Keychain unreadable";
const ACCOUNT: &str = r#"{"oauthAccount":{"accountUuid":"uuid-SENTINEL-KEYCHAIN","emailAddress":"user-SENTINEL-KEYCHAIN@example.com"}}"#;
const OAUTH: &str = r#"{"claudeAiOauth":{"accessToken":"CANARY-LEAK-PROBE"}}"#;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// A store with:
/// - `work`: an account, no login file;
/// - `keyed`: an account, no login file, and an API-key setting;
/// - `bare`: no account and no login file;
/// - `filed`: an account and an OAuth login file.
///
/// A fake `security` answers by service: `work` and `keyed` with `code`,
/// every other directory with 44 (no item).
struct Fx {
    _t: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
    bin: PathBuf,
    log: PathBuf,
}

fn fx(code: i32) -> Fx {
    fx_rules(&[("work", code), ("keyed", code)])
}

fn fx_rules(rules: &[(&str, i32)]) -> Fx {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = t.path().join("home");
    let root = t.path().join("root");
    let bin = t.path().join("bin");
    let log = t.path().join("security.log");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let store = ProfileStore::new(root.clone());
    for p in ["work", "keyed", "bare", "filed"] {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    for p in ["work", "keyed", "filed"] {
        private(&store.resolve(&n(p)).unwrap().join(".claude.json"), ACCOUNT);
    }
    private(
        &store.resolve(&n("keyed")).unwrap().join("settings.json"),
        r#"{"apiKeyHelper":"echo CANARY-LEAK-PROBE"}"#,
    );
    private(
        &store
            .resolve(&n("filed"))
            .unwrap()
            .join(".credentials.json"),
        OAUTH,
    );
    let mut cases = String::new();
    for (name, code) in rules {
        let service = service_name(&store.resolve(&n(name)).unwrap());
        cases.push_str(&format!("  '{service}') exit {code} ;;\n"));
    }
    let script = format!(
        "#!/bin/sh\nprintf '%s|%s\\n' \"$HOME\" \"$*\" >> '{}'\ncase \"$3\" in\n{cases}  *) exit 44 ;;\nesac\n",
        log.display()
    );
    write_executable(&bin.join("security"), &script);
    Fx {
        _t: t,
        home,
        store,
        bin,
        log,
    }
}

impl Fx {
    /// One line per `security` start.
    fn starts(&self) -> usize {
        fs::read_to_string(&self.log).map_or(0, |s| s.lines().count())
    }

    fn toggle(&self, body: &str) {
        private(&self.store.root().join(".auto-switch"), body);
    }

    fn kind(&self, p: &str) -> (Kind, String) {
        let r = resolve(&self.store, &n(p), &self.home);
        (r.kind, r.source.to_string())
    }

    fn guard(&self, p: &str, held: bool) -> Result<(), ProfileError> {
        launch_guard(&self.store, &n(p), Some(&self.home), || held)
    }
}

fn is_no_login(r: &Result<(), ProfileError>, name: &str) -> bool {
    matches!(r, Err(ProfileError::SubscriptionNoLogin(p)) if p == name)
}

// --- the pure rule table ---------------------------------------------------------

fn ev(own: LoginKind) -> Evidence {
    Evidence {
        override_kind: None,
        own_login: own,
        home_login: None,
        key_setting: false,
    }
}

fn detected(e: &Evidence, k: Option<Keychain>) -> (Kind, String, bool) {
    let r = resolve_with(e, k);
    (r.kind, r.source.to_string(), r.keychain_login())
}

#[test]
fn resolve_from_is_resolve_with_no_keychain_answer() {
    use LoginKind::*;
    for own in [OAuth, NoOAuth, Unreadable, Missing] {
        for home in [None, Some(OAuth), Some(NoOAuth), Some(Missing)] {
            for key in [false, true] {
                let mut e = ev(own);
                e.home_login = home;
                e.key_setting = key;
                assert_eq!(resolve_from(&e), resolve_with(&e, None));
                assert!(resolve_from(&e).keychain.is_none());
                assert!(!resolve_from(&e).keychain_login());
            }
        }
    }
}

#[test]
fn rule_3_a_present_item_is_a_subscription_login_of_its_own() {
    for key in [false, true] {
        let mut e = ev(LoginKind::Missing);
        e.key_setting = key;
        let (k, s, login) = detected(&e, Some(Keychain::Present));
        assert_eq!((k, s.as_str(), login), (Kind::Subscription, PRESENT, true));
        assert!(
            !resolve_with(&e, Some(Keychain::Present)).own_oauth,
            "no file"
        );
    }
}

#[test]
fn rule_3_an_unreadable_keychain_is_a_subscription_with_no_login_even_with_a_key() {
    for cause in [
        Cause::NoHome,
        Cause::NotStarted,
        Cause::Exit(51),
        Cause::Signal,
        Cause::Wait,
    ] {
        for key in [false, true] {
            let mut e = ev(LoginKind::Missing);
            e.key_setting = key;
            let (k, s, login) = detected(&e, Some(Keychain::Unreadable(cause)));
            assert_eq!(
                (k, s.as_str(), login),
                (Kind::Subscription, UNREADABLE, false)
            );
        }
    }
}

#[test]
fn rule_3_an_absent_item_falls_through_to_the_old_rules() {
    let mut e = ev(LoginKind::Missing);
    assert_eq!(
        detected(&e, Some(Keychain::Absent)),
        (Kind::Subscription, "detected: no login".into(), false)
    );
    e.key_setting = true;
    assert_eq!(
        detected(&e, Some(Keychain::Absent)),
        (Kind::Platform, "detected: api key setting".into(), false),
        "an API-key setting still decides, as before"
    );
    let mut live = ev(LoginKind::Missing);
    live.home_login = Some(LoginKind::NoOAuth);
    assert_eq!(
        detected(&live, Some(Keychain::Absent)).1,
        "detected: live in ~/.claude (no oauth login)"
    );
}

#[test]
fn rule_3_comes_before_the_live_in_home_rule() {
    let mut e = ev(LoginKind::Missing);
    e.home_login = Some(LoginKind::NoOAuth);
    assert_eq!(detected(&e, Some(Keychain::Present)).1, PRESENT);
}

#[test]
fn an_override_keeps_its_source_and_still_carries_the_keychain_answer() {
    for k in [Kind::Subscription, Kind::Platform] {
        let mut e = ev(LoginKind::Missing);
        e.override_kind = Some(k);
        let r = resolve_with(&e, Some(Keychain::Present));
        assert_eq!((r.kind, r.source), (k, KindSource::Override));
        assert!(r.keychain_login());
    }
}

#[test]
fn nothing_but_a_present_item_makes_a_keychain_login() {
    for k in [
        None,
        Some(Keychain::Absent),
        Some(Keychain::Unreadable(Cause::Exit(51))),
        Some(Keychain::Unreadable(Cause::Wait)),
    ] {
        assert!(
            !resolve_with(&ev(LoginKind::Missing), k).keychain_login(),
            "{k:?}"
        );
    }
}

// --- the guard --------------------------------------------------------------------

fn resolved(k: Option<Keychain>) -> Resolved {
    resolve_with(&ev(LoginKind::Missing), k)
}

#[test]
fn the_guard_passes_a_present_item_and_refuses_the_rest_with_the_right_error() {
    let name = n("work");
    assert!(guard(&resolved(Some(Keychain::Present)), &name, false).is_ok());
    assert!(matches!(
        guard(&resolved(Some(Keychain::Absent)), &name, false),
        Err(ProfileError::SubscriptionNoLogin(p)) if p == "work"
    ));
    assert!(matches!(
        guard(&resolved(None), &name, false),
        Err(ProfileError::SubscriptionNoLogin(_))
    ));
    let err = guard(
        &resolved(Some(Keychain::Unreadable(Cause::Exit(51)))),
        &name,
        false,
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            ProfileError::KeychainUnreadable { ref name, cause: Cause::Exit(51) } if name == "work"
        ),
        "{err:?}"
    );
}

#[test]
fn a_held_login_passes_whatever_the_keychain_said() {
    let name = n("work");
    for k in [
        None,
        Some(Keychain::Absent),
        Some(Keychain::Present),
        Some(Keychain::Unreadable(Cause::Wait)),
    ] {
        assert!(guard(&resolved(k), &name, true).is_ok(), "{k:?}");
    }
}

#[test]
fn an_api_key_profile_is_never_asked_about_the_keychain_in_the_guard() {
    let mut e = ev(LoginKind::Missing);
    e.key_setting = true;
    let r = resolve_with(&e, Some(Keychain::Absent));
    assert_eq!(r.kind, Kind::Platform);
    assert!(guard(&r, &n("keyed"), false).is_ok());
}

#[test]
fn the_unreadable_refusal_names_the_profile_the_cause_and_the_remedy() {
    let name = "work".to_string();
    let err = ProfileError::KeychainUnreadable {
        name: name.clone(),
        cause: Cause::Exit(51),
    };
    assert_eq!(
        err.to_string(),
        "profile work has no login file and the macOS Keychain could not be read (`security` exit status 51); launching it could bill the Claude Platform API key per token. Unlock the login Keychain and run again, or run 'amplihack profile kind set work api-key' if per-token billing is intended."
    );
    assert_eq!(
        err.hint(),
        "unlock the login Keychain and run again, or: amplihack profile kind set <name> api-key"
    );
    // Each cause, and never the old refusal's words.
    for cause in [
        Cause::NoHome,
        Cause::NotStarted,
        Cause::Exit(1),
        Cause::Signal,
        Cause::Wait,
    ] {
        let text = ProfileError::KeychainUnreadable {
            name: name.clone(),
            cause,
        }
        .to_string();
        assert!(
            text.contains("Keychain") && text.contains("could not be read"),
            "{text}"
        );
        assert!(text.contains(&format!("({cause})")), "{text}");
        assert!(!text.contains("no login of its own"), "{text}");
        assert!(!text.contains("platform"), "no internal name: {text}");
    }
}

#[test]
fn the_old_refusal_is_unchanged_byte_for_byte() {
    assert_eq!(
        ProfileError::SubscriptionNoLogin("ghost".into()).to_string(),
        "profile ghost is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. Run 'amplihack profile login ghost', or run 'amplihack profile kind set ghost api-key' if per-token billing is intended."
    );
}

// --- on disk, through the real Ctx ------------------------------------------------

#[test]
fn a_present_item_resolves_and_launches() {
    let f = fx(0);
    let _seam = test_seam(&f.bin);
    assert_eq!(f.kind("work"), (Kind::Subscription, PRESENT.into()));
    assert!(f.guard("work", false).is_ok());
    let r = resolve(&f.store, &n("work"), &f.home);
    assert!(r.keychain_login() && !r.own_oauth);
    assert!(!r.may_be_live(), "a Keychain login has no file to move");
}

#[test]
fn an_absent_item_keeps_the_old_refusal() {
    let f = fx(44);
    let _seam = test_seam(&f.bin);
    assert_eq!(
        f.kind("work"),
        (Kind::Subscription, "detected: no login".into())
    );
    assert!(is_no_login(&f.guard("work", false), "work"));
}

#[test]
fn an_absent_item_with_an_api_key_setting_is_still_an_api_key_profile() {
    let f = fx(44);
    let _seam = test_seam(&f.bin);
    assert_eq!(
        f.kind("keyed"),
        (Kind::Platform, "detected: api key setting".into())
    );
    assert!(f.guard("keyed", false).is_ok());
}

#[test]
fn a_locked_keychain_is_refused_distinctly_and_is_not_an_api_key() {
    let f = fx(51);
    let _seam = test_seam(&f.bin);
    for p in ["work", "keyed"] {
        assert_eq!(f.kind(p), (Kind::Subscription, UNREADABLE.into()), "{p}");
        let err = f.guard(p, false).unwrap_err();
        assert!(
            matches!(
                err,
                ProfileError::KeychainUnreadable { cause: Cause::Exit(51), ref name } if name == p
            ),
            "{p}: {err:?}"
        );
        let text = err.to_string();
        assert!(text.contains("could not be read"), "{text}");
        assert!(!text.contains("no login of its own"), "{text}");
        assert!(!text.contains("SENTINEL"), "no account value: {text}");
    }
}

#[test]
fn a_keychain_that_cannot_be_asked_is_unreadable_not_absent() {
    let f = fx(0);
    fs::remove_file(f.bin.join("security")).unwrap();
    let _seam = test_seam(&f.bin);
    let err = f.guard("work", false).unwrap_err();
    assert!(
        matches!(
            err,
            ProfileError::KeychainUnreadable {
                cause: Cause::NotStarted,
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn each_profile_is_asked_about_its_own_directory() {
    // Only `keyed` has an item: `work` must read absent, not borrow its answer.
    let f = fx_rules(&[("keyed", 0)]);
    let _seam = test_seam(&f.bin);
    assert_eq!(f.kind("keyed").1, PRESENT);
    assert_eq!(f.kind("work").1, "detected: no login");
}

#[test]
fn the_keychain_is_asked_only_when_nothing_else_has_decided() {
    let f = fx(0);
    let _seam = test_seam(&f.bin);
    // A login file (any kind) decides by rules 1 and 2.
    assert_eq!(f.kind("filed").1, "detected: oauth login");
    let dir = f.store.resolve(&n("filed")).unwrap();
    fs::write(dir.join(".credentials.json"), "{}").unwrap();
    assert_eq!(f.kind("filed").0, Kind::Platform);
    fs::write(dir.join(".credentials.json"), "{not json").unwrap();
    assert_eq!(f.kind("filed").1, "detected: login file unreadable");
    // No account to name: the item cannot be tied to the profile.
    assert_eq!(
        f.kind("bare"),
        (Kind::Subscription, "detected: no login".into())
    );
    assert_eq!(f.starts(), 0, "security was not started for any of these");
}

#[test]
fn an_api_key_override_never_asks_the_keychain() {
    let f = fx(51);
    let _seam = test_seam(&f.bin);
    f.toggle("enabled=off\nkind=work:api-key\n");
    assert_eq!(f.kind("work"), (Kind::Platform, "override".into()));
    assert!(f.guard("work", false).is_ok());
    assert_eq!(f.starts(), 0);
}

#[test]
fn a_subscription_override_asks_the_keychain_with_the_same_three_outcomes() {
    for (code, ok) in [(0, true), (44, false), (51, false)] {
        let f = fx(code);
        let _seam = test_seam(&f.bin);
        f.toggle("enabled=off\nkind=work:subscription\n");
        assert_eq!(
            f.kind("work"),
            (Kind::Subscription, "override".into()),
            "{code}"
        );
        let got = f.guard("work", false);
        assert_eq!(got.is_ok(), ok, "exit {code}: {got:?}");
        match code {
            44 => assert!(is_no_login(&got, "work")),
            51 => assert!(matches!(got, Err(ProfileError::KeychainUnreadable { .. }))),
            _ => {}
        }
    }
}

#[test]
fn a_corrupt_toggle_file_counts_a_keychain_login_and_still_refuses_the_rest() {
    for (code, ok) in [(0, true), (44, false), (51, false)] {
        let f = fx(code);
        let _seam = test_seam(&f.bin);
        f.toggle("enabled=off\nkind=work:platform,work:platform\n");
        assert!(Ctx::load(&f.store, Some(&f.home)).corrupt());
        // Corrupt: every profile is checked as a subscription profile, so
        // even `keyed` needs its login, and a Keychain login counts.
        assert_eq!(f.guard("work", false).is_ok(), ok, "exit {code}");
        assert_eq!(f.guard("keyed", false).is_ok(), ok, "exit {code} keyed");
        assert!(f.guard("filed", false).is_ok(), "its own login file");
    }
}

#[test]
fn a_held_profile_passes_even_when_the_keychain_cannot_be_read() {
    let f = fx(51);
    let _seam = test_seam(&f.bin);
    assert!(f.guard("work", true).is_ok());
}

#[test]
fn held_is_asked_only_when_a_refusal_would_follow() {
    let asked = Cell::new(0);
    let run = |code: i32| {
        let f = fx(code);
        let _seam = test_seam(&f.bin);
        asked.set(0);
        let _ = launch_guard(&f.store, &n("work"), Some(&f.home), || {
            asked.set(asked.get() + 1);
            false
        });
        asked.get()
    };
    assert_eq!(run(0), 0, "a present item needs no hold");
    assert_eq!(run(44), 1);
    assert_eq!(run(51), 1);
}

#[test]
fn off_macos_nothing_is_asked_and_detection_is_unchanged() {
    let f = fx(0);
    // No seam: the ambient platform in a test build is not macOS.
    assert!(!Ctx::load(&f.store, Some(&f.home)).macos());
    assert_eq!(
        f.kind("work"),
        (Kind::Subscription, "detected: no login".into())
    );
    assert_eq!(
        f.kind("keyed"),
        (Kind::Platform, "detected: api key setting".into())
    );
    assert!(is_no_login(&f.guard("work", false), "work"));
    assert_eq!(f.starts(), 0);
}

#[test]
fn the_platform_is_macos_only_under_the_seam() {
    let f = fx(0);
    let _seam = test_seam(&f.bin);
    assert!(Ctx::load(&f.store, Some(&f.home)).macos());
}

#[test]
fn no_home_is_unreadable_on_macos_and_starts_nothing() {
    let f = fx(0);
    let _seam = test_seam(&f.bin);
    let r = Ctx::load(&f.store, None).resolve(&f.store, &n("work"));
    assert_eq!(
        (r.kind, r.source.to_string()),
        (Kind::Subscription, UNREADABLE.into())
    );
    let err = launch_guard(&f.store, &n("work"), None, || false).unwrap_err();
    assert!(
        matches!(
            err,
            ProfileError::KeychainUnreadable {
                cause: Cause::NoHome,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(f.starts(), 0);
}

#[test]
fn every_caller_of_ctx_sees_the_same_kind() {
    // An API-key setting and a present item: the guard, `kind`, `kind_of`
    // and the API-key list must all say subscription, since the kind decides
    // the billing tier. With an absent item all of them say api-key.
    for (code, platform) in [(0, false), (44, true)] {
        let f = fx(code);
        let _seam = test_seam(&f.bin);
        let ctx = Ctx::load(&f.store, Some(&f.home));
        let keyed = n("keyed");
        let want = if platform {
            Kind::Platform
        } else {
            Kind::Subscription
        };
        assert_eq!(ctx.resolve(&f.store, &keyed).kind, want);
        assert_eq!(ctx.resolve_kind(&f.store, &keyed).kind, want);
        assert_eq!(ctx.kind(&f.store, &keyed), want);
        assert_eq!(kind_of(&f.store, &keyed, &f.home), want);
        assert_eq!(
            platform_profiles(&f.store, &f.home).contains(&keyed),
            platform,
            "exit {code}"
        );
        assert_eq!(
            ctx.try_platform_profiles(&f.store)
                .unwrap()
                .contains(&keyed),
            platform
        );
        let all = ctx.try_resolve_all(&f.store).unwrap();
        let (_, r) = all.iter().find(|(p, _)| *p == keyed).unwrap();
        assert_eq!(r.kind, want);
    }
}

#[test]
fn a_keychain_login_with_a_key_setting_warns_like_a_login_file_does() {
    let f = fx(0);
    let _seam = test_seam(&f.bin);
    let ctx = Ctx::load(&f.store, Some(&f.home));
    let r = ctx.resolve(&f.store, &n("keyed"));
    assert!(r.key_setting && r.keychain_login());
    assert!(r.warn_key_setting(), "Claude may prefer the key");
    f.toggle("enabled=off\nkind=keyed:subscription\n");
    let ctx = Ctx::load(&f.store, Some(&f.home));
    assert!(
        !ctx.resolve(&f.store, &n("keyed")).warn_key_setting(),
        "an explicit kind silences it"
    );
    // No login of any kind: nothing to warn about.
    let f = fx(44);
    let _seam = test_seam(&f.bin);
    assert!(!resolve(&f.store, &n("work"), &f.home).warn_key_setting());
}

/// Crusty round 2 of PR #2891: the lookup keeps no state between profiles,
/// so one profile's unreadable answer is not passed on to the next. Each
/// profile with an account and no file is asked once, and gets its own
/// answer, in whichever order the resolution takes them.
#[test]
fn each_profile_gets_the_answer_of_its_own_lookup() {
    let f = fx_rules(&[("work", 51), ("keyed", 0)]);
    let _seam = test_seam(&f.bin);
    let ctx = Ctx::load(&f.store, Some(&f.home));
    let all = ctx.try_resolve_all(&f.store).unwrap();
    let answer = |p: &str| {
        all.iter()
            .find(|(q, _)| q.as_str() == p)
            .unwrap()
            .1
            .keychain
    };
    assert_eq!(answer("work"), Some(Keychain::Unreadable(Cause::Exit(51))));
    assert_eq!(answer("keyed"), Some(Keychain::Present));
    assert_eq!(f.starts(), 2, "one start for each of work and keyed");
}

// --- the reference says what the code does --------------------------------------

const REFERENCE: &str = include_str!("../../../../../docs/reference/profile-command.md");
const HOWTO: &str = include_str!("../../../../../docs/howto/claude-profiles.md");

#[test]
fn the_reference_names_both_new_sources() {
    for source in [PRESENT, UNREADABLE] {
        assert!(REFERENCE.contains(&format!("`{source}`")), "{source}");
    }
}

#[test]
fn the_reference_and_howto_quote_the_unreadable_refusal_and_its_hint() {
    let err = ProfileError::KeychainUnreadable {
        name: "work".into(),
        cause: Cause::Exit(51),
    };
    let line = format!("error: {err}\nhint: {}", err.hint());
    assert!(REFERENCE.contains(&line), "reference: {line}");
    assert!(HOWTO.contains(&line), "howto: {line}");
}

#[test]
fn the_reference_lists_every_cause_text() {
    for cause in [
        Cause::NoHome,
        Cause::NotStarted,
        Cause::Exit(7),
        Cause::Signal,
        Cause::Wait,
    ] {
        let text = cause.to_string().replace('7', "<n>");
        assert!(REFERENCE.contains(&text), "{text}");
    }
}

#[test]
fn the_reference_lists_the_new_login_column_values() {
    assert!(REFERENCE.contains("`keychain`"));
    assert!(REFERENCE.contains("`keychain unreadable`"));
    assert!(
        REFERENCE.contains("`home`, `keychain`, `keychain unreadable` or\n`none`"),
        "the order of the column values is documented"
    );
}

#[test]
fn the_reference_names_the_service_rule_and_never_claims_a_refusal_is_never_billing() {
    assert!(REFERENCE.contains("Claude Code-credentials-<h>"));
    assert!(REFERENCE.contains("security find-generic-password -s <service>"));
    for doc in [
        REFERENCE,
        HOWTO,
        include_str!("../../../../../CHANGELOG.md"),
    ] {
        assert!(!doc.contains("a refusal, never billing"));
    }
}
