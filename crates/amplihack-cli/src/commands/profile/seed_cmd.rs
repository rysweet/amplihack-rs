//! `amplihack profile seed-record <name> [--write]` (issue #1962): prints what
//! `rotate/seed.rs` found. It takes a profile name and no folder, and the name is
//! parsed before any path is built. An account value goes to stdout only, and only
//! a value `Account` kept as safe is printed. See "profile seed-record" in
//! docs/reference/profile-command.md.

use super::ProfileError;
use super::name::ProfileName;
use super::rotate::identify::Account;
use super::rotate::seed::{self, Explicit};
use super::store::ProfileStore;
use super::usage_cmd;

/// Printed for an account value that is not safe to print.
const UNPRINTABLE: &str = "<unprintable>";

fn shown(account: &Account) -> String {
    format!(
        "{}, organization {}",
        account.email().unwrap_or(UNPRINTABLE),
        account.org().unwrap_or(UNPRINTABLE)
    )
}

/// Runs the command. A refusal is an error, so it exits 1.
pub(super) fn run(store: &ProfileStore, raw: &str, write: bool) -> Result<(), ProfileError> {
    let name = ProfileName::parse(raw)?;
    store.resolve(&name)?;
    let refuse = |reason: &str| {
        ProfileError::SeedRefused(format!(
            "seed-record {name}: refused: {reason}; nothing written"
        ))
    };
    let home = usage_cmd::home_dir().ok_or_else(|| refuse("the home directory is not known"))?;
    match seed::explicit(store, &home, &name, write)? {
        Explicit::Existing => println!("{name}: a usable record exists; nothing written"),
        Explicit::Preview(account) => println!(
            "{name} -> {}: no record; nothing written; run with --write to record it",
            shown(&account)
        ),
        Explicit::Recorded(account) => println!("{name} -> {}: recorded", shown(&account)),
        Explicit::Refused(reason) => return Err(refuse(&reason)),
    }
    Ok(())
}
