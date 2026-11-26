//! A profile's kind (issue #1700): a subscription profile, which runs on a
//! claude.ai OAuth login billed by plan, or an API-key profile, which runs
//! on the Claude Platform API key billed per token. See "API-key profiles"
//! in docs/reference/profile-command.md.
//!
//! The kind is worked out again on every read and never stored. The
//! evidence comes from `rotate::identify` (key names and file presence
//! only) and from the `kind=` overrides in `<root>/.auto-switch`;
//! [`resolve_with`] is the pure rule table. Every unsure case is a
//! subscription profile with no login, which [`guard`] refuses to launch,
//! so a wrong guess refuses a launch instead of billing per token.
//!
//! Issue #2885: on macOS a profile's login is in the login Keychain and
//! there is no login file. [`Ctx`] asks `keychain::detect` about such a
//! profile, for every caller alike, so the launch guard, `kind list`,
//! auto-switch and the rotator see one kind. A Keychain that cannot be read
//! is a subscription profile with no login, never an API-key profile.

use super::ProfileError;
use super::auto_switch::config::{self, FileState, ReadMode};
use super::keychain::{self, Keychain};
use super::name::ProfileName;
use super::rotate::identify::{self, LoginKind};
use super::rotate::ledger::RotateState;
use super::store::ProfileStore;
use std::collections::BTreeMap;
use std::path::Path;

/// A profile's kind. The API-key kind is `platform` inside amplihack and
/// in `.auto-switch`; every command prints it as `api-key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Subscription,
    Platform,
}

impl Kind {
    /// The label every command prints.
    pub fn label(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Platform => "api-key",
        }
    }

    /// The spelling written to `.auto-switch`.
    pub fn stored(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::Platform => "platform",
        }
    }

    /// `subscription`, `platform` or `api-key`; nothing else.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "subscription" => Some(Self::Subscription),
            "platform" | "api-key" => Some(Self::Platform),
            _ => None,
        }
    }
}

/// Where a kind came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindSource {
    /// A `kind=` entry in `.auto-switch`.
    Override,
    /// Detected; the text is one of the reasons in [`resolve_with`].
    Detected(&'static str),
}

impl std::fmt::Display for KindSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Override => f.write_str("override"),
            Self::Detected(why) => write!(f, "detected: {why}"),
        }
    }
}

/// What a profile's kind is decided from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evidence {
    /// The `kind=` entry for the profile, if any.
    pub override_kind: Option<Kind>,
    /// `<root>/<name>/.credentials.json`.
    pub own_login: LoginKind,
    /// `~/.claude/.credentials.json`, only when the rotate ledger names the
    /// profile as live.
    pub home_login: Option<LoginKind>,
    /// The profile names an API-key setting.
    pub key_setting: bool,
}

/// A profile's resolved kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub kind: Kind,
    pub source: KindSource,
    /// The profile's own login file is an OAuth login.
    pub own_oauth: bool,
    /// The profile names an API-key setting.
    pub key_setting: bool,
    /// What the macOS Keychain said about the profile's directory; `None`
    /// when it was not asked (off macOS, a login file is there, an `api-key`
    /// override, or no identifiable account). Apart from `own_oauth`, which
    /// is about the login file alone.
    pub keychain: Option<Keychain>,
}

impl Resolved {
    /// The profile's own login is an item in the macOS Keychain.
    pub fn keychain_login(&self) -> bool {
        self.keychain == Some(Keychain::Present)
    }

    /// An OAuth subscription profile that also names an API-key setting:
    /// Claude may prefer the key. An explicit kind silences it. A login in
    /// the Keychain counts as a login file does.
    pub fn warn_key_setting(&self) -> bool {
        self.source != KindSource::Override
            && self.kind == Kind::Subscription
            && (self.own_oauth || self.keychain_login())
            && self.key_setting
    }

    /// Whether the profile's login file may be moved into `~/.claude`: an
    /// OAuth login, or any login file of an API-key profile. A Keychain
    /// login has no file to move, so it does not count here.
    pub fn may_be_live(&self) -> bool {
        self.kind == Kind::Platform || self.own_oauth
    }
}

