//! Issue #1619: one typed diagnostic per thing worth saying about a tier.
//!
//! The stderr line and the recorded fall-back reason both derive from one
//! value, so they cannot disagree. Tests assert variants and fields, never
//! rendered strings.
//!
//! A notice never echoes argv, a prompt or a model the operator did not
//! configure: these lines land in run logs that get pasted into issues.

use super::claude_effort;
use super::codex_effort;
use super::table::{Effort, Tier, column_for};
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
        /// The effort the tier asked for, when the column's declared
        /// `max_effort` stepped it down to the one above.
        capped_from: Option<Effort>,
        /// Whether the model was decided somewhere this line cannot speak
        /// for — the LiteLLM gateway, which substitutes its own downstream.
        /// The line then reports the effort alone and leaves the model to
        /// the gateway's own line.
        model_elsewhere: bool,
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
    /// The operator's own `AMPLIHACK_DEFAULT_MODEL` replaced the model the
    /// tier maps to, which is what keeps amplihack usable on an account
    /// without the models the shipped mapping names.
    DefaultModel { harness: String, model: String },
    /// `AMPLIHACK_DEFAULT_MODEL` named something that could act as another
    /// argument; the step's own tier applies.
    DefaultModelRefused { reason: String },
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
    /// A tier file names an effort outside the accepted list. `tier` is the
    /// path component it was written under: one of the three tier words, or
    /// `max_effort` for a column's ceiling. The value is dropped and the rest
    /// of the file still applies.
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
    /// A `[step_tiers]` key that is a step id no recipe amplihack staged
    /// declares. The likeliest operator error in the feature, and the one
    /// with no other symptom: the run simply proceeds on the tier the recipe
    /// declares. Staging reaches this for more than a typo — a recipe it
    /// could not read, one over the size bound, and one past the candidate
    /// cap all look the same from here — so the line names the causes rather
    /// than asserting the one it has not established.
    StepTierUnmatched { step: String },
    /// A `[step_tiers]` key naming a step a recipe does declare, but not as
    /// an agent step. `model:` is a field the runner ignores on every other
    /// kind of step, so nothing is rewritten and the entry is a no-op.
    StepTierNotAnAgentStep { recipe: String, step: String },
    /// The recipe named on the command line declares a configured step, and
    /// a search directory holds a different file of the same name. The
    /// staged copy is not written: it would answer every later lookup of
    /// that name for the whole run, bodies and all.
    StagedRecipeShadowed { recipe: String },
    /// The search directories held more recipes than staging will look at,
    /// so a step declared only in one of the rest cannot be found.
    RecipeCandidatesCapped { cap: usize },
    /// A patched copy of a recipe stopped being YAML, so it was dropped and
    /// the recipe as written ran instead. A `model:` line is the only edit
    /// made, so this should not happen and is worth reporting.
    StagedRecipeDropped { recipe: String, reason: String },
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
                capped_from,
                model_elsewhere,
            } => {
                let what = match (model.as_deref(), effort) {
                    (Some(model), Some(effort)) => format!(
                        "passing `--model {model}` and `{}`",
                        effort_argument(harness, *effort)
                    ),
                    (Some(model), None) => format!("passing `--model {model}`"),
                    (None, Some(effort)) if *model_elsewhere => {
                        format!("passing `{}`", effort_argument(harness, *effort))
                    }
                    (None, Some(effort)) => format!(
                        "passing `{}`; the model stays `{harness}`'s own",
                        effort_argument(harness, *effort)
                    ),
                    // The caller does not emit this variant when the tier
                    // gave the launch neither of the two — a fall-back line
                    // has already said so. Worded for truth rather than left
                    // as a placeholder, in case a later caller does.
                    (None, None) => {
                        format!("the model and effort stay `{harness}`'s own")
                    }
                };
                let capped = match capped_from {
                    Some(requested) => format!(
                        " The tier asks for `{}`; the column's `max_effort` lowered it.",
                        requested.as_str()
                    ),
                    None => String::new(),
                };
                format!(
                    "amplihack: model tier `{}` on `{harness}`: {what}.{capped} {ADVICE}",
                    tier.as_str()
                )
            }
            Notice::EffortCallerChose { harness, tier } => format!(
                "amplihack: model tier `{}` on `{harness}`: leaving the `{}` already on the \
                 command line alone.",
                tier.as_str(),
                effort_flag(harness)
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
            Notice::DefaultModel { harness, model } => format!(
                "amplihack: AMPLIHACK_DEFAULT_MODEL is set, so `{harness}` is launched with \
                 `--model {}` in place of the model its tier maps to. Unset it to restore \
                 the mapping. {ADVICE}",
                display_safe(model)
            ),
            Notice::DefaultModelRefused { reason } => format!(
                "amplihack: AMPLIHACK_DEFAULT_MODEL is ignored: the model {reason}. The \
                 step's own tier applies."
            ),
            Notice::TableLoadFailed { path, reason } => format!(
                "amplihack: {path}: {reason}; the model tiers amplihack ships are used instead."
            ),
            Notice::UnknownTier { column, word } => {
                let (column, word) = (display_safe(column), display_safe(word));
                format!(
                    "amplihack: model tiers: `{column}.{word}`: `{word}` is not a tier; the \
                     entry is ignored. The tiers are {}.",
                    Tier::ALL.map(Tier::as_str).join(", ")
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
                 it is ignored. The accepted values are {}.",
                display_safe(column),
                display_safe(tier),
                display_safe(value),
                accepted_efforts(column)
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
                "amplihack: model tiers: step_tiers.{}: amplihack staged no recipe declaring a \
                 step with that id, so nothing is overridden and every step keeps the tier its \
                 recipe declares. Usually the id is mistyped; a recipe amplihack could not \
                 read, one larger than the size bound, and one past the candidate cap all \
                 look the same from here. {ADVICE}",
                display_safe(step)
            ),
            Notice::StepTierNotAnAgentStep { recipe, step } => format!(
                "amplihack: model tiers: step_tiers.{}: `{}` declares that step, but not as an \
                 agent step, and a model applies to agent steps only; nothing is overridden. \
                 {ADVICE}",
                display_safe(step),
                display_safe(recipe)
            ),
            Notice::StagedRecipeShadowed { recipe } => format!(
                "amplihack: model tiers: the recipe named on the command line declares a \
                 configured step, but a search directory already holds a `{}`; staging it \
                 would answer every later lookup of that name for the whole run, so it was \
                 not staged and that recipe keeps the tiers it declares. {ADVICE}",
                display_safe(recipe)
            ),
            Notice::RecipeCandidatesCapped { cap } => format!(
                "amplihack: model tiers: the recipe search path holds more recipes than the \
                 {cap} staging looks at in one run, so a step declared only in one of the \
                 rest is reported as unmatched. {ADVICE}"
            ),
            Notice::StagedRecipeDropped { recipe, reason } => format!(
                "amplihack: model tiers: {}: {reason}, so it was dropped and the recipe as \
                 written runs instead. A `model:` line is the only edit made, so please \
                 report this. {ADVICE}",
                display_safe(recipe)
            ),
            Notice::NotATable { key } => format!(
                "amplihack: model tiers: `{}` is not a table of tiers; it is ignored.",
                display_safe(key)
            ),
        }
    }
}

