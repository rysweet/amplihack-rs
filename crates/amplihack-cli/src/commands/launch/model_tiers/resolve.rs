//! Issue #1619: the pure resolver — the single decision point for a tier.
//!
//! Everything the feature decides about one launch is decided here, from
//! values the caller gathered. Purity is a security property as much as a
//! testing one: every rejection path is reachable from a unit test rather
//! than from a process spawn.
//!
//! The guarantees G1 to G4 are pinned by `tests_resolve.rs`:
//!
//! - **G1** no tier word ever leaves as a model value, on any harness. An
//!   unresolved tier word asks the CLI for a model called `strong` and breaks
//!   the step;
//! - **G2** the argv is byte-identical when there is no tier and no force;
//! - **G3** `--model` occurrences are replaced or deleted, never inserted by
//!   tier resolution;
//! - **G4** a concrete id is never read or rewritten (issue #1527), the one
//!   exception being `model.force`, which is announced.
//!
//! G5 is a property of the module rather than a test: it reads no
//! environment, no file and no clock. Every input arrives through [`Request`].
//!
//! Falling back removes the tier pair and substitutes nothing of its own. The
//! existing chain in `model_args` then decides, which is what lets a gateway
//! launch get `--model $AMPLIHACK_LITELLM_MODEL` in the tier's place. No path
//! here invents a model name (issue #1421).

use super::argv_scan::{ModelOccurrence, find_model_options, options_end};
use super::notice::Notice;
use super::record::{Fallback, Record};
use super::table::{Effort, Entry, Table, Tier, column_for, validate_model_id};

/// Everything the resolver needs about one launch.
pub(crate) struct Request<'a> {
    /// The binary amplihack is about to start, by name.
    pub(crate) harness: &'a str,
    /// The passthrough tail, as the caller gave it.
    pub(crate) argv: &'a [String],
    /// The shipped mapping with any instance override already merged in.
    pub(crate) table: &'a Table,
    /// `model.force` or `AMPLIHACK_FORCE_MODEL`, unvalidated.
    pub(crate) force: Option<&'a str>,
    /// A set, non-blank `AMPLIHACK_DEFAULT_MODEL`, unvalidated. Read on the
    /// Claude column only, where it outranks the shipped tier model: before
    /// this feature it chose the model of every Claude-compatible launch and
    /// was never read for Codex or Copilot, which it still is not.
    pub(crate) default_model: Option<&'a str>,
    /// Whether this launch is routed through the LiteLLM gateway.
    pub(crate) through_gateway: bool,
}

/// What the launch should do instead.
#[derive(Debug, Default)]
pub(crate) struct Resolution {
    /// The passthrough tail with every tier resolved.
    pub(crate) argv: Vec<String>,
    /// Lines for stderr, in order.
    pub(crate) notices: Vec<Notice>,
    /// The measurement record, when a tier or a forced model was involved.
    pub(crate) record: Option<Record>,
    /// The reasoning effort the tier asks for, where the harness has one.
    pub(crate) effort: Option<Effort>,
}

/// What happens to one argv position.
enum Action {
    Keep,
    Replace(String),
    Delete,
}

