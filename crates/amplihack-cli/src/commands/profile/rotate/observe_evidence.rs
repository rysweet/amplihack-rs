//! Issue #2013 (D10): what each source says about the live account, for a
//! refusal line only. No decision reads it: the verified identity, the
//! ledger and the record decide; `~/.claude.json` is evidence only.
//!
//! Each value is an e-mail address. A source with nothing to say is `None`;
//! one that could not be read is [`UNREADABLE`]. The line never holds an
//! account UUID or organization ID, so a record, which stores a UUID, is
//! shown by the e-mail of the source it agrees with.

use super::super::super::identify::Account;
use super::super::super::identity_claims;
use super::super::super::ledger::RotateLedger;
use super::super::super::refusal::{Evidence, UNREADABLE};
use super::Ctx;
use crate::commands::profile::auto_switch::kit::{CLAUDE_JSON, State, exists, read_claude_json};
use crate::commands::profile::name::ProfileName;

/// An account with no e-mail address to show.
const NO_EMAIL: &str = "an account without an e-mail address";
/// A record that names none of the other sources' accounts.
const ANOTHER: &str = "another account";

fn email(a: &Account) -> String {
    a.email().unwrap_or(NO_EMAIL).to_string()
}

/// The evidence for the live profile `live` (the ledger's).
pub fn of(
    ctx: &Ctx,
    state: &State,
    ledger: Option<&RotateLedger>,
    verified: Option<&Account>,
    live: Option<&ProfileName>,
) -> Evidence {
    let ledger_account = ledger
        .and_then(|l| l.login_identity.as_ref())
        .and_then(|i| serde_json::to_value(i).ok())
        .and_then(|b| Account::from_block(&b));
    let home = home(ctx);
    let record = live.map(|p| identity_claims::record(state, p));
    let record = match record {
        None | Some(Ok(None)) => None,
        Some(Err(_)) => Some(UNREADABLE.to_string()),
        Some(Ok(Some(rec))) => Some(
            [verified, ledger_account.as_ref(), home.1.as_ref()]
                .into_iter()
                .flatten()
                .find(|a| rec.names(a))
                .map_or_else(|| ANOTHER.to_string(), email),
        ),
    };
    Evidence {
        verified: verified.map(email),
        ledger: ledger_account.as_ref().map(email),
        record,
        home: home.0,
    }
}

/// The value `~/.claude.json` shows, and the account its block names.
fn home(ctx: &Ctx) -> (Option<String>, Option<Account>) {
    let Some((v, _)) = read_claude_json(&ctx.home) else {
        if exists(&ctx.home.join(CLAUDE_JSON)) {
            return (Some(UNREADABLE.to_string()), None);
        }
        return (None, None);
    };
    match v.get("oauthAccount") {
        None | Some(serde_json::Value::Null) => (None, None),
        Some(block) => match Account::from_block(block) {
            Some(a) => (Some(email(&a)), Some(a)),
            None => (Some(UNREADABLE.to_string()), None),
        },
    }
}
