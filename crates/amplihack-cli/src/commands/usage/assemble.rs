//! Build a report whose absent observations are null, never claims of zero consumption.
use super::{
    aggregate::{ModelMap, add},
    collect::diagnostic,
    models::*,
};
use std::collections::BTreeSet;

fn sum(models: &ModelMap) -> Option<Tokens> {
    let mut result = None;
    for u in models.values() {
        add_optional(&mut result, Some(u.tokens));
    }
    result
}
fn add_optional(total: &mut Option<Tokens>, other: Option<Tokens>) {
    if let Some(other) = other {
        total.get_or_insert_default().add(other);
    }
}

pub(super) fn assemble(
    m: &Manifest,
    buckets: Vec<ModelMap>,
    unresolved: Vec<UnresolvedUsage>,
    mut ds: BTreeSet<Diagnostic>,
    first: Vec<Option<String>>,
    last: Vec<Option<String>>,
) -> Report {
    let mut all = ModelMap::new();
    for bucket in &buckets {
        for b in bucket.values() {
            add(
                &mut all,
                b.provider,
                b.model.clone(),
                b.tokens,
                b.observed_attempts,
            );
        }
    }
    let mut sessions: Vec<_> = m
        .sessions
        .iter()
        .enumerate()
        .map(|(si, s)| {
            let direct = sum(&buckets[si]);
            if direct.is_none() {
                diagnostic(&mut ds, Some(si), None, None, "no_usage_observed");
            }
            SessionReport {
                id: s.id.clone(),
                run_id: s.run_id.clone(),
                parent: s.parent.clone(),
                provider: s.provider,
                direct,
                inclusive: direct,
                models: buckets[si].values().cloned().collect(),
                complete: !ds.iter().any(|d| d.session == Some(si)),
                first_observed: first[si].clone(),
                last_observed: last[si].clone(),
            }
        })
        .collect();
    for si in 0..sessions.len() {
        let mut parent = sessions[si].parent.clone();
        let tokens = sessions[si].direct;
        let complete = sessions[si].complete;
        while let Some(pi) = parent.and_then(|id| sessions.iter().position(|s| s.id == id)) {
            add_optional(&mut sessions[pi].inclusive, tokens);
            sessions[pi].complete &= complete;
            parent = sessions[pi].parent.clone();
        }
    }
    let total = sum(&all);
    let missing_identity =
        m.workstream.is_none() || m.repository.is_none() || m.branch.is_none() || m.prs.is_empty();
    if missing_identity {
        diagnostic(
            &mut ds,
            None,
            None,
            None,
            "unattributed_workstream_identity",
        );
    }
    let mut unattributed = sum(&buckets[m.sessions.len()]);
    if missing_identity {
        unattributed = total;
    } else {
        for s in &sessions {
            if s.run_id.is_none() {
                add_optional(&mut unattributed, s.direct);
            }
        }
    }
    Report {
        version: 1,
        workstream: m.workstream.clone(),
        repository: m.repository.clone(),
        branch: m.branch.clone(),
        prs: m.prs.clone(),
        allocation: if m.prs.len() > 1 {
            "shared_non_additive"
        } else {
            "exclusive"
        },
        total,
        unattributed,
        unresolved,
        models: all.into_values().collect(),
        sessions,
        complete: ds.is_empty(),
        diagnostics: ds.into_iter().collect(),
        first_observed: first.into_iter().flatten().min(),
        last_observed: last.into_iter().flatten().max(),
        api_price_estimate: None,
        price_reference: None,
        price_assumptions: "Unknown: no versioned price table supplied; observations are not subscription quota or invoices.",
        attempt_semantics: "Unique provider responses where available; cumulative-only Codex transitions are observed attempts, unknown cumulative remainders have unknown attempts. Failed/unrecorded calls are unknown, not zero.",
    }
}
