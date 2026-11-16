//! Issue #1619: one typed diagnostic per thing worth saying about a tier.
//!
//! The stderr line and the recorded fall-back reason both derive from one
//! value, so they cannot disagree. Tests assert variants and fields, never
//! rendered strings.
//!
//! A notice never echoes argv, a prompt or a model the operator did not
//! configure: these lines land in run logs that get pasted into issues.

use super::table::{ACCEPTED_EFFORTS, Effort, TIER_WORDS, Tier};
use crate::user_config::safe::display_safe;

/// Something amplihack decided about a tier, or found in a tier file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notice {
    /// The tier gave this launch a model, a reasoning effort, or both. One
    /// line per tiered launch, as the existing issue #1421 model line is.
    Applied {
        harness: String,
        tier: Tier,
        model: Option<String>,
        effort: Option<Effort>,
    },
    /// The command line already set the reasoning effort, so the tier gave
    /// this launch its model only.
    EffortCallerChose { harness: String, tier: Tier },
    /// The harness has no column at all, so the tier has nothing to resolve
    /// to and the harness's own default model runs.
    NoColumn {
        harness: String,
        column: String,
        tier: Tier,
    },
    /// The column exists but has no cell for this tier.
    NoMapping {
        harness: String,
        column: String,
        tier: Tier,
    },
    /// The cell exists and names no model, which is how an instance says the
    /// model is not available to it.
    NoModel {
        harness: String,
        column: String,
        tier: Tier,
    },
    /// The launch goes through the LiteLLM gateway, which routes on the model
    /// name and was never configured to route the tier's model.
    Gateway { harness: String, tier: Tier },
    /// `model.force` replaced what the command line asked for.
    Forced { harness: String, model: String },
    /// `model.force` named something that could act as another argument.
    ForcedRefused { reason: String },
    /// A tier file could not be read or parsed; the shipped mapping applies.
    TableLoadFailed { path: String, reason: String },
    /// A tier file names a word that is not one of the three tiers.
    UnknownTier { column: String, word: String },
    /// A tier file's cell carries a field amplihack does not know.
    UnknownField {
        column: String,
        tier: String,
        field: String,
    },
    /// A tier file's cell names a model amplihack will not put on a command
    /// line; the cell is dropped and the shipped value stays.
    RejectedModel {
        column: String,
        tier: String,
        reason: String,
    },
    /// A tier file's cell names an effort outside the accepted list; the
    /// effort is dropped and the model still applies.
    BadEffort {
        column: String,
        tier: String,
        value: String,
    },
    /// A `[step_tiers]` entry whose value is not one of the three tiers.
    BadStepTier { step: String, value: String },
    /// A `[step_tiers]` key that is not an identifier, so amplihack will not
    /// act on it.
    BadStepId { step: String },
    /// A `[step_tiers]` key that is a step id no recipe in this run's search
    /// path declares. The likeliest operator error in the feature, and the
    /// one with no other symptom: the run simply proceeds on the tier the
    /// recipe declares.
    StepTierUnmatched { step: String },
    /// A tier file's top-level key is not a table.
    NotATable { key: String },
}

/// Where the advice at the end of a line points. Every line ends the same
/// way the existing `--model` lines do, so an operator always learns which
/// setting to change.
const ADVICE: &str = "See `amplihack config get model.tier_map` and docs/reference/model-tiers.md.";

