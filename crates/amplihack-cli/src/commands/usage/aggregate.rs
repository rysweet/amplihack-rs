//! Global response identity wins; cumulative-only history fills an explicitly unknown remainder.
use super::{
    adapters::Event,
    assemble,
    collect::{Observation, diagnostic},
    models::*,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type ModelMap = BTreeMap<(Provider, String), ModelUsage>;
struct Identified {
    owner: Option<usize>,
    native_session: Option<String>,
    ambiguous_owner: bool,
    ambiguous_model: bool,
    incomplete: bool,
    source: usize,
    model: String,
    tokens: Option<Tokens>,
}
#[derive(Default)]
struct Snapshots {
    owner: Option<usize>,
    source: usize,
    highwater: Tokens,
    fallback: ModelMap,
    ambiguous_subsets: bool,
}

pub(super) fn add(
    map: &mut ModelMap,
    provider: Provider,
    model: String,
    tokens: Tokens,
    attempts: u64,
) {
    let b = map.entry((provider, model.clone())).or_insert(ModelUsage {
        provider,
        model,
        tokens: Tokens::default(),
        observed_attempts: 0,
        reasoning_in_output: provider == Provider::Claude,
    });
    b.tokens.add(tokens);
    b.observed_attempts = b.observed_attempts.saturating_add(attempts);
}
// Source diagnostics retain their original session/file/line. This separate
// marker follows deduplicated usage to its accounting owner and then ancestors.
fn incomplete_owner(ds: &mut BTreeSet<Diagnostic>, owner: Option<usize>) {
    if owner.is_some() {
        diagnostic(ds, owner, None, None, "incomplete_usage_observation");
    }
}
fn invalid_subsets(t: Tokens) -> bool {
    t.cache_read.saturating_add(t.cache_write) > t.input || t.reasoning > t.output
}
fn disjoint(t: Tokens, ds: &mut BTreeSet<Diagnostic>, source: usize) -> Tokens {
    if invalid_subsets(t) {
        diagnostic(ds, Some(source), None, None, "inconsistent_delta_subsets");
    }
    Tokens {
        input: t
            .input
            .saturating_sub(t.cache_read)
            .saturating_sub(t.cache_write),
        output: t.output.saturating_sub(t.reasoning),
        ..t
    }
}
fn difference(now: Tokens, previous: Tokens) -> Tokens {
    Tokens::from_values(std::array::from_fn(|i| {
        now.values()[i].saturating_sub(previous.values()[i])
    }))
}

pub(super) fn aggregate(
    m: &Manifest,
    observations: Vec<Observation>,
    mut ds: BTreeSet<Diagnostic>,
) -> Report {
    let mut snapshots = BTreeMap::<(Provider, String), Snapshots>::new();
    let mut unique = BTreeMap::<(Provider, String), Identified>::new();
    let mut unresolved = Vec::new();
    let mut coverage = BTreeSet::<(Provider, String, String)>::new();
    let mut first = vec![None; m.sessions.len() + 1];
    let mut last = first.clone();
    for o in observations {
        let source = &m.sessions[o.session];
        let Event::Usage {
            key,
            native_session,
            model,
            tokens,
            cumulative,
        } = o.event
        else {
            continue;
        };
        let owner = native_session.as_ref().and_then(|id| {
            m.sessions
                .iter()
                .position(|s| s.id == *id && s.provider == source.provider)
        });
        if owner.is_none() {
            diagnostic(
                &mut ds,
                Some(o.session),
                None,
                None,
                if native_session.is_some() {
                    "unmapped_native_session"
                } else {
                    "unattributed_native_session"
                },
            );
        }
        if o.incomplete {
            incomplete_owner(&mut ds, owner);
        }
        let model = model.unwrap_or_else(|| "unknown".into());
        if model == "unknown" {
            diagnostic(&mut ds, Some(o.session), None, None, "unknown_model");
        }
        let incomplete = o.incomplete
            || model == "unknown"
            || (!cumulative && key.is_none())
            || (source.provider == Provider::Codex && tokens.is_some_and(invalid_subsets));
        if incomplete {
            incomplete_owner(&mut ds, owner);
        }
        let index = owner.unwrap_or(m.sessions.len());
        if let Some(t) = o.timestamp.filter(|_| tokens.is_some()) {
            if first[index].is_none() {
                first[index] = Some(t.clone());
            }
            last[index] = Some(t);
        }
        let group = native_session
            .clone()
            .unwrap_or_else(|| format!("unidentified-source-{}", o.session));
        if !cumulative {
            let Some(key) = key else {
                diagnostic(
                    &mut ds,
                    Some(o.session),
                    None,
                    None,
                    "usage_without_stable_identity",
                );
                let Some(tokens) = tokens else { continue };
                unresolved.push(UnresolvedUsage {
                    provider: source.provider,
                    model,
                    tokens: if source.provider == Provider::Codex {
                        disjoint(tokens, &mut ds, o.session)
                    } else {
                        tokens
                    },
                    reason: "missing_stable_identity_non_additive",
                });
                continue;
            };
            coverage.insert((source.provider, group.clone(), key.clone()));
            unique
                .entry((source.provider, key))
                .and_modify(|old| {
                    let ownership_conflict = old.native_session.is_some()
                        && native_session.is_some()
                        && old.native_session != native_session;
                    if ownership_conflict {
                        incomplete_owner(&mut ds, old.owner);
                        incomplete_owner(&mut ds, owner);
                        old.ambiguous_owner = true;
                        old.owner = None;
                    } else if !old.ambiguous_owner && old.native_session.is_none() {
                        old.native_session = native_session.clone();
                        old.owner = owner;
                    }
                    let model_conflict =
                        old.model != model && old.model != "unknown" && model != "unknown";
                    if ownership_conflict || model_conflict {
                        old.incomplete = true;
                        incomplete_owner(&mut ds, old.owner);
                        incomplete_owner(&mut ds, owner);
                        diagnostic(
                            &mut ds,
                            Some(o.session),
                            None,
                            None,
                            "conflicting_message_identity",
                        );
                        diagnostic(
                            &mut ds,
                            Some(old.source),
                            None,
                            None,
                            "conflicting_message_identity",
                        );
                    }
                    if model_conflict {
                        // Conflicting copies cannot establish a model, even if
                        // a later copy repeats one of the conflicting names.
                        old.ambiguous_model = true;
                        old.model = "unknown".into();
                    } else if old.model == "unknown" && !old.ambiguous_model {
                        old.model = model.clone();
                    }
                    if let Some(tokens) = tokens {
                        old.tokens.get_or_insert_default().max(tokens);
                    }
                    old.incomplete |= incomplete;
                })
                .or_insert(Identified {
                    owner,
                    native_session,
                    ambiguous_owner: false,
                    ambiguous_model: false,
                    incomplete,
                    source: o.session,
                    model,
                    tokens,
                });
        } else {
            let Some(tokens) = tokens else { continue };
            if native_session.is_none() {
                unresolved.push(UnresolvedUsage {
                    provider: source.provider,
                    model,
                    tokens: disjoint(tokens, &mut ds, o.session),
                    reason: "missing_native_session_non_additive",
                });
                continue;
            }
            let state = snapshots
                .entry((source.provider, group))
                .or_insert(Snapshots {
                    owner,
                    source: o.session,
                    ..Snapshots::default()
                });
            // Old token_count records do not expose cache-write counters; absence cannot imply regression.
            if [0, 4, 5, 6]
                .iter()
                .any(|i| tokens.values()[*i] < state.highwater.values()[*i])
            {
                diagnostic(
                    &mut ds,
                    Some(o.session),
                    None,
                    None,
                    "cumulative_counter_regressed",
                );
                incomplete_owner(&mut ds, owner);
            }
            let delta = difference(tokens, state.highwater);
            state.ambiguous_subsets |= invalid_subsets(delta);
            state.highwater.max(tokens);
            let attempts = u64::from(delta.total() > 0);
            add(&mut state.fallback, source.provider, model, delta, attempts);
        }
    }
    let mut buckets: Vec<ModelMap> = vec![BTreeMap::new(); m.sessions.len() + 1];
    let mut identified_totals = BTreeMap::<(Provider, String), Tokens>::new();
    for (provider, group, key) in coverage {
        // A partial copy still identifies the response covered by a snapshot.
        // Use its globally merged counters so missing fields in that copy do
        // not become a second cumulative remainder.
        if let Some(tokens) = unique.get(&(provider, key)).and_then(|u| u.tokens) {
            identified_totals
                .entry((provider, group))
                .or_default()
                .add(tokens);
        }
    }
    for ((provider, _), u) in unique {
        if u.incomplete {
            incomplete_owner(&mut ds, u.owner);
        }
        // Retain identity and diagnostic evidence even when a streamed/copy
        // record has no counters, but never turn that absence into numeric zero.
        let Some(mut tokens) = u.tokens else { continue };
        if tokens.cache_write_5m.saturating_add(tokens.cache_write_1h) > tokens.cache_write {
            diagnostic(
                &mut ds,
                Some(u.source),
                None,
                None,
                "conflicting_cache_partitions",
            );
            incomplete_owner(&mut ds, u.owner);
            // Keep the observed aggregate; duration evidence remains visibly inconsistent.
        }
        if provider == Provider::Codex {
            if invalid_subsets(tokens) {
                incomplete_owner(&mut ds, u.owner);
            }
            tokens = disjoint(tokens, &mut ds, u.source);
        }
        add(
            &mut buckets[u.owner.unwrap_or(m.sessions.len())],
            provider,
            u.model,
            tokens,
            1,
        );
    }
    for ((provider, group), s) in snapshots {
        let bucket = &mut buckets[s.owner.unwrap_or(m.sessions.len())];
        if let Some(identified) = identified_totals.get(&(provider, group)) {
            let remainder = difference(s.highwater, *identified);
            if invalid_subsets(remainder) {
                diagnostic(
                    &mut ds,
                    Some(s.source),
                    None,
                    None,
                    "inconsistent_cumulative_remainder",
                );
                incomplete_owner(&mut ds, s.owner);
                unresolved.push(UnresolvedUsage {
                    provider,
                    model: "unknown".into(),
                    tokens: disjoint(remainder, &mut ds, s.source),
                    reason: "inconsistent_cumulative_remainder_non_additive",
                });
            } else if remainder.total() > 0 {
                diagnostic(
                    &mut ds,
                    Some(s.source),
                    None,
                    None,
                    "unidentified_cumulative_remainder",
                );
                incomplete_owner(&mut ds, s.owner);
                add(
                    bucket,
                    provider,
                    "unknown".into(),
                    disjoint(remainder, &mut ds, s.source),
                    0,
                );
            }
        } else if s.ambiguous_subsets && s.fallback.len() > 1 {
            diagnostic(
                &mut ds,
                Some(s.source),
                None,
                None,
                "ambiguous_cumulative_model_attribution",
            );
            incomplete_owner(&mut ds, s.owner);
            let attempts = s.fallback.values().map(|u| u.observed_attempts).sum();
            add(
                bucket,
                provider,
                "unknown".into(),
                disjoint(s.highwater, &mut ds, s.source),
                attempts,
            );
        } else {
            for usage in s.fallback.into_values() {
                if invalid_subsets(usage.tokens) {
                    incomplete_owner(&mut ds, s.owner);
                }
                add(
                    bucket,
                    provider,
                    usage.model,
                    disjoint(usage.tokens, &mut ds, s.source),
                    usage.observed_attempts,
                );
            }
        }
    }
    assemble::assemble(m, buckets, unresolved, ds, first, last)
}
