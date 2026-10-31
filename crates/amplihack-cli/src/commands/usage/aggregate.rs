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
    source: usize,
    model: String,
    tokens: Tokens,
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
    let mut contexts = BTreeMap::new();
    let mut snapshots = BTreeMap::<(Provider, String), Snapshots>::new();
    let mut unique = BTreeMap::<(Provider, String), Identified>::new();
    let mut unresolved = Vec::new();
    let mut coverage = BTreeMap::<(Provider, String, String), Tokens>::new();
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
            if let Event::Model(model) = o.event {
                contexts.insert(o.session, model);
            }
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
        let model = model
            .or_else(|| contexts.get(&o.session).cloned())
            .unwrap_or_else(|| "unknown".into());
        if model == "unknown" {
            diagnostic(&mut ds, Some(o.session), None, None, "unknown_model");
        }
        let index = owner.unwrap_or(m.sessions.len());
        if let Some(t) = o.timestamp {
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
            coverage
                .entry((source.provider, group.clone(), key.clone()))
                .or_default()
                .max(tokens);
            unique
                .entry((source.provider, key))
                .and_modify(|old| {
                    if old.owner.is_none() && owner.is_some() {
                        old.owner = owner;
                    }
                    if old.owner != owner && owner.is_some()
                        || old.model != model && old.model != "unknown" && model != "unknown"
                    {
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
                    if old.model == "unknown" {
                        old.model = model.clone();
                    }
                    old.tokens.max(tokens);
                })
                .or_insert(Identified {
                    owner,
                    source: o.session,
                    model,
                    tokens,
                });
        } else {
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
    for ((provider, group, _), tokens) in coverage {
        identified_totals
            .entry((provider, group))
            .or_default()
            .add(tokens);
    }
    for ((provider, _), u) in unique {
        let mut tokens = u.tokens;
        if tokens.cache_write_5m.saturating_add(tokens.cache_write_1h) > tokens.cache_write {
            diagnostic(
                &mut ds,
                Some(u.source),
                None,
                None,
                "conflicting_cache_partitions",
            );
            // Keep the observed aggregate; duration evidence remains visibly inconsistent.
        }
        if provider == Provider::Codex {
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