impl Notice {
    /// The one stderr line for this notice. Built only from the fields above
    /// and fixed text, never from argv.
    pub(crate) fn render(&self) -> String {
        match self {
            Notice::Applied {
                harness,
                tier,
                model,
                effort,
            } => {
                let what = match (model.as_deref(), effort) {
                    (Some(model), Some(effort)) => format!(
                        "passing `--model {model}` and `-c model_reasoning_effort={}`",
                        effort.as_str()
                    ),
                    (Some(model), None) => format!("passing `--model {model}`"),
                    (None, Some(effort)) => format!(
                        "passing `-c model_reasoning_effort={}`; the model stays `{harness}`'s own",
                        effort.as_str()
                    ),
                    // The caller does not emit this variant when the tier
                    // gave the launch neither of the two — a fall-back line
                    // has already said so. Worded for truth rather than left
                    // as a placeholder, in case a later caller does.
                    (None, None) => {
                        format!("the model and effort stay `{harness}`'s own")
                    }
                };
                format!(
                    "amplihack: model tier `{}` on `{harness}`: {what}. {ADVICE}",
                    tier.as_str()
                )
            }
            Notice::EffortCallerChose { harness, tier } => format!(
                "amplihack: model tier `{}` on `{harness}`: leaving the \
                 `-c model_reasoning_effort` already on the command line alone.",
                tier.as_str()
            ),
            Notice::NoColumn {
                harness,
                column,
                tier,
            } => format!(
                "amplihack: model tier `{}` has no `{column}` column, so `{harness}` runs on its \
                 own default model. {ADVICE}",
                tier.as_str()
            ),
            Notice::NoMapping {
                harness,
                column,
                tier,
            } => format!(
                "amplihack: model tier `{}` has no entry in the `{column}` column, so \
                 `{harness}` runs on its own default model. {ADVICE}",
                tier.as_str()
            ),
            Notice::NoModel {
                harness,
                column,
                tier,
            } => format!(
                "amplihack: model tier `{}` names no model in the `{column}` column, so \
                 `{harness}` runs on its own default model. {ADVICE}",
                tier.as_str()
            ),
            Notice::Gateway { harness, tier } => format!(
                "amplihack: model tier `{}` is not applied to `{harness}`: the LiteLLM gateway \
                 routes on the model name. Set AMPLIHACK_LITELLM_MODEL to change it.",
                tier.as_str()
            ),
            Notice::Forced { harness, model } => format!(
                "amplihack: model.force is set, so `{harness}` is launched with `--model {model}` \
                 whatever the step asked for. Unset model.force or AMPLIHACK_FORCE_MODEL to \
                 restore per-step tiers."
            ),
            Notice::ForcedRefused { reason } => {
                format!("amplihack: model.force is ignored: {reason}. The step's own tier applies.")
            }
            Notice::TableLoadFailed { path, reason } => format!(
                "amplihack: {path}: {reason}; the model tiers amplihack ships are used instead."
            ),
            Notice::UnknownTier { column, word } => {
                let (column, word) = (display_safe(column), display_safe(word));
                format!(
                    "amplihack: model tiers: `{column}.{word}`: `{word}` is not a tier; the \
                     entry is ignored. The tiers are {}.",
                    TIER_WORDS.join(", ")
                )
            }
            Notice::UnknownField {
                column,
                tier,
                field,
            } => format!(
                "amplihack: model tiers: `{}.{}`: unknown field `{}`; it is ignored. A cell \
                 may name `model` and `effort`.",
                display_safe(column),
                display_safe(tier),
                display_safe(field)
            ),
            Notice::RejectedModel {
                column,
                tier,
                reason,
            } => format!(
                "amplihack: model tiers: `{}.{}`: the model {reason}; the entry is ignored \
                 and the shipped value applies.",
                display_safe(column),
                display_safe(tier)
            ),
            Notice::BadEffort {
                column,
                tier,
                value,
            } => format!(
                "amplihack: model tiers: `{}.{}`: `{}` is not an accepted reasoning effort; \
                 it is ignored and the model still applies. The accepted values are {}.",
                display_safe(column),
                display_safe(tier),
                display_safe(value),
                ACCEPTED_EFFORTS.join(", ")
            ),
            Notice::BadStepTier { step, value } => format!(
                "amplihack: model tiers: step_tiers.{}: `{}` is not a tier; the step keeps \
                 the tier its recipe declares.",
                display_safe(step),
                display_safe(value)
            ),
            Notice::BadStepId { step } => format!(
                "amplihack: model tiers: step_tiers.{}: a step id is an identifier — letters, \
                 digits, `.`, `_` and `-`, starting with a letter or a digit — so this entry \
                 is ignored. {ADVICE}",
                display_safe(step)
            ),
            Notice::StepTierUnmatched { step } => format!(
                "amplihack: model tiers: step_tiers.{}: no recipe in this run declares a step \
                 with that id, so nothing is overridden and every step keeps the tier its \
                 recipe declares. {ADVICE}",
                display_safe(step)
            ),
            Notice::NotATable { key } => format!(
                "amplihack: model tiers: `{}` is not a table of tiers; it is ignored.",
                display_safe(key)
            ),
        }
    }
}
