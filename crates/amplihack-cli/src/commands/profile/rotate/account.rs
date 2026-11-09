//! An account: as `.claude.json` files name it (moved from `identify`, which
//! re-exports it), or as the OAuth profile endpoint reports it for a token
//! ([`Account::from_profile`]). Issue #1886: hand-change recovery and
//! `profile verify` and rotation decide whose login a folder holds from the
//! server answer. Rotation display repair uses that trusted identity.
//! Standalone login and display callers retain their existing semantics.

use super::ledger::safe_account;
use crate::commands::profile::auto_switch::kit::{CLAUDE_JSON, exists, read_claude_json};
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
    /// counts as missing, and so does an e-mail that fails [`email_value`];
    /// a block with neither a uuid nor an e-mail is no account.
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
            email: field("emailAddress").and_then(|e| email_value(&e)),
        };
        (a.uuid.is_some() || a.email.is_some()).then_some(a)
    }

    /// Issue #1947: whether a stored block is a well-formed identity. The block
    /// is a JSON object; each of `accountUuid`, `organizationUuid` and
    /// `emailAddress` is absent or a string that passes [`safe_account`]; an
    /// e-mail has exactly one `@` with text on both sides and no whitespace;
    /// and an account ID or an e-mail is present. Other keys are ignored.
    ///
    /// The e-mail rule is [`email_value`], the same one the readers apply, so
    /// every [`Account`] they produce yields a [`Self::block`] that passes
    /// here: amplihack never writes a `login_identity` its loader refuses.
    pub fn block_is_valid(block: &serde_json::Value) -> bool {
        let Some(block) = block.as_object() else {
            return false;
        };
        let field = |key: &str| match block.get(key) {
            None => Some(None),
            Some(serde_json::Value::String(v)) if safe_account(v) => Some(Some(v.as_str())),
            Some(_) => None,
        };
        let (Some(uuid), Some(_), Some(email)) = (
            field("accountUuid"),
            field("organizationUuid"),
            field("emailAddress"),
        ) else {
            return false;
        };
        email.is_none_or(|e| email_value(e).is_some()) && (uuid.is_some() || email.is_some())
    }

    /// From an answer of the OAuth profile endpoint (issue #1886):
    /// `account.uuid`, `account.email` and `organization.uuid`, with the
    /// same limits as [`Self::from_block`], so a malformed e-mail from the
    /// server counts as missing. The server reports these for the
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
            email: field("account", "email").and_then(|e| email_value(&e)),
        };
        (a.uuid.is_some() || a.email.is_some()).then_some(a)
    }

    /// What `profile verify` prints for this account: the e-mail, else the
    /// uuid. Never logged by the rotator.
    pub fn shown(&self) -> &str {
        self.email.as_deref().or(self.uuid.as_deref()).unwrap_or("")
    }

    /// Whether this names one account in one organization: only such an
    /// account can be seeded as a record (issue #1962).
    pub fn has_uuid_and_org(&self) -> bool {
        self.uuid.is_some() && self.org.is_some()
    }

    /// The e-mail, when the server sent one that is safe to print.
    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
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

    /// A display block derived only from trusted identity.
    pub fn block(&self) -> serde_json::Value {
        let mut b = serde_json::Map::new();
        for (k, v) in [
            ("accountUuid", &self.uuid),
            ("organizationUuid", &self.org),
            ("emailAddress", &self.email),
        ] {
            if let Some(v) = v {
                b.insert(k.into(), v.clone().into());
            }
        }
        b.into()
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

/// Whether `<home>/.claude.json` can be used: it does not exist, or it parses
/// as a JSON object (read without following a symlink). It only stops a swap
/// while the file is damaged (#1702); it never says whose login is in
/// `~/.claude` (#1886). Issue #2013 (D8): a missing block, or one naming
/// another account, is repaired from the verified identity, never a reason
/// to stop.
pub fn home_readable(home: &Path) -> bool {
    !exists(&home.join(CLAUDE_JSON)) || read_claude_json(home).is_some()
}

/// Issue #1947: the one e-mail rule, applied by [`Account::from_block`],
/// [`Account::from_profile`] and [`Account::block_is_valid`]. The value is
/// lower-cased, and both the original and the lower-cased form must pass
/// [`safe_account`]: lower-casing can lengthen a string (`İ` is 2 bytes and
/// becomes the 3-byte `i̇`), so a value at the 320-byte limit could otherwise
/// be stored over it. The lower-cased form must also pass [`email_shape`].
/// `None` means the e-mail is treated as missing.
fn email_value(raw: &str) -> Option<String> {
    let lower = raw.to_lowercase();
    (safe_account(raw) && safe_account(&lower) && email_shape(&lower)).then_some(lower)
}

/// Exactly one `@` with text on both sides, and no whitespace.
fn email_shape(v: &str) -> bool {
    !v.chars().any(char::is_whitespace)
        && v.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && !domain.is_empty() && !domain.contains('@')
        })
}