/// Resolve every tier on one command line.
pub(crate) fn resolve(request: &Request<'_>) -> Resolution {
    let column = column_for(request.harness);
    let occurrences = find_model_options(request.argv, column == "codex");

    // A gateway launch routes on the model name and has no default of its
    // own, so a forced model cannot route past it either.
    let force = if request.through_gateway {
        None
    } else {
        request.force
    };
    let (force, force_notice) = match force {
        Some(raw) => match validate_model_id(raw) {
            Ok(()) => (Some(raw), None),
            Err(reason) => (None, Some(Notice::ForcedRefused { reason })),
        },
        None => (None, None),
    };

    // The operator's own `AMPLIHACK_DEFAULT_MODEL` applies to the Claude
    // column only, as it did before tiers: `column_for` is the same
    // Claude-compatible test `model_args` uses. Elsewhere it is dropped before
    // validation, so it leaves no notice, no record and no argv token; a
    // Claude id on a Codex command line would fail the step. On the Claude
    // column it is held to the same check as `model.force` by the same code,
    // because it is an environment variable and the string lands as the argv
    // token straight after `--model`. Blank means "pass nothing and let the
    // CLI decide", and the gateway routes on the model name, so neither
    // displaces the table.
    let default_model = request
        .default_model
        .filter(|_| column == "claude")
        .filter(|raw| !raw.trim().is_empty())
        .filter(|_| !request.through_gateway);
    let (default_model, default_notice) = match default_model {
        Some(raw) => match validate_model_id(raw) {
            Ok(()) => (Some(raw), None),
            Err(reason) => (None, Some(Notice::DefaultModelRefused { reason })),
        },
        None => (None, None),
    };

    let mut out = Resolution {
        argv: Vec::new(),
        notices: force_notice.into_iter().collect(),
        record: None,
        effort: None,
    };
    let mut actions: Vec<Action> = (0..request.argv.len()).map(|_| Action::Keep).collect();
    let mut seen: Option<Seen> = None;

    for occurrence in &occurrences {
        let tier = Tier::parse(&occurrence.value);
        let decision = decide(request, &column, tier, force, default_model);
        if let Some(tier) = tier {
            let cell = request.table.entry(&column, tier);
            seen = Some(Seen {
                tier,
                requested: cell.and_then(|entry| entry.model.clone()),
                requested_effort: cell.and_then(|entry| entry.effort),
                decision: decision.clone(),
            });
        }
        apply(&mut actions, occurrence, &decision);
    }

    // `model.force` is the documented exception to issue #1527: it replaces
    // even a concrete id, and supplies one where the command line has none.
    if let Some(model) = force {
        out.notices.push(Notice::Forced {
            harness: request.harness.to_string(),
            model: model.to_string(),
        });
    }

    // Said once, and only where the variable actually decided something here:
    // with no tier on the command line it displaced nothing, and the untiered
    // path in `model_args` prints its own line about the same value, refusal
    // included. A forced model outranks it, so there is nothing to report.
    if force.is_none() && seen.is_some() {
        match (default_model, default_notice) {
            (Some(model), _) => out.notices.push(Notice::DefaultModel {
                harness: request.harness.to_string(),
                model: model.to_string(),
            }),
            (None, Some(notice)) => out.notices.push(notice),
            (None, None) => {}
        }
    }

    out.argv = rewrite(request.argv, &actions);
    if let Some(model) = force
        && occurrences.is_empty()
    {
        // The pair goes inside amplihack's own region of the command line,
        // before the end-of-options marker. Appended instead it would land
        // past `--`, which `passthrough_rewrite` forwards verbatim, so
        // `--model <id>` would reach the harness as positional text while
        // `Notice::Forced` said it was applied. The index comes from the same
        // `argv_scan` function that decided there were no occurrences, so the
        // scanner and the inserter cannot disagree about where options end.
        let at = options_end(&out.argv);
        out.argv
            .splice(at..at, ["--model".to_string(), model.to_string()]);
    }

    if let Some(seen) = seen {
        let effort = effective_effort(
            &column,
            seen.requested_effort,
            &seen.decision,
            request.table.max_effort(&column),
        );
        out.effort = effort;
        // A fall-back that still carried a reasoning effort is announced by
        // the caller's one Applied line instead, so a launch never gets two
        // lines about one tier.
        if let Some(notice) = seen
            .decision
            .notice(request, &column, seen.tier)
            .filter(|_| effort.is_none())
        {
            out.notices.push(notice);
        }
        out.record = Some(Record {
            tier: Some(seen.tier),
            requested_model: seen.requested,
            effective_model: seen.decision.model(),
            requested_effort: seen.requested_effort,
            effective_effort: effort,
            fallback: seen.decision.fallback(),
            ..blank_record(request.harness, column)
        });
    } else if let Some(model) = force {
        // No tier was asked for, so every tier field stays empty and the one
        // thing this launch has to report is the model it was forced onto.
        out.record = Some(Record {
            effective_model: Some(model.to_string()),
            ..blank_record(request.harness, column)
        });
    }
    out
}

/// A record of this launch with nothing decided yet: the schema version, the
/// sequence number `record::append` assigns under the lock, the run id the
/// caller fills in, and the two names that are the same whatever was decided.
fn blank_record(harness: &str, column: String) -> Record {
    Record {
        v: 1,
        seq: 0,
        run_id: None,
        harness: harness.to_string(),
        column,
        tier: None,
        requested_model: None,
        effective_model: None,
        requested_effort: None,
        effective_effort: None,
        fallback: None,
    }
}

/// The last tier the command line asked for, and what became of it.
struct Seen {
    tier: Tier,
    requested: Option<String>,
    requested_effort: Option<Effort>,
    decision: Decision,
}

/// What one `--model` value becomes.
#[derive(Clone)]
enum Decision {
    /// Not a tier, and no force: forwarded exactly as typed (issue #1527).
    AsTyped,
    /// Replaced with this model.
    Model(String),
    /// The pair is removed and the harness's own default model runs.
    Removed(Fallback),
}

