//! Issue #1700: a profile's kind (subscription or API key), how it is
//! resolved, the launch guard's decision and API-key tier membership. See
//! "Kind detection" and "Launch guard" in docs/reference/profile-command.md.
//!
//! Contract (`profile::kind`):
//! - `Kind::{Subscription, Platform}`; `label()` is `subscription` or
//!   `api-key`; `Kind::parse` accepts `subscription`, `platform` and
//!   `api-key` (as `Platform`), nothing else.
//! - `KindSource::{Override, Detected(&'static str)}`; `Display` is
//!   `override` or `detected: <reason>`.
//! - `Evidence { override_kind: Option<Kind>, own_login: LoginKind,
//!   home_login: Option<LoginKind>, key_setting: bool }`. `home_login` is
//!   `Some` only when the rotate ledger names this profile as live, and is
//!   then the kind of `~/.claude/.credentials.json`.
//! - `resolve_from(&Evidence) -> Resolved { kind, source, own_oauth,
//!   key_setting }` in the documented order; the first rule wins. Every
//!   unsure case is `Subscription`. `Resolved::warn_key_setting()`: an
//!   OAuth subscription profile that also names an API-key setting.
//! - `guard(&Resolved, &ProfileName, held: bool) -> Result<(),
//!   ProfileError>`: `Err(ProfileError::SubscriptionNoLogin(name))` for a
//!   subscription profile without its own OAuth login that is not `held`.
//! - `resolve(&ProfileStore, &ProfileName, home: &Path) -> Resolved`:
//!   reads overrides from `<root>/.auto-switch`, the ledger's `live` from
//!   the rotate ledger and the files through `rotate::identify`.
//! - `platform_profiles(&ProfileStore, home) -> Vec<ProfileName>`, sorted.
//! - `profile::overflow::tiers(&ProfileStore, home) -> Vec<OverflowEntry>`:
//!   the `overflow=` entries without API-key profiles, then one
//!   `OverflowEntry::platform` for the first API-key profile by name that
//!   `overflow=` names (issue #1804: marking is the consent to per-token
//!   billing; an unmarked API-key profile is never a tier).
//! - `auto_switch::config::Config.kind: BTreeMap<ProfileName, Kind>`;
//!   `render` writes `kind=` after `overflow=`, always as `platform` or
//!   `subscription`, and leaves the line out when the map is empty.
//! - `Evidence` and `LoginKind` derive `Clone`/`Copy`, `Debug` and
//!   `PartialEq`; `ProfileError::SubscriptionNoLogin(String)` holds the name.

use super::ProfileError;
use super::auto_switch::config::{self, FileState};
use super::kind::{
    Ctx, Evidence, Kind, KindSource, Resolved, guard, platform_profiles, resolve, resolve_from,
};
use super::name::ProfileName;
use super::overflow::{OverflowEntry, PLATFORM_TIER, tiers};
use super::rotate::identify::LoginKind;
use super::store::ProfileStore;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const OAUTH: &str = r#"{"claudeAiOauth":{"accessToken":"CANARY-LEAK-PROBE"}}"#;

fn n(s: &str) -> ProfileName {
    ProfileName::parse(s).unwrap()
}

fn ev(own: LoginKind) -> Evidence {
    Evidence {
        override_kind: None,
        own_login: own,
        home_login: None,
        key_setting: false,
    }
}

fn detected(e: &Evidence) -> (Kind, String) {
    let r = resolve_from(e);
    (r.kind, r.source.to_string())
}

// --- Kind and its labels --------------------------------------------------------

#[test]
fn labels_and_spellings() {
    assert_eq!(Kind::Subscription.label(), "subscription");
    assert_eq!(Kind::Platform.label(), "api-key");
    assert_eq!(Kind::parse("subscription"), Some(Kind::Subscription));
    assert_eq!(Kind::parse("platform"), Some(Kind::Platform));
    assert_eq!(Kind::parse("api-key"), Some(Kind::Platform));
    for bad in ["", "auto", "API-KEY", "Platform", "apikey", "sub"] {
        assert_eq!(Kind::parse(bad), None, "{bad}");
    }
    assert_eq!(KindSource::Override.to_string(), "override");
    assert_eq!(
        KindSource::Detected("oauth login").to_string(),
        "detected: oauth login"
    );
}

