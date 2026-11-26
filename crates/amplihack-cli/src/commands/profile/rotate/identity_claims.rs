//! Rotation-scoped ownership claims. A failed read is never an unclaimed
//! login: swap authorization refuses on one. A seed may let a usable record
//! stand in for a read that says nothing against the folder (issue #1962, S4).
//! The complete profile list, records, and file bindings are retained until
//! the last authorization check.

use super::identify::{Account, LoginKind, login_kind};
use super::login_identity::{
    BUDGET_SPENT, Bound, DIR_NOT_PRIVATE, LOGIN_UNTRUSTED, Reader, UNAVAILABLE,
};
use crate::commands::profile::auto_switch::kit::{CREDS, State};
use crate::commands::profile::expected::{self, Record};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use crate::commands::profile::usage_direct::reason;

/// What a failed read of another profile means to a scan. There is no default:
/// each caller says which it wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unread {
    /// Swap authorization: any failed read refuses.
    Refuse,
    /// A seed: a usable record stands in for a read that failed for a cause
    /// that says nothing against the folder ([`record_stands_in`]).
    RecordCounts,
}

/// Whether a read that failed for `cause` says nothing against the folder. An
/// untrusted or changed login or an open directory always blocks, and so does
/// an answer that names no account.
fn record_stands_in(cause: &str) -> bool {
    [
        reason::EXPIRED,
        reason::HTTP_401,
        reason::HTTP_429,
        reason::TIMEOUT,
        reason::TRANSPORT,
        BUDGET_SPENT,
    ]
    .contains(&cause)
}

/// Why a scan could not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// The profile root could not be listed.
    Listing,
    /// This profile could not be checked, for this cause.
    Profile(ProfileName, &'static str),
    /// This profile's record exists but cannot be used.
    UnusableRecord(ProfileName),
}

const RECORD_UNREADABLE: &str =
    "trusted login identity: expected record is unreadable; repair it before retrying";

impl Blocked {
    /// The fixed cause swap authorization reports.
    fn cause(&self) -> &'static str {
        match self {
            Self::Listing => UNAVAILABLE,
            Self::Profile(_, cause) => cause,
            Self::UnusableRecord(_) => RECORD_UNREADABLE,
        }
    }

    /// The reason a seed reports, naming the profile.
    pub fn reason(&self) -> String {
        match self {
            Self::Listing => "the profile root could not be listed".to_string(),
            Self::Profile(name, cause) => format!("profile {name} not read ({cause})"),
            Self::UnusableRecord(name) => {
                format!("profile {name} has an unusable record; repair it first")
            }
        }
    }
}

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
    /// Who claims the account, by login or by record, sorted by name.
    pub claimers: Vec<ProfileName>,
}

/// Read an expected record, distinguishing absence from unusable evidence.
pub fn record(state: &State, name: &ProfileName) -> Result<Option<Record>, &'static str> {
    expected::lookup(state, name).map_err(|_| RECORD_UNREADABLE)
}

/// Only a definite NotFound establishes absence; all other errors are uncertain.
type MetadataProbe<'a> = &'a dyn Fn(&std::path::Path) -> std::io::Result<std::fs::Metadata>;

fn absent(path: &std::path::Path, probe: MetadataProbe<'_>) -> Result<bool, &'static str> {
    match probe(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(_) => Err(UNAVAILABLE),
    }
}

/// The scan a seed uses: [`Unread::RecordCounts`], and a blocked scan names the
/// profile. Never used to authorize a swap.
pub fn scan_for_seed(
    store: &ProfileStore,
    state: &State,
    except: &ProfileName,
    account: &Account,
    reader: Reader<'_>,
) -> Result<Claims, Blocked> {
    let probe: MetadataProbe<'_> = &|p| std::fs::symlink_metadata(p);
    walk(
        store,
        state,
        Some(except),
        account,
        reader,
        None,
        probe,
        Unread::RecordCounts,
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
    scan_with(store, state, except, account, reader, ignored_login, &|p| {
        std::fs::symlink_metadata(p)
    })
}

fn scan_with(
    store: &ProfileStore,
    state: &State,
    except: Option<&ProfileName>,
    account: &Account,
    reader: Reader<'_>,
    ignored_login: Option<&std::path::Path>,
    probe: MetadataProbe<'_>,
) -> Result<Claims, &'static str> {
    walk(
        store,
        state,
        except,
        account,
        reader,
        ignored_login,
        probe,
        Unread::Refuse,
    )
    .map_err(|blocked| blocked.cause())
}

#[allow(clippy::too_many_arguments)]
fn walk(
    store: &ProfileStore,
    state: &State,
    except: Option<&ProfileName>,
    account: &Account,
    reader: Reader<'_>,
    ignored_login: Option<&std::path::Path>,
    probe: MetadataProbe<'_>,
    policy: Unread,
) -> Result<Claims, Blocked> {
    let mut names = store.list().map_err(|_| Blocked::Listing)?;
    names.sort();
    let mut claims = Claims {
        names: names.clone(),
        records: Vec::new(),
        bindings: Vec::new(),
        absent: Vec::new(),
        claim: Claim::Unclaimed,
        claimers: Vec::new(),
    };
    for raw in names {
        let name = ProfileName::parse(&raw).map_err(|_| Blocked::Listing)?;
        if Some(&name) == except {
            continue;
        }
        let dir = store
            .verify_profile_dir(&name)
            .map_err(|_| Blocked::Profile(name.clone(), DIR_NOT_PRIVATE))?;
        let rec =
            expected::lookup(state, &name).map_err(|()| Blocked::UnusableRecord(name.clone()))?;
        let mut claimed = rec.as_ref().is_some_and(|r| r.names(account));
        claims.records.push((name.clone(), rec.clone()));
        if ignored_login != Some(dir.as_path()) {
            match absent(&dir.join(CREDS), probe) {
                Ok(true) => claims.absent.push(dir.join(CREDS)),
                Err(_) => return Err(Blocked::Profile(name, LOGIN_UNTRUSTED)),
                Ok(false) => {
                    // A positively identified API-key login cannot own an OAuth account.
                    let platform = login_kind(&dir.join(CREDS)) == LoginKind::NoOAuth;
                    match reader.bound(&dir, platform) {
                        Ok(bound) => {
                            claimed |= bound.account().is_some_and(|a| a.same(account));
                            claims.bindings.push(bound);
                        }
                        Err(cause)
                            if policy == Unread::RecordCounts
                                && rec.is_some()
                                && record_stands_in(cause) => {}
                        Err(cause) => return Err(Blocked::Profile(name, cause)),
                    }
                }
            }
        }
        if claimed {
            claims.claim = Claim::Claimed;
            claims.claimers.push(name);
        }
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
        self.current_with(store, state, introduced, &|p| std::fs::symlink_metadata(p))
    }

    fn current_with(
        &self,
        store: &ProfileStore,
        state: &State,
        introduced: Option<&ProfileName>,
        probe: MetadataProbe<'_>,
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
            && self.absent.iter().all(|p| absent(p, probe) == Ok(true))
            && self
                .records
                .iter()
                .all(|(n, r)| record(state, n).as_ref() == Ok(r))
    }
}

#[cfg(test)]
#[path = "tests_identity_claims.rs"]
mod tests;