const OAUTH_LOGIN: &str = "oauth login";
const NO_OAUTH_LOGIN: &str = "login file has no oauth login";
const LOGIN_UNREADABLE: &str = "login file unreadable";
const KEYCHAIN_LOGIN: &str = "oauth login in the macOS Keychain";
const KEYCHAIN_UNREADABLE: &str = "macOS Keychain unreadable";
const LIVE_OAUTH: &str = "live in ~/.claude (oauth login)";
const LIVE_NO_OAUTH: &str = "live in ~/.claude (no oauth login)";
const LIVE_UNREADABLE: &str = "live in ~/.claude (login file unreadable)";
const API_KEY_SETTING: &str = "api key setting";
const NO_LOGIN: &str = "no login";

/// The documented rule table; the first rule that matches wins:
/// 1. an override;
/// 2. the profile's own login file, when it is there;
/// 3. when it is missing and the macOS Keychain was asked: an item there is
///    the profile's own OAuth login, and a Keychain that could not be read
///    is a subscription profile with no login, whatever else is set;
/// 4. when it is missing and the profile is live, the login in `~/.claude`;
/// 5. when it is missing, an API-key setting name;
/// 6. otherwise a subscription profile with no login.
///
/// `keychain` is `None` when the Keychain was not asked, and an absent item
/// decides nothing: both go on to rule 4. An answer counts only with the
/// login file missing, since rule 2 comes first.
///
/// An unreadable file or Keychain is never proof of an API key.
pub fn resolve_with(e: &Evidence, keychain: Option<Keychain>) -> Resolved {
    use LoginKind::*;
    let own_oauth = e.own_login == OAuth;
    let keychain = keychain.filter(|_| e.own_login == Missing);
    let done = |kind, source| Resolved {
        kind,
        source,
        own_oauth,
        key_setting: e.key_setting,
        keychain,
    };
    if let Some(k) = e.override_kind {
        return done(k, KindSource::Override);
    }
    let (kind, why) = match (e.own_login, keychain, e.home_login) {
        (OAuth, ..) => (Kind::Subscription, OAUTH_LOGIN),
        (NoOAuth, ..) => (Kind::Platform, NO_OAUTH_LOGIN),
        (Unreadable, ..) => (Kind::Subscription, LOGIN_UNREADABLE),
        (Missing, Some(Keychain::Present), _) => (Kind::Subscription, KEYCHAIN_LOGIN),
        (Missing, Some(Keychain::Unreadable(_)), _) => (Kind::Subscription, KEYCHAIN_UNREADABLE),
        (Missing, Some(Keychain::Absent) | None, home) => match home {
            Some(OAuth) => (Kind::Subscription, LIVE_OAUTH),
            Some(NoOAuth) => (Kind::Platform, LIVE_NO_OAUTH),
            Some(Unreadable | Missing) => (Kind::Subscription, LIVE_UNREADABLE),
            None if e.key_setting => (Kind::Platform, API_KEY_SETTING),
            None => (Kind::Subscription, NO_LOGIN),
        },
    };
    done(kind, KindSource::Detected(why))
}

/// [`resolve_with`] when the Keychain was not asked: the rule table as it is
/// off macOS, with the same nine sources as before issue #2885. Kept for the
/// tests that pin that table; [`Ctx`] always goes through [`resolve_with`].
#[cfg(test)]
pub fn resolve_from(e: &Evidence) -> Resolved {
    resolve_with(e, None)
}

/// The launch guard's decision for a subscription profile without an OAuth
/// login file. In order:
/// - its login is `held` for it elsewhere (identified as the rotator's live
///   login, or leased or hosted): allowed;
/// - an item in the macOS Keychain: allowed;
/// - a Keychain that could not be read: refused as
///   [`ProfileError::KeychainUnreadable`], which is not a guess either way;
/// - otherwise refused as [`ProfileError::SubscriptionNoLogin`].
pub fn guard(r: &Resolved, name: &ProfileName, held: bool) -> Result<(), ProfileError> {
    if r.kind != Kind::Subscription || r.own_oauth || held {
        return Ok(());
    }
    match r.keychain {
        Some(Keychain::Present) => Ok(()),
        Some(Keychain::Unreadable(cause)) => Err(ProfileError::KeychainUnreadable {
            name: name.to_string(),
            cause,
        }),
        Some(Keychain::Absent) | None => Err(ProfileError::SubscriptionNoLogin(name.to_string())),
    }
}

