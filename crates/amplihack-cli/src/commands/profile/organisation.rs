//! Metadata-only organisation resolution for auto-switch (#2136). Hosting,
//! rather than the directory name, identifies the live login. Credential
//! files are never read here.
//!
//! The organisation comes from the profile's expected record, read by
//! [`expected::lookup`], the one reader of `expected/<profile>.json`. A
//! record that is missing, unsafe or malformed reads as no record there,
//! and so as an unknown organisation here. Unknown is never a crossing, so
//! doubtful metadata cannot keep a session on a login it would otherwise
//! leave; such a swap carries the "unresolved" warning.
//!
//! The rotator takes the organisation from its trusted identity reads
//! (`Snap::org_of`), not from this file. Its swap guard compares each read
//! with the same expected record (`Record::names`, organisation included)
//! and refuses the swap on a mismatch, so wherever a record exists the two
//! agree for every swap that goes ahead.
use super::auto_switch::kit::{Ledger, State};
use super::expected;
use super::name::ProfileName;

/// The organisation of the login hosted in `host`'s directory.
pub fn resolve(state: &State, ledger: &Ledger, host: &str) -> Option<String> {
    resolve_login(state, ledger.account_in(host))
}

/// The organisation recorded for `login`, or `None` when it is unknown.
pub fn resolve_login(state: &State, login: &str) -> Option<String> {
    let name = ProfileName::parse(login).ok()?;
    expected::lookup(state, &name).ok().flatten()?.org
}

#[cfg(test)]
#[path = "tests_organisation.rs"]
mod tests;
