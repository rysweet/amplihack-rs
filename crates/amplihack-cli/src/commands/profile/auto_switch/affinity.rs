//! Organisation affinity for auto-switch selection (#2136): the one place
//! the threshold swap ([`super::swap`]) and the nested rerun
//! ([`super::rerun`]) restrict their candidates. The rotator applies the
//! same pure policy (`amplihack_utils::organisation_affinity::pools`) in
//! `rotate/observe.rs`, on its trusted identity reads.
//!
//! Reporting: [`record_swap`] records a swap auto-switch made across or into
//! an unknown organisation in the rotate log and in
//! `.auto-switch.d/affinity/last-crossing.json`; [`status_line`] prints it
//! in `auto-switch status`.
use super::candidate::{self, Pool};
use super::overflow_inputs::OverflowInputs;
use super::state::{Ledger, State};
use crate::commands::profile::name::ProfileName;
use crate::commands::profile::organisation;
use amplihack_utils::organisation_affinity::{Affinity, Candidate, pools, valid_id};

/// The pools one auto-switch decision may select from, and the
/// organisations it read to work them out.
pub struct Restriction {
    /// Targets the selector may rank.
    pub allowed: Vec<ProfileName>,
    /// Targets the plan-room fallback may use.
    pub plan_room: Vec<ProfileName>,
    /// The organisation of the login hosted in the source directory.
    pub source_org: Option<String>,
    orgs: Vec<(ProfileName, Option<String>)>,
}

impl Restriction {
    /// `target`'s organisation, as read for this decision.
    pub fn org_of(&self, target: &ProfileName) -> Option<&str> {
        self.orgs
            .iter()
            .find(|(p, _)| p == target)
            .and_then(|(_, o)| o.as_deref())
    }
}

/// Work out both pools once for one decision. `eligible` is the caller's
/// own target rule; affinity only ever narrows it. Overflow targets are
/// eligible only when the unrestricted selection would already move onto
/// overflow, so affinity cannot turn a plan move into paid spend.
#[allow(clippy::too_many_arguments)]
pub fn restrict(
    state: &State,
    ledger: &Ledger,
    pool: &Pool<'_>,
    overflow: &OverflowInputs,
    source_host: &str,
    current: &str,
    necessary: bool,
    eligible: impl Fn(&ProfileName) -> bool,
) -> Restriction {
    let ov = overflow.pool();
    let paid_allowed = candidate::select_with_overflow(pool, &ov, current, &eligible)
        .is_some_and(|p| overflow.order.contains(p));
    let affinity_eligible =
        |p: &ProfileName| eligible(p) && (paid_allowed || !overflow.order.contains(p));
    let target = |p: &ProfileName| {
        p.as_str() != current
            && affinity_eligible(p)
            && !pool.exclude.contains(p)
            && candidate::overflow_usable(p.as_str(), &overflow.blocked)
    };
    let source_org = organisation::resolve(state, ledger, source_host);
    let orgs: Vec<(ProfileName, Option<String>)> = pool
        .profiles
        .iter()
        .filter(|p| target(p))
        .map(|p| (p.clone(), organisation::resolve_login(state, p.as_str())))
        .collect();
    // Capacity: whether the selector, offered only this target, picks it.
    // Room: whether the plan-room fallback, offered only this target, picks
    // it. Asked only of eligible targets.
    let only = |p: &ProfileName, capacity: bool| {
        let this = |q: &ProfileName| q == p && affinity_eligible(q);
        let picked = if capacity {
            candidate::select_restricted(pool, &ov, current, this, |_| false)
        } else {
            candidate::select_restricted(pool, &ov, current, |_| false, this)
        };
        picked == Some(p)
    };
    let candidates: Vec<Candidate<'_>> = orgs
        .iter()
        .map(|(p, org)| Candidate {
            org: org.as_deref(),
            eligible: true,
            capacity: only(p, true),
            room: only(p, false),
        })
        .collect();
    let pools = pools(source_org.as_deref(), necessary, &candidates);
    let names = |indices: &[usize]| -> Vec<ProfileName> {
        indices.iter().map(|&i| orgs[i].0.clone()).collect()
    };
    Restriction {
        allowed: names(&pools.allowed),
        plan_room: names(&pools.plan_room),
        source_org,
        orgs,
    }
}

/// State subdirectory and name of the last-crossing record.
const RECORD: (&str, &str) = ("affinity", "last-crossing");

/// Records a swap auto-switch has just made, when it is not within one
/// organisation: the warning goes to the rotate log, to the status-line
/// command's stderr (which Claude Code does not show) and into
/// `.auto-switch.d/affinity/last-crossing.json`, which `auto-switch status`
/// prints. Swaps within one organisation record nothing. Nothing here can
/// stop or undo the swap; a failed write is reported on stderr.
pub fn record_swap(
    state: &State,
    dir: &str,
    from: &str,
    to: &str,
    from_org: Option<&str>,
    to_org: Option<&str>,
    at: i64,
) {
    let affinity = Affinity::between(from_org, to_org);
    let Some(warning) = affinity.warning() else {
        return;
    };
    eprintln!("amplihack: auto-switch in {dir} from {from} to {to}: {warning}");
    if let Some(line) = affinity.log_line(from, to) {
        crate::commands::profile::rotate::append_to_log(&state.root, &line);
    }
    let valid = |o: Option<&str>| o.filter(|v| valid_id(v)).map(str::to_owned);
    let record = serde_json::json!({
        "at": at,
        "dir": dir,
        "from": from,
        "to": to,
        "from_org": valid(from_org),
        "to_org": valid(to_org),
        "affinity": affinity,
    });
    if let Err(e) = state.write_sub_json(RECORD.0, RECORD.1, &record) {
        eprintln!("warning: auto-switch could not record the organisation crossing: {e}");
    }
}

/// `auto-switch status`: the last swap auto-switch made across or into an
/// unknown organisation, from [`record_swap`]'s record. `None` when there
/// is none; a damaged record is named, not hidden.
pub fn status_line(state: &State, ago: impl Fn(i64) -> String) -> Option<String> {
    let v = match state.read_sub_json(RECORD.0, RECORD.1) {
        Ok(Some(v)) => v,
        Ok(None) => return None,
        Err(e) => return Some(format!("last crossing: unreadable ({e})")),
    };
    let text = |k: &str| v.get(k).and_then(|x| x.as_str());
    let affinity: Option<Affinity> = v
        .get("affinity")
        .and_then(|a| serde_json::from_value(a.clone()).ok());
    let (Some(dir), Some(from), Some(to), Some(warning)) = (
        text("dir"),
        text("from"),
        text("to"),
        affinity.and_then(Affinity::warning),
    ) else {
        return Some("last crossing: unreadable (missing fields)".into());
    };
    let org = |k: &str| text(k).unwrap_or("unknown").to_string();
    let when = v.get("at").and_then(|a| a.as_i64()).map(ago);
    Some(format!(
        "last crossing: in {dir} from {from} (organisation {}) to {to} (organisation {}){}: {warning}",
        org("from_org"),
        org("to_org"),
        when.map(|w| format!(", {w}")).unwrap_or_default(),
    ))
}