/// What kind resolution reads once per command: the overrides, the rotate
/// ledger's `live` and the login in `~/.claude`. On macOS it also holds what
/// a Keychain lookup runs in; the lookups themselves are made per profile.
pub struct Ctx {
    /// `None` when `.auto-switch` is corrupt: no override is trusted.
    overrides: Option<BTreeMap<ProfileName, Kind>>,
    ledger_live: Option<ProfileName>,
    /// `~/.claude/.credentials.json`; read only when the ledger names a
    /// live profile. `Missing` without a home.
    home_login: Option<LoginKind>,
    /// The platform, and the home a Keychain lookup is given as its `HOME`.
    keychain: keychain::Env,
}

impl Ctx {
    /// Reads the overrides and the rotate ledger under the store's root, and
    /// the login in `<home>/.claude` when the ledger names a live profile.
    /// Nothing is asked of the Keychain here. On macOS, without a `home`, a
    /// profile that would be asked about resolves as Keychain unreadable.
    pub fn load(store: &ProfileStore, home: Option<&Path>) -> Self {
        let overrides = match config::read(store.root()) {
            FileState::Valid(c) => Some(c.kind),
            FileState::Missing => Some(BTreeMap::new()),
            FileState::Corrupt => None,
        };
        // An unreadable ledger names no live profile: rule 4 then does not
        // apply, and rules 5 and 6 still never make a guess that bills.
        let ledger_live = RotateState::new(store.root())
            .load_ledger()
            .ok()
            .flatten()
            .and_then(|l| l.live)
            .and_then(|n| ProfileName::parse(&n).ok());
        let home_login = ledger_live.as_ref().map(|_| match home {
            Some(h) => identify::login_kind(&h.join(".claude").join(".credentials.json")),
            None => LoginKind::Missing,
        });
        Self {
            overrides,
            ledger_live,
            home_login,
            keychain: keychain::Env::ambient(home),
        }
    }

    /// Whether this platform keeps logins in the macOS Keychain, so that a
    /// missing login file alone says nothing about a profile.
    pub fn macos(&self) -> bool {
        self.keychain.macos()
    }

    /// The profile the rotate ledger names as live, if any.
    pub fn ledger_live(&self) -> Option<&ProfileName> {
        self.ledger_live.as_ref()
    }

    /// `.auto-switch` is unreadable or invalid.
    pub fn corrupt(&self) -> bool {
        self.overrides.is_none()
    }

    /// The evidence for `name`, and what the Keychain said about it.
    ///
    /// The Keychain is asked (rule 3) only when nothing before it has
    /// decided: the profile's directory resolves, no override says
    /// `api-key`, and there is no login file. `keychain::detect` then adds
    /// its own two conditions, macOS and an identifiable account. A
    /// `subscription` override keeps its kind and is still asked, because
    /// the guard needs to know whether the profile has a login.
    ///
    /// With `all_keys` false, the API-key setting names are read only when
    /// the kind depends on them (rule 5), since that means parsing
    /// `.claude.json`, which may be large.
    fn gather(
        &self,
        store: &ProfileStore,
        name: &ProfileName,
        all_keys: bool,
    ) -> (Evidence, Option<Keychain>) {
        let dir = store.resolve(name).ok();
        let mut e = Evidence {
            override_kind: self.overrides.as_ref().and_then(|o| o.get(name).copied()),
            own_login: dir.as_ref().map_or(LoginKind::Missing, |d| {
                identify::login_kind(&d.join(".credentials.json"))
            }),
            home_login: (self.ledger_live.as_ref() == Some(name))
                .then_some(self.home_login)
                .flatten(),
            key_setting: false,
        };
        let ask = e.override_kind != Some(Kind::Platform) && e.own_login == LoginKind::Missing;
        let keychain = match &dir {
            Some(d) if ask => keychain::detect(&self.keychain, d),
            _ => None,
        };
        let needed = all_keys
            || (e.override_kind.is_none()
                && e.own_login == LoginKind::Missing
                && e.home_login.is_none()
                && matches!(keychain, None | Some(Keychain::Absent)));
        e.key_setting = needed
            && dir
                .as_ref()
                .is_some_and(|d| identify::api_key_setting_names(d));
        (e, keychain)
    }

    pub fn resolve(&self, store: &ProfileStore, name: &ProfileName) -> Resolved {
        let (e, keychain) = self.gather(store, name, true);
        resolve_with(&e, keychain)
    }

