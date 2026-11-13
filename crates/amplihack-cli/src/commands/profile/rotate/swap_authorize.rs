//! Authorization shared by automatic rotation, switching, and recovery.
//! All trusted evidence is rechecked while refresh locks are held.

use super::identify::Account;
use super::identity_claims::{self, Claim, Claims};
use super::ledger::RotateState;
use super::login_identity::{Bound, Reader, SwapIdentity, UNAVAILABLE, unavailable_line};
use super::swap::Source;
use super::swap_expect::{ACCOUNT_MISMATCH, Pending};
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::State;
use crate::commands::profile::expected::Record;
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::store::ProfileStore;
use std::path::Path;

/// Report a refusal before any login movement.
pub fn refused(why: impl Into<String>) -> ProfileError {
    ProfileError::RotateRefused(why.into())
}

/// Report a failed identity read by its cause (issue #1962).
fn read_refused(cause: &'static str) -> ProfileError {
    refused(unavailable_line(cause))
}

/// Whether full trusted ledger provenance establishes this profile's ownership.
pub fn ledger_owns(l: &super::ledger::RotateLedger, name: &ProfileName, account: &Account) -> bool {
    l.live.as_deref() == Some(name.as_str())
        && l.account
            .as_deref()
            .is_some_and(|key| account.matches_key(key))
        && l.login_identity
            .as_ref()
            .is_some_and(|prior| prior.names(account))
}