// --- resolve_from: the documented table -----------------------------------------------

#[test]
fn rule_2_the_profiles_own_login_file_decides() {
    use LoginKind::*;
    for (own, key, want) in [
        (OAuth, false, (Kind::Subscription, "detected: oauth login")),
        (OAuth, true, (Kind::Subscription, "detected: oauth login")),
        (
            NoOAuth,
            false,
            (Kind::Platform, "detected: login file has no oauth login"),
        ),
        (
            NoOAuth,
            true,
            (Kind::Platform, "detected: login file has no oauth login"),
        ),
        (
            Unreadable,
            false,
            (Kind::Subscription, "detected: login file unreadable"),
        ),
        // An unreadable file is never proof of an API key, even with a key
        // setting present.
        (
            Unreadable,
            true,
            (Kind::Subscription, "detected: login file unreadable"),
        ),
    ] {
        let mut e = ev(own);
        e.key_setting = key;
        let (k, s) = detected(&e);
        assert_eq!((k, s.as_str()), want, "{own:?} key={key}");
    }
}

#[test]
fn rule_3_a_missing_login_that_is_live_uses_the_home_login() {
    use LoginKind::*;
    for (home, want) in [
        (
            OAuth,
            (
                Kind::Subscription,
                "detected: live in ~/.claude (oauth login)",
            ),
        ),
        (
            NoOAuth,
            (
                Kind::Platform,
                "detected: live in ~/.claude (no oauth login)",
            ),
        ),
        (
            Unreadable,
            (
                Kind::Subscription,
                "detected: live in ~/.claude (login file unreadable)",
            ),
        ),
        (
            Missing,
            (
                Kind::Subscription,
                "detected: live in ~/.claude (login file unreadable)",
            ),
        ),
    ] {
        let mut e = ev(Missing);
        e.home_login = Some(home);
        // Rule 3 comes before rule 4: a key setting does not change it.
        e.key_setting = true;
        let (k, s) = detected(&e);
        assert_eq!((k, s.as_str()), want, "{home:?}");
    }
}

#[test]
fn rule_3_is_ignored_when_the_profile_has_its_own_login() {
    let mut e = ev(LoginKind::OAuth);
    e.home_login = Some(LoginKind::NoOAuth);
    assert_eq!(detected(&e).0, Kind::Subscription);
}

#[test]
fn rule_4_a_missing_login_with_a_key_setting_is_an_api_key() {
    let mut e = ev(LoginKind::Missing);
    e.key_setting = true;
    let (k, s) = detected(&e);
    assert_eq!(
        (k, s.as_str()),
        (Kind::Platform, "detected: api key setting")
    );
}

#[test]
fn rule_5_otherwise_a_subscription_with_no_login() {
    let (k, s) = detected(&ev(LoginKind::Missing));
    assert_eq!((k, s.as_str()), (Kind::Subscription, "detected: no login"));
    assert!(!resolve_from(&ev(LoginKind::Missing)).own_oauth);
}

#[test]
fn an_override_beats_every_detection() {
    use LoginKind::*;
    for own in [OAuth, NoOAuth, Unreadable, Missing] {
        for k in [Kind::Subscription, Kind::Platform] {
            let mut e = ev(own);
            e.override_kind = Some(k);
            e.key_setting = true;
            let r = resolve_from(&e);
            assert_eq!(r.kind, k, "{own:?}");
            assert_eq!(r.source, KindSource::Override);
            assert_eq!(r.own_oauth, own == OAuth, "own_oauth is still recorded");
        }
    }
}

#[test]
fn nothing_but_no_oauth_a_key_setting_or_an_override_makes_an_api_key() {
    use LoginKind::*;
    for own in [OAuth, NoOAuth, Unreadable, Missing] {
        for home in [
            None,
            Some(OAuth),
            Some(NoOAuth),
            Some(Unreadable),
            Some(Missing),
        ] {
            for key in [false, true] {
                let e = Evidence {
                    override_kind: None,
                    own_login: own,
                    home_login: home,
                    key_setting: key,
                };
                let platform = resolve_from(&e).kind == Kind::Platform;
                let evidence = own == NoOAuth
                    || (own == Missing && home == Some(NoOAuth))
                    || (own == Missing && home.is_none() && key);
                assert_eq!(platform, evidence, "{own:?} {home:?} key={key}");
            }
        }
    }
}

