//! A profile's kind (issue #1700): a subscription profile, which runs on a
//! claude.ai OAuth login billed by plan, or an API-key profile, which runs
//! on the Claude Platform API key billed per token. See "API-key profiles"
//! in docs/reference/profile-command.md.
//!
//! The kind is worked out again on every read and never stored. The
//! evidence comes from `rotate::identify` (key names and file presence
//! only) and from the `kind=` overrides in `<root>/.auto-switch`;
//! [`resolve_from`] is the pure rule table. Every unsure case is a
//! subscription profile with no login, which [`guard`] refuses to launch,
//! so a wrong guess refuses a launch instead of billing per token.

use super::ProfileError;
use super::auto_switch::config::{self, FileState, ReadMode};
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
    /// Detected; the text is one of the reasons in [`resolve_from`].
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
}

impl Resolved {
    /// An OAuth subscription profile that also names an API-key setting:
    /// Claude may prefer the key. An explicit kind silences it.
    pub fn warn_key_setting(&self) -> bool {
        self.source != KindSource::Override
            && self.kind == Kind::Subscription
            && self.own_oauth
            && self.key_setting
    }

    /// Whether the profile's login file may be moved into `~/.claude`: an
    /// OAuth login, or any login file of an API-key profile.
    pub fn may_be_live(&self) -> bool {
        self.kind == Kind::Platform || self.own_oauth
    }
}

const OAUTH_LOGIN: &str = "oauth login";
const NO_OAUTH_LOGIN: &str = "login file has no oauth login";
const LOGIN_UNREADABLE: &str = "login file unreadable";
const LIVE_OAUTH: &str = "live in ~/.claude (oauth login)";
const LIVE_NO_OAUTH: &str = "live in ~/.claude (no oauth login)";
const LIVE_UNREADABLE: &str = "live in ~/.claude (login file unreadable)";
const API_KEY_SETTING: &str = "api key setting";
const NO_LOGIN: &str = "no login";

/// The documented rule table; the first rule that matches wins:
/// 1. an override;
/// 2. the profile's own login file, when it is there;
/// 3. when it is missing and the profile is live, the login in `~/.claude`;
/// 4. when it is missing, an API-key setting name;
/// 5. otherwise a subscription profile with no login.
///
/// An unreadable file is never proof of an API key.
pub fn resolve_from(e: &Evidence) -> Resolved {
    use LoginKind::*;
    let own_oauth = e.own_login == OAuth;
    let done = |kind, source| Resolved {
        kind,
        source,
        own_oauth,
        key_setting: e.key_setting,
    };
    if let Some(k) = e.override_kind {
        return done(k, KindSource::Override);
    }
    let (kind, why) = match (e.own_login, e.home_login) {
        (OAuth, _) => (Kind::Subscription, OAUTH_LOGIN),
        (NoOAuth, _) => (Kind::Platform, NO_OAUTH_LOGIN),
        (Unreadable, _) => (Kind::Subscription, LOGIN_UNREADABLE),
        (Missing, Some(OAuth)) => (Kind::Subscription, LIVE_OAUTH),
        (Missing, Some(NoOAuth)) => (Kind::Platform, LIVE_NO_OAUTH),
        (Missing, Some(Unreadable | Missing)) => (Kind::Subscription, LIVE_UNREADABLE),
        (Missing, None) if e.key_setting => (Kind::Platform, API_KEY_SETTING),
        (Missing, None) => (Kind::Subscription, NO_LOGIN),
    };
    done(kind, KindSource::Detected(why))
}

/// The launch guard's decision: a subscription profile without its own
/// OAuth login is refused unless its login is `held` for it elsewhere
/// (identified as the rotator's live login, or leased or hosted).
pub fn guard(r: &Resolved, name: &ProfileName, held: bool) -> Result<(), ProfileError> {
    if r.kind == Kind::Subscription && !r.own_oauth && !held {
        return Err(ProfileError::SubscriptionNoLogin(name.to_string()));
    }
    Ok(())
}

/// What kind resolution reads once per command: the overrides, the rotate
/// ledger's `live` and the login in `~/.claude`.
pub struct Ctx {
    /// `None` when `.auto-switch` is corrupt: no override is trusted.
    overrides: Option<BTreeMap<ProfileName, Kind>>,
    ledger_live: Option<ProfileName>,
    /// `~/.claude/.credentials.json`; read only when the ledger names a
    /// live profile. `Missing` without a home.
    home_login: Option<LoginKind>,
}

impl Ctx {
    /// Reads the overrides and the rotate ledger under the store's root, and
    /// the login in `<home>/.claude` when the ledger names a live profile.
    pub fn load(store: &ProfileStore, home: Option<&Path>) -> Self {
        let overrides = match config::read(store.root()) {
            FileState::Valid(c) => Some(c.kind),
            FileState::Missing => Some(BTreeMap::new()),
            FileState::Corrupt => None,
        };
        // An unreadable ledger names no live profile: rule 3 then does not
        // apply, and rules 4 and 5 still never make a guess that bills.
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
        }
    }

    /// `.auto-switch` is unreadable or invalid.
    pub fn corrupt(&self) -> bool {
        self.overrides.is_none()
    }

    pub fn evidence(&self, store: &ProfileStore, name: &ProfileName) -> Evidence {
        self.evidence_with(store, name, true)
    }

    /// The evidence for `name`. With `all_keys` false, the API-key setting
    /// names are read only when the kind depends on them (rule 4), since
    /// that means parsing `.claude.json`, which may be large.
    fn evidence_with(&self, store: &ProfileStore, name: &ProfileName, all_keys: bool) -> Evidence {
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
        let needed = all_keys
            || (e.override_kind.is_none()
                && e.own_login == LoginKind::Missing
                && e.home_login.is_none());
        e.key_setting = needed
            && dir
                .as_ref()
                .is_some_and(|d| identify::api_key_setting_names(d));
        e
    }

    pub fn resolve(&self, store: &ProfileStore, name: &ProfileName) -> Resolved {
        resolve_from(&self.evidence(store, name))
    }

    /// [`Self::resolve`] without reading the API-key setting names when the
    /// overrides or logins decide the kind: `kind`, `source`, `own_oauth`
    /// and [`Resolved::may_be_live`] are the same, but `key_setting` may be
    /// false where it is true, so not for [`Resolved::warn_key_setting`].
    pub(super) fn resolve_kind(&self, store: &ProfileStore, name: &ProfileName) -> Resolved {
        resolve_from(&self.evidence_with(store, name, false))
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
        let mut out: Vec<ProfileName> = store
            .list()
            .ok()?
            .iter()
            .filter_map(|n| ProfileName::parse(n).ok())
            .filter(|n| self.kind(store, n) == Kind::Platform)
            .collect();
        out.sort();
        Some(out)
    }
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
/// its own OAuth login, since no override can then be trusted. `held` is
/// called only when a refusal would otherwise follow.
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
            config::update(store.root(), ReadMode::Editable, |c| {
                had = c.kind.get(&name).copied();
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
                        println!(
                            "note: {name} is billed per token on the Claude Platform API key, which is stored outside the profile directory; the rotator uses it only after every plan and credit profile."
                        );
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