/// Evidence required in addition to current expected records.
#[derive(Clone, Copy)]
pub enum Proof<'a> {
    /// A new operation must prove ownership from current state.
    Live,
    /// Recovery retains the complete original authorization, when available.
    Recovery(Option<&'a SwapIdentity>),
    Duplicate(&'a super::duplicate::DuplicateProof),
    Conflict(&'a super::duplicate::ConflictProof),
}

/// Keep the final authorization tied to the accounts the accepted snapshot
/// measured. A target without a record still cannot change accounts in the
/// gap between revalidation and the refresh-locked swap guard.
pub fn check_snapshot(
    authorized: &Authorized,
    snapshot: Option<&super::observe::Snap>,
    to: &ProfileName,
) -> Result<(), ProfileError> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    let same = |a: Option<&Account>, b: Option<&Account>| match (a, b) {
        (Some(a), Some(b)) => a.same(b),
        (None, None) => true,
        _ => false,
    };
    let target = snapshot
        .candidate_accounts
        .iter()
        .find(|(name, _)| name == to);
    if (!snapshot.candidates.contains(to) && authorized.incoming.account().is_some())
        || !same(
            authorized.outgoing.account(),
            snapshot.trusted_live.as_ref(),
        )
        || !same(
            authorized.incoming.account(),
            target.and_then(|(_, a)| a.as_ref()),
        )
    {
        return Err(refused(
            "trusted login identity changed after revalidation; nothing moved",
        ));
    }
    Ok(())
}

/// Trusted accounts and ownership dependencies accepted for one swap.
pub struct Authorized {
    /// The login being filed under the outgoing profile.
    pub outgoing: Bound,
    /// The login being made live.
    pub incoming: Bound,
    /// Deferred expected-record update from these trusted accounts.
    pub pending: Pending,
    records: Vec<(ProfileName, Option<Record>)>,
    claims: Vec<Claims>,
    occupied: Option<Bound>,
}

impl Authorized {
    /// Whether every material observation still holds.
    pub fn current(&self, store: &ProfileStore) -> bool {
        self.current_ignoring(store, None)
    }
    /// Validate dependencies, allowing a directory introduced by adoption.
    pub fn current_ignoring(&self, store: &ProfileStore, introduced: Option<&ProfileName>) -> bool {
        let Ok(state) = State::new(store.root()) else {
            return false;
        };
        self.outgoing.current()
            && self.incoming.current()
            && self.occupied.as_ref().is_none_or(Bound::current)
            && self
                .records
                .iter()
                .all(|(n, r)| identity_claims::record(&state, n).as_ref() == Ok(r))
            && self
                .claims
                .iter()
                .all(|c| c.current_ignoring(store, &state, introduced))
    }
    /// Compare full identities and expected records across fresh readings.
    pub fn same(&self, next: &Self) -> bool {
        self.outgoing.same(&next.outgoing)
            && self.incoming.same(&next.incoming)
            && self.records == next.records
    }
}

/// Authorizes the actual login at each supplied directory, including the
/// outgoing login temporarily in the target after an interrupted exchange.
#[allow(clippy::too_many_arguments)]
pub fn authorize(
    store: &ProfileStore,
    rs: &RotateState,
    from: &Source,
    to: &ProfileName,
    outgoing_dir: &Path,
    incoming_dir: &Path,
    platform_in: bool,
    platform_out: bool,
    reader: Reader<'_>,
    proof: Proof<'_>,
) -> Result<Authorized, ProfileError> {
    if let Source::Profile(name) = from {
        let dir = store
            .verify_profile_dir(name)
            .map_err(|_| refused(UNAVAILABLE))?;
        if dir != outgoing_dir
            && crate::commands::profile::auto_switch::kit::exists(
                &dir.join(crate::commands::profile::auto_switch::kit::CREDS),
            )
            && !matches!(proof, Proof::Duplicate(p) if p.permits(&dir))
            && !matches!(proof, Proof::Conflict(p) if p.permits(&dir))
        {
            return Err(refused(UNAVAILABLE));
        }
    }
    let state = State::new(store.root()).map_err(|_| refused(UNAVAILABLE))?;
    let outgoing = reader
        .bound(outgoing_dir, platform_out)
        .map_err(read_refused)?;
    let incoming = reader
        .bound(incoming_dir, platform_in)
        .map_err(read_refused)?;
    if let Some(origin) = super::custody::origin(store, to)? {
        check_record(store, &origin, incoming.account())?;
    }
    if let Proof::Recovery(Some(prior)) = proof
        && !prior.names(outgoing.account(), incoming.account())
    {
        return Err(refused(
            "trusted login identity conflicts with the swap journal; no recovery movement",
        ));
    }
    let mut records = Vec::new();
    let mut claims = Vec::new();
    let incoming_record = identity_claims::record(&state, to).map_err(refused)?;
    if matches!(proof, Proof::Recovery(None))
        && incoming.account().is_some()
        && incoming_record.is_none()
    {
        return Err(refused(
            "trusted login identity: legacy journal lacks incoming ownership evidence",
        ));
    }
    if let Some(a) = incoming.account() {
        match &incoming_record {
            Some(r) if !r.names(a) => {
                return Err(refused(format!(
                    "trusted login identity conflicts with the account recorded for profile {to}"
                )));
            }
            None => {
                let scan = identity_claims::scan(store, &state, Some(to), a, reader)
                    .map_err(read_refused)?;
                if matches!(scan.claim, Claim::Claimed) {
                    return Err(refused(
                        "trusted login identity: incoming login is claimed by another profile",
                    ));
                }
                claims.push(scan);
            }
            _ => {}
        }
    }
    records.push((to.clone(), incoming_record));
    let pending = match (from, outgoing.account()) {
        (Source::Profile(name), Some(a)) => {
            let rec = identity_claims::record(&state, name).map_err(refused)?;
            let pending = match &rec {
                Some(r) if !r.names(a) => {
                    return Err(refused(format!("{ACCOUNT_MISMATCH} {name}")));
                }
                Some(_) => super::swap_expect::relabel_for(store, name, a),
                None => {
                    let came_from = match proof {
                        Proof::Recovery(Some(prior)) => {
                            prior.outgoing.as_ref().is_some_and(|p| p.names(a))
                        }
                        _ => rs.load_ledger()?.is_some_and(|l| ledger_owns(&l, name, a)),
                    };
                    if !came_from {
                        return Err(refused(format!(
                            "trusted login identity: outgoing ownership is not established; run: amplihack profile seed-record {name}"
                        )));
                    }
                    let scan = identity_claims::scan_except(
                        store,
                        &state,
                        Some(name),
                        a,
                        reader,
                        Some(outgoing_dir),
                    )
                    .map_err(read_refused)?;
                    if matches!(scan.claim, Claim::Claimed) {
                        return Err(refused(
                            "trusted login identity: outgoing login is claimed by another profile",
                        ));
                    }
                    claims.push(scan);
                    Pending::Backfill(a.clone())
                }
            };
            records.push((name.clone(), rec));
            pending
        }
        (Source::Adopt, Some(a)) => {
            let scan =
                identity_claims::scan_except(store, &state, None, a, reader, Some(outgoing_dir))
                    .map_err(read_refused)?;
            let claimed = matches!(scan.claim, Claim::Claimed);
            claims.push(scan);
            if claimed {
                Pending::AdoptedUnrecorded
            } else {
                Pending::Adopted(a.clone())
            }
        }
        (_, None) => Pending::Nothing,
    };
    let auth = Authorized {
        outgoing,
        incoming,
        pending,
        records,
        claims,
        occupied: match proof {
            Proof::Conflict(p) => Some(p.occupied().clone()),
            _ => None,
        },
    };
    if !auth.current(store) {
        return Err(refused(UNAVAILABLE));
    }
    Ok(auth)
}

/// Account checks that need no claim scan, for the completed placement of
/// an interrupted swap. An existing unusable record always refuses.
pub fn check_record(
    store: &ProfileStore,
    name: &ProfileName,
    account: Option<&Account>,
) -> Result<(), ProfileError> {
    let state = State::new(store.root()).map_err(|_| refused(UNAVAILABLE))?;
    let rec = identity_claims::record(&state, name).map_err(refused)?;
    if let (Some(r), Some(a)) = (rec, account)
        && !r.names(a)
    {
        return Err(refused(format!(
            "trusted login identity conflicts with the account recorded for profile {name}"
        )));
    }
    Ok(())
}
