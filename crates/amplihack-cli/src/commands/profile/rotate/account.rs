//! An account: as `.claude.json` files name it (moved from `identify`, which
//! re-exports it), or as the OAuth profile endpoint reports it for a token
//! ([`Account::from_profile`]). Issue #1886: hand-change recovery and
//! `profile verify` decide whose login a folder holds only from the server's
//! answer, never from `.claude.json`; the swap, duplicate and login checks,
//! the adopt rules and display still read `.claude.json`.

use super::ledger::safe_account;
use crate::commands::profile::auto_switch::kit::read_claude_json;
use std::path::Path;

/// An `oauthAccount`: its user (`accountUuid`), its organization
/// (`organizationUuid`) and its e-mail (lower-cased). `Debug` hides all
/// three. [`Account::same`] is the only identity comparison in amplihack.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    uuid: Option<String>,
    org: Option<String>,
    email: Option<String>,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Account(..)")
    }
}

impl Account {
    #[cfg(test)]
    pub fn from_claude_json(raw: &[u8]) -> Option<Self> {
        Self::from_value(&serde_json::from_slice(raw).ok()?)
    }

    /// From a parsed `.claude.json`.
    pub fn from_value(v: &serde_json::Value) -> Option<Self> {
        Self::from_block(v.get("oauthAccount")?)
    }

    /// From an `oauthAccount` block itself (as saved by auto-switch). A
    /// value over 320 bytes, with control characters, empty or not a string
    /// counts as missing; a block with neither a uuid nor an e-mail is no
    /// account.
    pub fn from_block(block: &serde_json::Value) -> Option<Self> {
        let field = |k: &str| {
            block
                .get(k)
                .and_then(serde_json::Value::as_str)
                .filter(|s| safe_account(s))
                .map(str::to_string)
        };
        let a = Self {
            uuid: field("accountUuid"),
            org: field("organizationUuid"),
            email: field("emailAddress").map(|e| e.to_lowercase()),
        };
        (a.uuid.is_some() || a.email.is_some()).then_some(a)
    }

    /// From an answer of the OAuth profile endpoint (issue #1886):
    /// `account.uuid`, `account.email` and `organization.uuid`, with the
    /// same limits as [`Self::from_block`]. The server reports these for the
    /// token it was sent, so unlike `.claude.json` they say whose login a
    /// credential file holds.
    pub fn from_profile(v: &serde_json::Value) -> Option<Self> {
        let field = |block: &str, k: &str| {
            v.get(block)
                .and_then(|b| b.get(k))
                .and_then(serde_json::Value::as_str)
                .filter(|s| safe_account(s))
                .map(str::to_string)
        };
        let a = Self {
            uuid: field("account", "uuid"),
            org: field("organization", "uuid"),
            email: field("account", "email").map(|e| e.to_lowercase()),
        };
        (a.uuid.is_some() || a.email.is_some()).then_some(a)
    }

    /// What `profile verify` prints for this account: the e-mail, else the
    /// uuid. Never logged by the rotator.
    pub fn shown(&self) -> &str {
        self.email.as_deref().or(self.uuid.as_deref()).unwrap_or("")
    }

    /// The account in `dir/.claude.json`.
    pub fn of_dir(dir: &Path) -> Option<Self> {
        Self::from_value(&read_claude_json(dir)?.0)
    }

    /// Two logins are one account when they are the same user in the same
    /// organization: one user in two organizations has two subscriptions,
    /// each with its own limits. With a uuid on both sides the uuids and
    /// the organizations (both present and equal, or both missing) decide;
    /// with a uuid on neither side the e-mails do; with a uuid on one side
    /// only they are different accounts.
    pub fn same(&self, other: &Self) -> bool {
        match (&self.uuid, &other.uuid) {
            (Some(a), Some(b)) => a == b && self.org == other.org,
            (None, None) => matches!((&self.email, &other.email), (Some(a), Some(b)) if a == b),
            _ => false,
        }
    }

    /// The value stored in the ledger: the uuid, else the e-mail.
    pub fn key(&self) -> &str {
        self.uuid.as_deref().or(self.email.as_deref()).unwrap_or("")
    }

    /// The organization uuid, when the login names one.
    pub fn org(&self) -> Option<&str> {
        self.org.as_deref()
    }

    /// Whether a stored `key` and `org` name this account. A key that is
    /// this account's uuid matches only in the same organization (both
    /// present and equal, or both missing). Any other key falls through to
    /// [`Self::matches_key`], which matches on e-mail and ignores `org`.
    ///
    /// This is not [`Self::same`] semantics. An e-mail-keyed record (written
    /// from a login with no uuid) matches a current login that has a uuid
    /// whenever the e-mail agrees, even in another organization, where
    /// [`Self::same`] would call the two different accounts. A normal OAuth
    /// login carries a uuid, so such records are rare; the user docs state
    /// that an e-mail record is compared by e-mail alone.
    pub fn matches_record(&self, key: &str, org: Option<&str>) -> bool {
        if self.uuid.as_deref() == Some(key) {
            return self.org.as_deref() == org;
        }
        self.matches_key(key)
    }

    /// Whether a stored key names this account.
    pub fn matches_key(&self, key: &str) -> bool {
        self.uuid.as_deref() == Some(key)
            || self
                .email
                .as_deref()
                .is_some_and(|e| e.eq_ignore_ascii_case(key))
    }
}

/// Whether `<home>/.claude.json` holds an account that can be read. It only
/// stops a swap while Claude rewrites the file (#1702); it never says whose
/// login is in `~/.claude` (#1886).
pub fn home_readable(home: &Path) -> bool {
    Account::of_dir(home).is_some()
}