#[test]
fn an_oauth_login_with_a_key_setting_warns_and_nothing_else_does() {
    let mut e = ev(LoginKind::OAuth);
    e.key_setting = true;
    assert!(resolve_from(&e).warn_key_setting());
    e.key_setting = false;
    assert!(!resolve_from(&e).warn_key_setting());
    let mut p = ev(LoginKind::NoOAuth);
    p.key_setting = true;
    assert!(!resolve_from(&p).warn_key_setting(), "an API-key profile");
    let mut o = ev(LoginKind::OAuth);
    o.key_setting = true;
    o.override_kind = Some(Kind::Subscription);
    assert!(
        !resolve_from(&o).warn_key_setting(),
        "an explicit kind silences it"
    );
}

// --- guard ------------------------------------------------------------------------------

fn resolved(e: Evidence) -> Resolved {
    resolve_from(&e)
}

#[test]
fn the_guard_refuses_a_subscription_profile_with_no_login_of_its_own() {
    use LoginKind::*;
    let name = n("old-team");
    for own in [Missing, Unreadable] {
        let err = guard(&resolved(ev(own)), &name, false).unwrap_err();
        assert!(
            matches!(err, ProfileError::SubscriptionNoLogin(ref p) if p == "old-team"),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "profile old-team is a subscription profile with no login of its own; launching it would bill the Claude Platform API key per token. Run 'amplihack profile login old-team', or run 'amplihack profile kind set old-team api-key' if per-token billing is intended."
        );
        assert!(!err.to_string().contains("platform"), "no internal name");
    }
    // A `{}` login overridden to subscription is refused too.
    let mut e = ev(NoOAuth);
    e.override_kind = Some(Kind::Subscription);
    assert!(guard(&resolved(e), &name, false).is_err());
}

#[test]
fn the_guard_allows_api_key_profiles_oauth_logins_and_held_logins() {
    use LoginKind::*;
    let name = n("p");
    assert!(guard(&resolved(ev(NoOAuth)), &name, false).is_ok());
    assert!(guard(&resolved(ev(OAuth)), &name, false).is_ok());
    assert!(guard(&resolved(ev(Missing)), &name, true).is_ok(), "held");
    let mut e = ev(Missing);
    e.override_kind = Some(Kind::Platform);
    assert!(guard(&resolved(e), &name, false).is_ok(), "override");
    let mut live = ev(Missing);
    live.home_login = Some(OAuth);
    assert!(
        guard(&resolved(live), &name, false).is_err(),
        "the ledger alone is not held: identify::live must say Known"
    );
    assert!(guard(&resolved(live), &name, true).is_ok());
}

// --- resolve on disk ---------------------------------------------------------------------

struct Env {
    _t: tempfile::TempDir,
    home: PathBuf,
    store: ProfileStore,
}

fn private(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
}

