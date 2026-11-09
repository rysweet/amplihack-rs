//! Issue #2013 (D10): what a refused swap says. One line names the kind of
//! cause, what each source said about the account, and the action that
//! fixes it. Pure; see "Rotate refusal alerts" in
//! docs/reference/profile-command.md, whose table this module follows row
//! for row.

use super::super::ledger::safe_account;
use crate::commands::profile::name::ProfileName;

/// The kind of a refusal: one per row of the reference table, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    IdentityUnavailable,
    AccountMismatch,
    LedgerMismatch,
    NoRecord,
    SameAccount,
    HomeUnreadable,
    TargetNoLogin,
    HasLogin,
    FileUnsafe,
    RefreshLock,
    Unsupported,
    ApiKey,
    Other,
}

/// The evidence value for a source that was read but could not be used.
/// The line shows it as it is: `unreadable`.
pub const UNREADABLE: &str = "unreadable";

/// What each source said about the account, as e-mail addresses only. `None`
/// shows `none`; a value that fails [`safe_account`] shows `unreadable`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evidence {
    /// The verified identity of the login in `~/.claude`.
    pub verified: Option<String>,
    /// The account the ledger records for its live profile.
    pub ledger: Option<String>,
    /// The account recorded for the profile in `expected/<p>.json`.
    pub record: Option<String>,
    /// The account the `oauthAccount` block in `~/.claude.json` names.
    pub home: Option<String>,
}

impl Cause {
    /// Every kind, in the reference table's order.
    pub const ALL: [Cause; 13] = [
        Self::IdentityUnavailable,
        Self::AccountMismatch,
        Self::LedgerMismatch,
        Self::NoRecord,
        Self::SameAccount,
        Self::HomeUnreadable,
        Self::TargetNoLogin,
        Self::HasLogin,
        Self::FileUnsafe,
        Self::RefreshLock,
        Self::Unsupported,
        Self::ApiKey,
        Self::Other,
    ];

    /// The kind as the line and the alert-limit key print it.
    pub fn kind(self) -> &'static str {
        match self {
            Self::IdentityUnavailable => "identity-unavailable",
            Self::AccountMismatch => "account-mismatch",
            Self::LedgerMismatch => "ledger-mismatch",
            Self::NoRecord => "no-record",
            Self::SameAccount => "same-account",
            Self::HomeUnreadable => "home-unreadable",
            Self::TargetNoLogin => "target-no-login",
            Self::HasLogin => "has-login",
            Self::FileUnsafe => "file-unsafe",
            Self::RefreshLock => "refresh-lock",
            Self::Unsupported => "unsupported",
            Self::ApiKey => "api-key",
            Self::Other => "other",
        }
    }

    /// The fixed action that fixes it, with `p` filled in (`-` for none).
    pub fn fix(self, p: Option<&ProfileName>) -> String {
        let p = p.map_or("-", ProfileName::as_str);
        match self {
            Self::IdentityUnavailable => "none needed; the next cycle reads the identity again; \
                 if this repeats, run: amplihack profile verify"
                .to_string(),
            Self::AccountMismatch => format!(
                "run plain claude, /login to the account recorded for {p}, then wait for the next cycle"
            ),
            Self::LedgerMismatch => "none needed; the next cycle checks for a manual /login in \
                 ~/.claude; if this repeats, run: amplihack profile verify"
                .to_string(),
            Self::NoRecord => format!(
                "run: amplihack profile seed-record {p}, check the account it shows, then run it again with --write"
            ),
            Self::SameAccount => format!(
                "run: amplihack profile verify, then log {p} in to its own account with: amplihack profile login {p}"
            ),
            Self::HomeUnreadable => "repair ~/.claude.json so it is a regular file you own \
                 holding a JSON object; it is never rewritten while it cannot be parsed"
                .to_string(),
            Self::TargetNoLogin => format!("run: amplihack profile login {p}"),
            Self::HasLogin => {
                format!("decide which login {p} keeps, then run: amplihack profile login {p}")
            }
            Self::FileUnsafe => "fix the owner and mode of the path named before 'Evidence:' \
                 (chmod 600 for a login file, chmod 700 for the profile root)"
                .to_string(),
            Self::RefreshLock => "none needed; the next cycle tries again".to_string(),
            Self::Unsupported => {
                "move the profile root onto the same Linux filesystem as your home directory"
                    .to_string()
            }
            Self::ApiKey => "see \"Use an API key as the last resort\" in \
                 docs/howto/claude-profile-rotation.md"
                .to_string(),
            Self::Other => {
                "see \"Swap refused\" in docs/howto/claude-profile-rotation.md".to_string()
            }
        }
    }

    /// The fixed texts of each row's refusal reasons, matched as substrings.
    fn markers(self) -> &'static [&'static str] {
        match self {
            Self::IdentityUnavailable => &[
                "trusted login identity unavailable",
                "trusted login identity changed",
            ],
            Self::AccountMismatch => &[
                super::super::swap_expect::ACCOUNT_MISMATCH,
                "trusted login identity conflicts with the account recorded for profile",
            ],
            Self::LedgerMismatch => &["is not the one the ledger names"],
            Self::NoRecord => &[
                "ownership is not established",
                "expected record is unreadable",
            ],
            Self::SameAccount => &[
                "claimed by another profile",
                "apart: both claim the account",
            ],
            Self::HomeUnreadable => &[
                "~/.claude.json is not a JSON object",
                "~/.claude.json cannot be",
            ],
            Self::TargetNoLogin => &["has no stored login"],
            Self::HasLogin => &["already has a login"],
            Self::FileUnsafe => &[
                "a login file is not a private regular file with one link",
                "HOME is not a directory owned by you",
                "~/.claude is not a private directory owned by you",
                "failed the directory check",
            ],
            Self::RefreshLock => &["refresh lock"],
            Self::Unsupported => &[
                super::super::swap::DIFFERENT_DEVICES,
                "atomic exchange is not supported here",
            ],
            Self::ApiKey => &[
                "has no oauth login and is not an API-key profile",
                "API-key login check",
            ],
            Self::Other => &[],
        }
    }

    /// The kind of a refusal `reason`: the first row whose reason matches,
    /// [`Cause::Other`] when none does.
    pub fn of(reason: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|c| c.markers().iter().any(|m| reason.contains(m)))
            .unwrap_or(Self::Other)
    }

    /// The alert-limit key of this refusal about `p`: `<kind>:<p>`.
    pub fn key(self, p: Option<&ProfileName>) -> String {
        format!("{}:{}", self.kind(), p.map_or("-", ProfileName::as_str))
    }
}

/// One evidence value: `none`, `unreadable`, or the account.
fn value(v: Option<&str>) -> &str {
    match v {
        None => "none",
        Some(v) if safe_account(v) => v,
        Some(_) => "unreadable",
    }
}

/// The refusal line: `ALERT: swap refused (<kind>, profile <p|->): <cause>.
/// Evidence: credentials/verified=<a>, ledger=<b>, record=<c>,
/// .claude.json=<d>. Fix: <action>`.
pub fn line(cause: Cause, p: Option<&ProfileName>, text: &str, ev: &Evidence) -> String {
    format!(
        "ALERT: swap refused ({}, profile {}): {}. Evidence: credentials/verified={}, ledger={}, \
         record={}, .claude.json={}. Fix: {}",
        cause.kind(),
        p.map_or("-", ProfileName::as_str),
        text.trim_end_matches('.'),
        value(ev.verified.as_deref()),
        value(ev.ledger.as_deref()),
        value(ev.record.as_deref()),
        value(ev.home.as_deref()),
        cause.fix(p)
    )
}