    /// [`Self::resolve`] without reading the API-key setting names when the
    /// overrides, logins or Keychain decide the kind: `kind`, `source`,
    /// `own_oauth`, `keychain` and [`Resolved::may_be_live`] are the same,
    /// but `key_setting` may be false where it is true, so not for
    /// [`Resolved::warn_key_setting`].
    pub fn resolve_kind(&self, store: &ProfileStore, name: &ProfileName) -> Resolved {
        let (e, keychain) = self.gather(store, name, false);
        resolve_with(&e, keychain)
    }

    /// `name`'s kind alone; see [`Self::resolve_kind`].
    pub fn kind(&self, store: &ProfileStore, name: &ProfileName) -> Kind {
        self.resolve_kind(store, name).kind
    }

    /// Every API-key profile, sorted by name. A root that cannot be listed
    /// has none.
    pub fn platform_profiles(&self, store: &ProfileStore) -> Vec<ProfileName> {
        self.try_platform_profiles(store).unwrap_or_default()
    }

    /// [`Self::platform_profiles`], or `None` when the root cannot be
    /// listed, for callers that must not read that as "no API-key profile"
    /// (issue #1804).
    pub fn try_platform_profiles(&self, store: &ProfileStore) -> Option<Vec<ProfileName>> {
        let all = self.try_resolve_all(store)?;
        Some(platforms_of(&all))
    }