/// The reasoning-effort argument `harness` is given, spelled as it reaches
/// that harness: Claude Code's `--effort <level>` or Codex's
/// `-c model_reasoning_effort=<level>`.
///
/// Derived from the harness's own column, the same way the injection decides
/// which of the two to write, so the line cannot name a flag the launch did
/// not use.
fn effort_argument(harness: &str, effort: Effort) -> String {
    match column_for(harness).as_str() {
        "codex" => format!("-c {}={}", codex_effort::KEY, effort.as_str()),
        _ => format!("{} {}", claude_effort::FLAG, effort.as_str()),
    }
}

/// Just the flag, for the line that reports a caller's own. Built from the
/// same two names [`effort_argument`] writes, so the two lines of one launch
/// cannot name different flags.
fn effort_flag(harness: &str) -> String {
    match column_for(harness).as_str() {
        "codex" => format!("-c {}", codex_effort::KEY),
        _ => claude_effort::FLAG.to_string(),
    }
}

/// The reasoning efforts `column` accepts, for the line that reports one it
/// does not. Claude Code has no `minimal` and Codex has no `max`, so naming
/// the union would send an operator back to a word their harness refuses.
fn accepted_efforts(column: &str) -> String {
    Effort::accepted_in(column)
        .iter()
        .map(|effort| effort.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}