/// `personal` (OAuth), `console` (`{}`), `keyed` (no login file, a key in
/// its settings), `ghost` (no login file, nothing). `~/.claude` holds an
/// OAuth login.
fn env() -> Env {
    let t = tempfile::tempdir().unwrap();
    fs::set_permissions(t.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let home = t.path().join("home");
    let root = t.path().join("root");
    let store = ProfileStore::new(root.clone());
    for p in ["personal", "console", "keyed", "ghost"] {
        store.add(&n(p)).unwrap();
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    private(&root.join("personal/.credentials.json"), OAUTH);
    private(&root.join("console/.credentials.json"), "{}");
    private(
        &root.join("keyed/settings.json"),
        r#"{"apiKeyHelper":"echo CANARY-LEAK-PROBE"}"#,
    );
    private(&home.join(".claude/.credentials.json"), OAUTH);
    Env { _t: t, home, store }
}

impl Env {
    fn kind(&self, p: &str) -> (Kind, String) {
        let r = resolve(&self.store, &n(p), &self.home);
        (r.kind, r.source.to_string())
    }

    fn toggle(&self, body: &str) {
        private(&self.store.root().join(".auto-switch"), body);
    }
}

#[test]
fn resolve_reads_each_profile() {
    let e = env();
    assert_eq!(
        e.kind("personal"),
        (Kind::Subscription, "detected: oauth login".into())
    );
    assert_eq!(
        e.kind("console"),
        (
            Kind::Platform,
            "detected: login file has no oauth login".into()
        )
    );
    assert_eq!(
        e.kind("keyed"),
        (Kind::Platform, "detected: api key setting".into())
    );
    assert_eq!(
        e.kind("ghost"),
        (Kind::Subscription, "detected: no login".into())
    );
}

#[test]
fn the_kind_alone_matches_resolve_and_skips_key_names_it_does_not_need() {
    let e = env();
    // `personal` has an OAuth login and also names a key: `resolve` reports
    // the setting, the kind-only path does not read it.
    private(
        &e.store.root().join("personal/settings.json"),
        r#"{"apiKeyHelper":"echo CANARY-LEAK-PROBE"}"#,
    );
    let ctx = Ctx::load(&e.store, Some(&e.home));
    for p in ["personal", "console", "keyed", "ghost"] {
        let full = ctx.resolve(&e.store, &n(p));
        let lean = ctx.resolve_kind(&e.store, &n(p));
        assert_eq!(ctx.kind(&e.store, &n(p)), full.kind, "{p}");
        assert_eq!(
            (lean.kind, lean.source, lean.own_oauth),
            (full.kind, full.source, full.own_oauth),
            "{p}"
        );
        assert_eq!(lean.may_be_live(), full.may_be_live(), "{p}");
    }
    assert!(ctx.resolve(&e.store, &n("personal")).key_setting);
    assert!(
        ctx.resolve_kind(&e.store, &n("keyed")).key_setting,
        "rule 4 still reads it"
    );
}

#[test]
fn resolve_reads_overrides_from_the_toggle_file() {
    let e = env();
    e.toggle("enabled=off\nkind=ghost:api-key,console:subscription\n");
    assert_eq!(e.kind("ghost"), (Kind::Platform, "override".into()));
    assert_eq!(e.kind("console"), (Kind::Subscription, "override".into()));
    assert_eq!(e.kind("personal").1, "detected: oauth login");
}

#[test]
fn a_corrupt_toggle_file_fails_closed() {
    let e = env();
    e.toggle("enabled=off\nkind=ghost:platform,ghost:platform\n");
    // No override is trusted from a corrupt file: ghost stays a subscription
    // profile with no login, and the guard refuses it.
    let r = resolve(&e.store, &n("ghost"), &e.home);
    assert_eq!(r.kind, Kind::Subscription);
    assert!(guard(&r, &n("ghost"), false).is_err());
}

#[test]
fn an_unknown_kind_makes_the_toggle_file_corrupt() {
    // A typo such as `apikey` must not silently drop the override and leave
    // the profile to detection: the whole file fails closed, as for a
    // duplicate name, so `kind set` and the guard report it.
    let e = env();
    for body in [
        "enabled=off\nkind=console:enterprise\n",
        "enabled=off\nkind=ghost:platform,console:apikey\n",
    ] {
        e.toggle(body);
        assert!(
            matches!(config::read(e.store.root()), FileState::Corrupt),
            "{body:?}"
        );
        let r = resolve(&e.store, &n("ghost"), &e.home);
        assert_eq!(
            (r.kind, r.source.to_string()),
            (Kind::Subscription, "detected: no login".into()),
            "{body:?}"
        );
    }
}

#[test]
fn resolve_uses_the_home_login_when_the_ledger_names_the_profile_live() {
    let e = env();
    let dir = e.store.root().join(".auto-switch.d/rotate");
    for d in [dir.parent().unwrap(), dir.as_path()] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).unwrap();
    }
    private(&dir.join("ledger.json"), r#"{"version":1,"live":"ghost"}"#);
    assert_eq!(
        e.kind("ghost"),
        (
            Kind::Subscription,
            "detected: live in ~/.claude (oauth login)".into()
        )
    );
    private(&e.home.join(".claude/.credentials.json"), "{}");
    assert_eq!(
        e.kind("ghost"),
        (
            Kind::Platform,
            "detected: live in ~/.claude (no oauth login)".into()
        )
    );
}

#[test]
fn platform_profiles_are_sorted_and_follow_overrides() {
    let e = env();
    assert_eq!(
        platform_profiles(&e.store, &e.home),
        vec![n("console"), n("keyed")]
    );
    e.toggle("enabled=off\nkind=keyed:subscription,ghost:platform\n");
    assert_eq!(
        platform_profiles(&e.store, &e.home),
        vec![n("console"), n("ghost")]
    );
}

// --- tiers ----------------------------------------------------------------------------------

#[test]
fn tiers_strip_api_key_profiles_from_overflow_and_append_the_first_at_tier_10() {
    let e = env();
    e.toggle("enabled=off\noverflow=console,personal:2,keyed:3\n");
    assert_eq!(
        tiers(&e.store, &e.home),
        vec![
            OverflowEntry::new(n("personal"), 2),
            OverflowEntry::platform(n("console")),
        ],
        "keyed is an API-key profile too, but only the first is used"
    );
    assert_eq!(tiers(&e.store, &e.home).last().unwrap().tier, PLATFORM_TIER);
}

#[test]
fn an_api_key_profile_that_overflow_does_not_name_is_never_a_tier() {
    // Issue #1804: console and keyed are both API-key profiles, and neither
    // is marked. Before, the first was added implicitly.
    let e = env();
    e.toggle("enabled=off\noverflow=personal\n");
    assert_eq!(
        tiers(&e.store, &e.home),
        vec![OverflowEntry::new(n("personal"), 1)],
        "no implicit API-key tier"
    );
    assert!(
        tiers(&e.store, &e.home)
            .iter()
            .all(|t| t.tier != PLATFORM_TIER)
    );
}

#[test]
fn with_no_toggle_file_there_is_no_api_key_tier() {
    let e = env();
    assert!(
        tiers(&e.store, &e.home).is_empty(),
        "a missing .auto-switch marks nothing"
    );
}

#[test]
fn a_marked_api_key_profile_is_the_last_tier_whatever_tier_is_written() {
    let e = env();
    // keyed is written at tier 1 (bare) and listed first; it is still the
    // API-key tier, after personal at tier 2.
    e.toggle("enabled=off\noverflow=keyed,personal:2\n");
    assert_eq!(
        tiers(&e.store, &e.home),
        vec![
            OverflowEntry::new(n("personal"), 2),
            OverflowEntry::platform(n("keyed")),
        ]
    );
}

#[test]
fn only_the_first_marked_api_key_profile_by_name_is_used() {
    let e = env();
    // keyed is listed first, but console comes first by name.
    e.toggle("enabled=off\noverflow=keyed,console\n");
    assert_eq!(
        tiers(&e.store, &e.home),
        vec![OverflowEntry::platform(n("console"))]
    );
    // Marking only keyed uses keyed, though console exists.
    e.toggle("enabled=off\noverflow=keyed\n");
    assert_eq!(
        tiers(&e.store, &e.home),
        vec![OverflowEntry::platform(n("keyed"))]
    );
}

#[test]
fn tiers_with_no_api_key_profile_are_the_overflow_entries() {
    let e = env();
    e.toggle("enabled=off\noverflow=personal\nkind=console:subscription,keyed:subscription\n");
    assert_eq!(
        tiers(&e.store, &e.home),
        vec![OverflowEntry::new(n("personal"), 1)]
    );
}

#[test]
fn tiers_from_a_corrupt_toggle_file_never_spend() {
    let e = env();
    e.toggle("enabled=off\noverflow=personal\noverflow=personal\n");
    assert!(
        tiers(&e.store, &e.home).is_empty(),
        "a damaged file never spends, on credits or on the API key"
    );
}

// --- The kind= line --------------------------------------------------------------------------

#[test]
fn the_kind_line_round_trips_after_overflow_and_is_left_out_when_empty() {
    let e = env();
    e.toggle("enabled=on\nthreshold=80\nexclude=\noverflow=personal\nkind=console:api-key,ghost:subscription\n");
    let FileState::Valid(mut c) = config::read(e.store.root()) else {
        panic!("valid");
    };
    assert_eq!(c.kind.get(&n("console")), Some(&Kind::Platform));
    assert_eq!(c.kind.get(&n("ghost")), Some(&Kind::Subscription));
    assert_eq!(
        config::render(&c),
        "enabled=on\nthreshold=80\nexclude=\noverflow=personal\nkind=console:platform,ghost:subscription\n"
    );
    c.kind.clear();
    assert_eq!(
        config::render(&c),
        "enabled=on\nthreshold=80\nexclude=\noverflow=personal\n"
    );
}