impl Decision {
    fn model(&self) -> Option<String> {
        match self {
            Decision::Model(model) => Some(model.clone()),
            _ => None,
        }
    }

    fn fallback(&self) -> Option<Fallback> {
        match self {
            Decision::Removed(reason) => Some(*reason),
            _ => None,
        }
    }

    /// The one line this decision prints, or `None` when there is nothing to
    /// say: a resolved tier is not news, and one launch gets one line.
    fn notice(&self, request: &Request<'_>, column: &str, tier: Tier) -> Option<Notice> {
        let harness = request.harness.to_string();
        let column = column.to_string();
        match self {
            Decision::Removed(Fallback::NoColumn) => Some(Notice::NoColumn {
                harness,
                column,
                tier,
            }),
            Decision::Removed(Fallback::NoMapping) => Some(Notice::NoMapping {
                harness,
                column,
                tier,
            }),
            Decision::Removed(Fallback::NoModel) => Some(Notice::NoModel {
                harness,
                column,
                tier,
            }),
            Decision::Removed(Fallback::Gateway) => Some(Notice::Gateway { harness, tier }),
            Decision::AsTyped | Decision::Model(_) => None,
        }
    }
}

/// What one `--model` value becomes, given the table, the force and the
/// gateway.
fn decide(
    request: &Request<'_>,
    column: &str,
    tier: Option<Tier>,
    force: Option<&str>,
    default_model: Option<&str>,
) -> Decision {
    let Some(tier) = tier else {
        // A concrete id. Only a forced model touches it.
        return match force {
            Some(model) => Decision::Model(model.to_string()),
            None => Decision::AsTyped,
        };
    };
    if request.through_gateway {
        return Decision::Removed(Fallback::Gateway);
    }
    if let Some(model) = force {
        return Decision::Model(model.to_string());
    }
    // Rank 3, Claude column only (`resolve` passes `None` elsewhere): below
    // `model.force` and the concrete id handled above, and above the table.
    // The Claude column has no effort launch control, so the tier's effort is
    // recorded but not applied.
    if let Some(model) = default_model {
        return Decision::Model(model.to_string());
    }
    match request.table.entry(column, tier) {
        None if request.table.has_column(column) => Decision::Removed(Fallback::NoMapping),
        None => Decision::Removed(Fallback::NoColumn),
        Some(Entry {
            model: Some(model), ..
        }) => Decision::Model(model.clone()),
        // A cell that names an effort and no model is still a fall-back for
        // the model: the harness's own runs. Every shipped Codex cell is one.
        Some(_) => Decision::Removed(Fallback::NoModel),
    }
}

/// The effort that actually reaches the harness. Only Codex has a launch
/// control for it; elsewhere a configured effort is recorded and documented
/// as not a launch control, never silently written onto a command line.
///
/// `ceiling` is the column's declared `max_effort`, and only ever lowers: an
/// account whose model refuses `xhigh` says so in its own file, and the step
/// runs one step down rather than failing. Amplihack does not detect a
/// refusal — that needs a live call, and there is no probe, retry or timer.
fn effective_effort(
    column: &str,
    requested: Option<Effort>,
    decision: &Decision,
    ceiling: Option<Effort>,
) -> Option<Effort> {
    match decision {
        Decision::AsTyped => None,
        _ if column == "codex" => match (requested, ceiling) {
            (Some(effort), Some(cap)) => Some(effort.min(cap)),
            (effort, _) => effort,
        },
        _ => None,
    }
}

/// Mark what happens to one occurrence's positions.
fn apply(actions: &mut [Action], occurrence: &ModelOccurrence, decision: &Decision) {
    match decision {
        Decision::AsTyped => {}
        Decision::Model(model) => {
            if occurrence.attached() {
                actions[occurrence.flag_index] = Action::Replace(format!("--model={model}"));
            } else if let Some(index) = occurrence.value_index {
                actions[index] = Action::Replace(model.clone());
            }
        }
        Decision::Removed(_) => {
            actions[occurrence.flag_index] = Action::Delete;
            if let Some(index) = occurrence.value_index {
                actions[index] = Action::Delete;
            }
        }
    }
}

/// The command line with every marked position replaced or dropped.
fn rewrite(argv: &[String], actions: &[Action]) -> Vec<String> {
    argv.iter()
        .zip(actions)
        .filter_map(|(arg, action)| match action {
            Action::Keep => Some(arg.clone()),
            Action::Replace(value) => Some(value.clone()),
            Action::Delete => None,
        })
        .collect()
}
