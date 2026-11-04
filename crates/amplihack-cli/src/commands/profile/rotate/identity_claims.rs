//! Rotation-scoped ownership claims. A failed read is never an unclaimed
//! login. The complete profile list, records, and file bindings are retained
//! until the last authorization check.

use super::identify::{Account, LoginKind, login_kind};
use super::login_identity::{Bound, Reader, UNAVAILABLE};
use crate::commands::profile::auto_switch::kit::{CREDS, State, exists};
use crate::commands::profile::expected::{self, Record};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;

/// Whether another validated profile claims this account.
pub enum Claim {
    /// At least one trusted login or expected record claims the account.
    Claimed,
    /// A complete successful scan found no other claim.
    Unclaimed,
}

/// The result and every observation needed to retain its validity.
pub struct Claims {
    names: Vec<String>,
    records: Vec<(ProfileName, Option<Record>)>,
    bindings: Vec<Bound>,
    absent: Vec<std::path::PathBuf>,
    /// Ownership result; an uncertain scan never constructs this value.
    pub claim: Claim,
}

/// Read an expected record, distinguishing absence from unusable evidence.
pub fn record(state: &State, name: &ProfileName) -> Result<Option<Record>, &'static str> {
    expected::lookup(state, name).map_err(
        |_| "trusted login identity: expected record is unreadable; repair it before retrying",
    )
}

/// Scan every other profile's record and trusted login identity.
pub fn scan(
    store: &ProfileStore,
    state: &State,
    except: Option<&ProfileName>,
    account: &Account,
    reader: Reader<'_>,
) -> Result<Claims, &'static str> {
    scan_except(store, state, except, account, reader, None)
}

/// Scan claims, optionally excluding the outgoing login's current location.
pub fn scan_except(
    store: &ProfileStore,
    state: &State,
    except: Option<&ProfileName>,
    account: &Account,
    reader: Reader<'_>,
    ignored_login: Option<&std::path::Path>,
) -> Result<Claims, &'static str> {
    let mut names = store.list().map_err(|_| UNAVAILABLE)?;
    names.sort();
    let mut claims = Claims {
        names: names.clone(),
        records: Vec::new(),
        bindings: Vec::new(),
        absent: Vec::new(),
        claim: Claim::Unclaimed,
    };
    for raw in names {
        let name = ProfileName::parse(&raw).map_err(|_| UNAVAILABLE)?;
        if Some(&name) == except {
            continue;
        }
        let dir = store.verify_profile_dir(&name).map_err(|_| UNAVAILABLE)?;
        let rec = record(state, &name)?;
        if rec.as_ref().is_some_and(|r| r.names(account)) {
            claims.claim = Claim::Claimed;
        }
        claims.records.push((name, rec));
        if ignored_login == Some(dir.as_path()) {
            continue;
        }
        if !exists(&dir.join(CREDS)) {
            claims.absent.push(dir.join(CREDS));
            continue;
        }
        // A positively identified API-key login cannot own an OAuth account.
        let platform = login_kind(&dir.join(CREDS)) == LoginKind::NoOAuth;
        let bound = reader.bound(&dir, platform)?;
        if bound.account().is_some_and(|a| a.same(account)) {
            claims.claim = Claim::Claimed;
        }
        claims.bindings.push(bound);
    }
    Ok(claims)
}

impl Claims {
    /// Validate retained observations, allowing only a newly adopted directory.
    pub fn current_ignoring(
        &self,
        store: &ProfileStore,
        state: &State,
        introduced: Option<&ProfileName>,
    ) -> bool {
        let Ok(mut names) = store.list() else {
            return false;
        };
        if let Some(name) = introduced {
            names.retain(|n| n != name.as_str());
        }
        names.sort();
        names == self.names
            && self.bindings.iter().all(Bound::current)
            && self.absent.iter().all(|p| !exists(p))
            && self
                .records
                .iter()
                .all(|(n, r)| record(state, n).as_ref() == Ok(r))
    }
}
