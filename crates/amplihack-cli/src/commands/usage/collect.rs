//! Read explicit files, deduplicate globally, then aggregate direct and inclusive totals.
use super::{
    adapters::{self, Event},
    manifest::validate,
    models::*,
};
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

type ModelMap = BTreeMap<(Provider, String), ModelUsage>;
struct Observation {
    session: usize,
    timestamp: Option<String>,
    event: Event,
}

fn diagnostic(
    ds: &mut BTreeSet<Diagnostic>,
    session: Option<usize>,
    file: Option<usize>,
    line: Option<usize>,
    code: &str,
) {
    ds.insert(Diagnostic {
        session,
        file,
        line,
        code: code.to_owned(),
    });
}

pub fn collect(m: &Manifest, base: &Path) -> Result<Report> {
    validate(m)?;
    let mut ds = BTreeSet::new();
    let mut observations = Vec::new();
    for (si, s) in m.sessions.iter().enumerate() {
        if !s.complete {
            diagnostic(
                &mut ds,
                Some(si),
                None,
                None,
                "session_coverage_unconfirmed",
            );
        }
        if s.run_id.is_none() {
            diagnostic(&mut ds, Some(si), None, None, "unattributed_run");
        }
        if s.parent
            .as_ref()
            .is_some_and(|p| !m.sessions.iter().any(|s| &s.id == p))
        {
            diagnostic(&mut ds, Some(si), None, None, "missing_parent");
        }
        if s.files.is_empty() {
            diagnostic(&mut ds, Some(si), None, None, "missing_files");
        }
        for (fi, path) in s.files.iter().enumerate() {
            if !std::fs::metadata(base.join(path)).is_ok_and(|meta| meta.is_file()) {
                diagnostic(&mut ds, Some(si), Some(fi), None, "unreadable_file");
                continue;
            }
            let Ok(file) = File::open(base.join(path)) else {
                diagnostic(&mut ds, Some(si), Some(fi), None, "unreadable_file");
                continue;
            };
            let mut native = None;
            for (li, line) in BufReader::new(file).lines().enumerate() {
                let Ok(line) = line else {
                    diagnostic(
                        &mut ds,
                        Some(si),
                        Some(fi),
                        Some(li + 1),
                        "unreadable_record",
                    );
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str(&line) else {
                    diagnostic(
                        &mut ds,
                        Some(si),
                        Some(fi),
                        Some(li + 1),
                        "invalid_json_record",
                    );
                    continue;
                };
                let mut parsed = adapters::parse(s.provider, &v);
                for code in parsed.diagnostics {
                    diagnostic(&mut ds, Some(si), Some(fi), Some(li + 1), code);
                }
                if let Some(Event::NativeSession(id)) = parsed.event {
                    native = Some(id);
                    continue;
                }
                if let Some(Event::Usage { native_session, .. }) = &mut parsed.event {
                    if native_session.is_none() {
                        *native_session = native.clone();
                    }
                    if native_session.is_none() {
                        diagnostic(
                            &mut ds,
                            Some(si),
                            Some(fi),
                            Some(li + 1),
                            "missing_native_session_identity",
                        );
                    }
                }
                if let Some(event) = parsed.event {
                    observations.push(Observation {
                        session: if s.provider == Provider::Codex {
                            native
                                .as_ref()
                                .and_then(|id| {
                                    m.sessions.iter().position(|candidate| {
                                        candidate.id == *id && candidate.provider == s.provider
                                    })
                                })
                                .unwrap_or(si)
                        } else {
                            si
                        },
                        timestamp: parsed.timestamp,
                        event,
                    });
                }
            }
        }
    }
    // Resumed and copied files can arrive out of order. Keep only sanitized events in memory.
    observations.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    let mut contexts = BTreeMap::new();
    let mut highwater = BTreeMap::<usize, Tokens>::new();
    let mut unique = BTreeMap::<(Provider, String), (usize, String, Tokens)>::new();
    let mut buckets: Vec<ModelMap> = vec![BTreeMap::new(); m.sessions.len()];
    let mut first = vec![None; m.sessions.len()];
    let mut last = first.clone();
    for o in observations {
        let source = &m.sessions[o.session];
        match o.event {
            Event::Model(model) => {
                contexts.insert(o.session, model);
            }
            Event::NativeSession(_) => unreachable!(),
            Event::Usage {
                key,
                native_session,
                model,
                tokens,
                cumulative,
            } => {
                let owner = native_session.as_ref().and_then(|id| {
                    m.sessions
                        .iter()
                        .position(|s| s.id == *id && s.provider == source.provider)
                });
                let si = owner.unwrap_or(o.session);
                if native_session.is_some() && owner.is_none() {
                    diagnostic(&mut ds, Some(si), None, None, "unmapped_native_session");
                    // Ambiguous ownership must not be attributed to a different manifest session.
                    continue;
                }
                let model = model
                    .or_else(|| contexts.get(&si).cloned())
                    .unwrap_or_else(|| "unknown".into());
                if model == "unknown" {
                    diagnostic(&mut ds, Some(si), None, None, "unknown_model");
                }
                if let Some(t) = o.timestamp {
                    if first[si].is_none() {
                        first[si] = Some(t.clone());
                    }
                    last[si] = Some(t);
                }
                let provider = m.sessions[si].provider;
                if provider == Provider::Claude || !cumulative {
                    let Some(key) = key else {
                        diagnostic(
                            &mut ds,
                            Some(si),
                            None,
                            None,
                            "usage_without_stable_identity",
                        );
                        continue;
                    };
                    // Provider IDs are global, independent of copied history or input file.
                    unique
                        .entry((provider, key))
                        .and_modify(|(old_si, old_model, old)| {
                            if *old_si != si || *old_model != model {
                                diagnostic(
                                    &mut ds,
                                    Some(si),
                                    None,
                                    None,
                                    "conflicting_message_identity",
                                );
                            }
                            old.max(tokens);
                        })
                        .or_insert((si, model, tokens));
                } else {
                    let prior = highwater.entry(si).or_default();
                    let previous = prior.values();
                    let now = tokens.values();
                    if now.iter().zip(previous).any(|(n, p)| *n < p) {
                        diagnostic(
                            &mut ds,
                            Some(si),
                            None,
                            None,
                            "cumulative_counter_regressed",
                        );
                    }
                    let delta = Tokens::from_values(std::array::from_fn(|i| {
                        now[i].saturating_sub(previous[i])
                    }));
                    prior.max(tokens);
                    if delta.total() > 0 {
                        add(
                            &mut buckets[si],
                            provider,
                            model,
                            codex_disjoint(delta, &mut ds, si),
                            1,
                        );
                    }
                }
            }
        }
    }
    for ((provider, _), (si, model, t)) in unique {
        if provider == Provider::Codex && highwater.contains_key(&si) {
            diagnostic(
                &mut ds,
                Some(si),
                None,
                None,
                "mixed_cumulative_and_last_only_usage",
            );
            continue;
        }
        let t = if provider == Provider::Codex {
            codex_disjoint(t, &mut ds, si)
        } else {
            t
        };
        add(&mut buckets[si], provider, model, t, 1);
    }
    let mut all = ModelMap::new();
    let mut sessions: Vec<_> = m
        .sessions
        .iter()
        .enumerate()
        .map(|(si, s)| {
            let mut direct = Tokens::default();
            for b in buckets[si].values() {
                direct.add(b.tokens);
                add(
                    &mut all,
                    b.provider,
                    b.model.clone(),
                    b.tokens,
                    b.observed_attempts,
                );
            }
            if buckets[si].is_empty() {
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
            sessions[pi].inclusive.add(tokens);
            sessions[pi].complete &= complete;
            parent = sessions[pi].parent.clone();
        }
    }
    let mut total = Tokens::default();
    let mut unattributed = Tokens::default();
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
    for s in &sessions {
        total.add(s.direct);
        if missing_identity || s.run_id.is_none() {
            unattributed.add(s.direct);
        }
    }
    Ok(Report {
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
        models: all.into_values().collect(),
        sessions,
        complete: ds.is_empty(),
        diagnostics: ds.into_iter().collect(),
        first_observed: first.into_iter().flatten().min(),
        last_observed: last.into_iter().flatten().max(),
        api_price_estimate: None,
        price_reference: None,
        price_assumptions: "Unknown: no versioned price table supplied; observations are not subscription quota or invoices.",
        attempt_semantics: "Claude: unique message IDs. Codex: positive cumulative transitions or unique request IDs; failed/unrecorded calls are unknown, not zero.",
    })
}
fn codex_disjoint(t: Tokens, ds: &mut BTreeSet<Diagnostic>, si: usize) -> Tokens {
    if t.cache_read > t.input || t.reasoning > t.output {
        diagnostic(ds, Some(si), None, None, "inconsistent_delta_subsets");
    }
    Tokens {
        input: t.input.saturating_sub(t.cache_read),
        output: t.output.saturating_sub(t.reasoning),
        ..t
    }
}
fn add(map: &mut ModelMap, provider: Provider, model: String, tokens: Tokens, attempts: u64) {
    let b = map.entry((provider, model.clone())).or_insert(ModelUsage {
        provider,
        model,
        tokens: Tokens::default(),
        observed_attempts: 0,
    });
    b.tokens.add(tokens);
    b.observed_attempts += attempts;
}