    /// Every profile with its kind as [`Self::resolve_kind`] gives it,
    /// sorted by name, or `None` when the root cannot be listed. Each profile
    /// is resolved once, so a caller that needs both the API-key profiles
    /// ([`platforms_of`]) and each login's kind reads the files once.
    pub fn try_resolve_all(&self, store: &ProfileStore) -> Option<Vec<(ProfileName, Resolved)>> {
        let mut out: Vec<(ProfileName, Resolved)> = store
            .list()
            .ok()?
            .iter()
            .filter_map(|n| ProfileName::parse(n).ok())
            .map(|n| {
                let r = self.resolve_kind(store, &n);
                (n, r)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Some(out)
    }
}

/// The API-key profiles of `all`, in its order.
pub fn platforms_of(all: &[(ProfileName, Resolved)]) -> Vec<ProfileName> {
    all.iter()
        .filter(|(_, r)| r.kind == Kind::Platform)
        .map(|(n, _)| n.clone())
        .collect()
}

/// What `kind set <name> api-key` prints: the cost, and that the override
/// alone does not put the profile to use (issue #1804).
pub(super) fn platform_note(name: &ProfileName) -> String {
    format!(
        "note: {name} is billed per token on the Claude Platform API key, which is stored outside the profile directory; the rotator and auto-switch use it only if you mark it with 'amplihack profile overflow add {name}', and then only after every plan and credit profile."
    )
}

/// Printed when `kind set <name> api-key` meets an existing `overflow=` mark.
/// That mark may have been given for a credit account with a spend limit. On
/// an API-key profile the same mark is the consent to per-token billing at
/// tier 10, so the change is said out loud (issue #1804).
pub(super) fn mark_became_key_consent(name: &ProfileName) -> String {
    format!(
        "warning: {name} is already marked in overflow=; on an API-key profile that mark allows per-token billing on the API key, at tier 10, with no spend limit. If you did not mean that, run: amplihack profile overflow remove {name}"
    )
}

/// `name`'s kind under `store`, with `home` the user's home directory.
#[cfg(test)]
pub fn resolve(store: &ProfileStore, name: &ProfileName, home: &Path) -> Resolved {
    Ctx::load(store, Some(home)).resolve(store, name)
}

/// `name`'s kind under `store`; see [`Ctx::kind`].
pub fn kind_of(store: &ProfileStore, name: &ProfileName, home: &Path) -> Kind {
    Ctx::load(store, Some(home)).kind(store, name)
}

/// Every API-key profile under `store`, sorted by name.
pub fn platform_profiles(store: &ProfileStore, home: &Path) -> Vec<ProfileName> {
    Ctx::load(store, Some(home)).platform_profiles(store)
}

/// The launch guard for `name` (`profile run`, `launch`, `claude` and
/// `switch`): refuses a subscription profile without its own OAuth login
/// unless `held`, and, while `.auto-switch` is corrupt, any profile without
/// its own OAuth login, since no override can then be trusted. A login in
/// the macOS Keychain is the profile's own OAuth login in both cases. `held`
/// is called only when a refusal would otherwise follow; the Keychain has
/// been asked by then, as part of resolving the kind.
pub fn launch_guard(
    store: &ProfileStore,
    name: &ProfileName,
    home: Option<&Path>,
    held: impl FnOnce() -> bool,
) -> Result<(), ProfileError> {
    let ctx = Ctx::load(store, home);
    let r = ctx.resolve_kind(store, name);
    // With no override to trust, every profile is checked as a
    // subscription profile.
    let checked = if ctx.corrupt() {
        Resolved {
            kind: Kind::Subscription,
            ..r
        }
    } else {
        r
    };
    if guard(&checked, name, false).is_err() {
        guard(&checked, name, held())?;
    }
    Ok(())
}

/// The one-line warning for an OAuth subscription profile that also names
/// an API-key setting.
pub fn key_setting_warning(name: &ProfileName) -> String {
    format!(
        "warning: profile {name} has an oauth login and an API-key setting is also configured; \
         Claude may bill per token. Set the kind to confirm: amplihack profile kind set {name} subscription"
    )
}

/// The note `profile login` prints for an API-key profile.
pub const LOGIN_NOTE: &str = "note: this profile uses a Claude Platform credential stored outside the profile directory, which amplihack does not manage.";

#[derive(Debug, clap::Subcommand)]
pub enum KindCommand {
    /// List each profile's kind (subscription or api-key) and where it came from
    List,
    /// Override a profile's kind; `auto` removes the override
    Set {
        name: String,
        #[arg(value_parser = ["subscription", "api-key", "platform", "auto"])]
        kind: String,
    },
}

pub fn run(
    store: &ProfileStore,
    cmd: KindCommand,
    home: Option<&Path>,
) -> Result<(), ProfileError> {
    match cmd {
        KindCommand::List => {
            let ctx = Ctx::load(store, home);
            if ctx.corrupt() {
                eprintln!(
                    "warning: {} is unreadable or invalid; no kind override is used",
                    config::FILE
                );
            }
            let names: Vec<ProfileName> = store
                .list()?
                .iter()
                .filter_map(|n| ProfileName::parse(n).ok())
                .collect();
            let width = names.iter().map(|n| n.as_str().len()).max().unwrap_or(0);
            for n in &names {
                let r = ctx.resolve(store, n);
                println!(
                    "{:<width$}  {:<12}  ({})",
                    n.as_str(),
                    r.kind.label(),
                    r.source
                );
            }
        }
        KindCommand::Set { name, kind } => {
            let name = ProfileName::parse(&name)?;
            let want = Kind::parse(&kind);
            if want.is_some() {
                // An existing profile directory, not a symlink. `auto`
                // accepts any valid name, so a stale override can go.
                store.resolve(&name)?;
            }
            let mut had = None;
            let mut marked = false;
            config::update(store.root(), ReadMode::Editable, |c| {
                had = c.kind.get(&name).copied();
                marked = c.overflow.iter().any(|e| e.name == name);
                match want {
                    Some(k) => {
                        c.kind.insert(name.clone(), k);
                    }
                    None => {
                        c.kind.remove(&name);
                    }
                }
                Ok(())
            })?;
            match want {
                Some(k) => {
                    println!("kind: {name} is {} (override)", k.label());
                    if k == Kind::Platform {
                        println!("{}", platform_note(&name));
                        if marked && had != Some(Kind::Platform) {
                            eprintln!("{}", mark_became_key_consent(&name));
                        }
                    }
                    if let Some(pid) = RotateState::new(store.root()).foreign_rotator() {
                        eprintln!(
                            "warning: the running rotator (pid {pid}) is from an older amplihack and will read <root>/.auto-switch as corrupt; upgrade it, then run 'amplihack profile rotate stop' and 'amplihack profile rotate start'"
                        );
                    }
                }
                None if had.is_some() => {
                    let r = Ctx::load(store, home).resolve(store, &name);
                    println!(
                        "kind: removed the override on {name}; detected {} ({})",
                        r.kind.label(),
                        r.source
                    );
                }
                None => println!("kind: {name} has no override"),
            }
        }
    }
    Ok(())
}
