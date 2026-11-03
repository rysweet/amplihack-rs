//! Issue #1883: agent steps inherit the parent session's effort and model.
//!
//! `amplihack recipe run` works out the run's values once
//! ([`resolve_parent`], then [`vet_for_binary`]) and puts them in the runner's
//! environment as [`PARENT_EFFORT_ENV`] and [`PARENT_MODEL_ENV`]. Each step's
//! `amplihack <binary>` launch reads only those two variables
//! ([`inherited_from_env`]) and its own arguments, adds what [`effort_args`]
//! and `command::model_args` return, and exports the two variables again for
//! the agent it starts ([`export_to_child`]), so a nested run inherits from
//! the step that started it.
//!
//! Everything here is pure apart from [`inherited_from_env`],
//! [`in_recipe_run`], [`print_once`] and [`export_to_child`]. Nothing in this
//! path returns an error or stops a launch: a value that fails a check is
//! treated as unset. See docs/reference/effort-and-model-inheritance.md.

use super::command::is_claude_compatible_tool as is_claude_compatible;
use crate::commands::agent::option_check::option_value;
use std::process::Command;
use std::sync::Once;

/// The effort the parent session ran with, exported for children.
pub(crate) const PARENT_EFFORT_ENV: &str = "AMPLIHACK_PARENT_EFFORT";
/// The model the parent session ran with, exported for children.
pub(crate) const PARENT_MODEL_ENV: &str = "AMPLIHACK_PARENT_MODEL";

const EFFORT_LEVEL_ENV: &str = "CLAUDE_CODE_EFFORT_LEVEL";
const RECIPE_RUN_ID_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";
const CODEX_EFFORT_KEY: &str = "model_reasoning_effort=";

/// The effort and model a launch inherits. `None` means "not inherited".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Inherited {
    pub(crate) effort: Option<String>,
    pub(crate) model: Option<String>,
}

/// Where a run's value came from (D3), named in the log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InheritSource {
    /// `-c effort=` / `-c model=`, or the same keys in a context file.
    RecipeContext,
    /// `AMPLIHACK_PARENT_EFFORT` / `AMPLIHACK_PARENT_MODEL`.
    ParentLaunch,
    /// `--effort` / `--model` on the nearest `claude` ancestor process.
    ParentClaude,
    /// `CLAUDE_CODE_EFFORT_LEVEL`.
    EffortEnv,
    /// `ANTHROPIC_MODEL`.
    ModelEnv,
}

impl InheritSource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::RecipeContext => "the recipe context",
            Self::ParentLaunch => "the parent amplihack launch",
            Self::ParentClaude => "the parent claude process",
            Self::EffortEnv => EFFORT_LEVEL_ENV,
            Self::ModelEnv => "ANTHROPIC_MODEL",
        }
    }
}

/// Every source `recipe run` reads, as raw (untrimmed, unchecked) values.
#[derive(Debug, Clone, Default)]
pub(crate) struct ParentInputs<'a> {
    pub(crate) context_effort: Option<&'a str>,
    pub(crate) context_model: Option<&'a str>,
    pub(crate) parent_effort: Option<&'a str>,
    pub(crate) parent_model: Option<&'a str>,
    pub(crate) ancestor_effort: Option<&'a str>,
    pub(crate) ancestor_model: Option<&'a str>,
    /// C2: false when an older amplihack session may have put its own
    /// default model on the ancestor's argv.
    pub(crate) ancestor_model_trusted: bool,
    pub(crate) effort_env: Option<&'a str>,
    pub(crate) model_env: Option<&'a str>,
}

/// The run's values, where each came from, and one warning per invalid
/// value that was skipped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) inherited: Inherited,
    pub(crate) effort_source: Option<InheritSource>,
    pub(crate) model_source: Option<InheritSource>,
    pub(crate) warnings: Vec<String>,
}

/// What a launch adds for effort (D4, D5, D6).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EffortArgs {
    pub(crate) argv: Vec<String>,
    pub(crate) env: Vec<(&'static str, String)>,
    /// The "cannot be passed" line, when this binary cannot take the effort.
    pub(crate) unsupported: Option<String>,
}

/// `raw` trimmed, when it passes `option_value` (D9, S3, S4).
pub(crate) fn checked(raw: &str) -> Option<String> {
    let v = raw.trim();
    option_value(v).ok().map(|()| v.to_string())
}

/// The first valid value among `tiers`, highest first. Each invalid value
/// tried before it adds one warning that names the setting and the source,
/// never the value (S9).
fn first_valid(
    setting: &str,
    tiers: &[(Option<&str>, InheritSource)],
    warnings: &mut Vec<String>,
) -> (Option<String>, Option<InheritSource>) {
    for (raw, source) in tiers {
        let Some(raw) = raw else { continue };
        match checked(raw) {
            Some(v) => return (Some(v), Some(*source)),
            None => warnings.push(ignoring_line(setting, *source)),
        }
    }
    (None, None)
}

/// The run's effort and model, each from its first valid source (D3).
pub(crate) fn resolve_parent(inputs: &ParentInputs<'_>) -> Resolved {
    let mut warnings = Vec::new();
    let (effort, effort_source) = first_valid(
        "effort",
        &[
            (inputs.context_effort, InheritSource::RecipeContext),
            (inputs.parent_effort, InheritSource::ParentLaunch),
            (inputs.ancestor_effort, InheritSource::ParentClaude),
            (inputs.effort_env, InheritSource::EffortEnv),
        ],
        &mut warnings,
    );
    let ancestor_model = inputs
        .ancestor_model
        .filter(|_| inputs.ancestor_model_trusted);
    let (model, model_source) = first_valid(
        "model",
        &[
            (inputs.context_model, InheritSource::RecipeContext),
            (inputs.parent_model, InheritSource::ParentLaunch),
            (ancestor_model, InheritSource::ParentClaude),
            (inputs.model_env, InheritSource::ModelEnv),
        ],
        &mut warnings,
    );
    Resolved {
        inherited: Inherited { effort, model },
        effort_source,
        model_source,
        warnings,
    }
}

/// What `binary` may receive of `resolved` (D6), and one "cannot be passed"
/// line per dropped setting, effort first. An unknown binary is handled as
/// copilot. `through_gateway` is whether a LiteLLM gateway variable is set.
pub(crate) fn vet_for_binary(
    binary: &str,
    resolved: &Resolved,
    through_gateway: bool,
) -> (Inherited, Vec<String>) {
    let mut lines = Vec::new();
    if is_claude_compatible(binary) {
        return (resolved.inherited.clone(), lines);
    }
    let effort = resolved.inherited.effort.as_deref().and_then(|e| {
        if binary == "codex" && codex_effort(e).is_some() {
            Some(e.to_string())
        } else {
            lines.push(cannot_pass_line("effort", e, binary));
            None
        }
    });
    let model = resolved.inherited.model.as_deref().and_then(|m| {
        let from_context = resolved.model_source == Some(InheritSource::RecipeContext);
        // `model_args` gives codex and copilot no model under the gateway
        // either, so the run reports it here rather than each step dropping
        // it silently.
        if from_context && !through_gateway {
            Some(m.to_string())
        } else {
            lines.push(cannot_pass_line("model", m, binary));
            None
        }
    });
    (Inherited { effort, model }, lines)
}

/// What a launch of `binary` with the step's `extra_args` adds for the
/// inherited effort. Nothing at all when no effort is inherited (C4); the
/// step's own effort wins and is what the child's variable holds (D5).
pub(crate) fn effort_args(
    binary: &str,
    extra_args: &[String],
    inherited: &Inherited,
) -> EffortArgs {
    let mut out = EffortArgs::default();
    let Some(level) = inherited.effort.as_deref() else {
        return out;
    };
    if is_claude_compatible(binary) {
        let own = flag_value(extra_args, "--effort");
        // Only `claude` is known to accept `--effort`; the others read the
        // variable, and an unknown flag would fail every step.
        if own.is_none() && binary == "claude" {
            out.argv = vec!["--effort".to_string(), level.to_string()];
        }
        out.env = vec![(EFFORT_LEVEL_ENV, own.unwrap_or_else(|| level.to_string()))];
    } else if binary == "codex" {
        if codex_effort_from_argv(extra_args).is_none() {
            match codex_effort(level) {
                Some(config) => out.argv = vec!["-c".to_string(), config],
                None => out.unsupported = Some(cannot_pass_line("effort", level, binary)),
            }
        }
    } else {
        out.unsupported = Some(cannot_pass_line("effort", level, binary));
    }
    out
}

/// The codex `-c` value for `level`, built from a matched literal so nothing
/// the operator typed reaches codex's config parser (S5).
pub(crate) fn codex_effort(level: &str) -> Option<String> {
    let literal = match level {
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        _ => return None,
    };
    Some(format!("{CODEX_EFFORT_KEY}{literal}"))
}

/// The value of the last `flag` (`--flag v` or `--flag=v`) in `args`. Stops
/// at `--`, so prompt text is never read as a flag (S7). A final `flag` with
/// no value gives `None`.
pub(crate) fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            break;
        }
        if arg == flag {
            match args.get(i + 1).filter(|v| v.as_str() != "--") {
                Some(v) => {
                    out = Some(v.clone());
                    i += 1;
                }
                None => out = None,
            }
        } else if let Some(v) = arg.strip_prefix(flag).and_then(|r| r.strip_prefix('=')) {
            out = Some(v.to_string());
        }
        i += 1;
    }
    out
}

/// The level of the last `-c`/`--config model_reasoning_effort=L` in a codex
/// argv, stopping at `--`.
pub(crate) fn codex_effort_from_argv(args: &[String]) -> Option<String> {
    let mut out = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            break;
        }
        let value = if arg == "-c" || arg == "--config" {
            i += 1;
            args.get(i).map(String::as_str)
        } else {
            arg.strip_prefix("--config=")
        };
        if let Some(level) = value.and_then(|v| v.strip_prefix(CODEX_EFFORT_KEY)) {
            out = Some(level.to_string());
        }
        i += 1;
    }
    out
}

/// What a step's launch inherits: only the two parent variables (D1). An
/// invalid value is dropped without a warning. Under `recipe run` the run has
/// already warned about it once; when `amplihack` is started directly with a
/// bad inherited value, the drop is silent.
pub(crate) fn inherited_from_env() -> Inherited {
    let read = |name: &str| std::env::var(name).ok().as_deref().and_then(checked);
    Inherited {
        effort: read(PARENT_EFFORT_ENV),
        model: read(PARENT_MODEL_ENV),
    }
}

/// Whether this process runs under `amplihack recipe run`.
pub(crate) fn in_recipe_run() -> bool {
    std::env::var_os(RECIPE_RUN_ID_ENV).is_some_and(|v| !v.is_empty())
}

/// The line for a setting `binary` cannot take (D7).
pub(crate) fn cannot_pass_line(setting: &str, value: &str, binary: &str) -> String {
    format!(
        "amplihack: {setting} '{value}' cannot be passed to {binary}; steps use {binary}'s default"
    )
}

/// The warning for an invalid value (D9). The value itself is never shown.
pub(crate) fn ignoring_line(setting: &str, source: InheritSource) -> String {
    format!(
        "amplihack: ignoring {setting} from {}: not a valid option value",
        source.label()
    )
}

/// The once-per-run summary (D8), or `None` when neither value is set. It
/// reports the resolved values, before [`vet_for_binary`].
pub(crate) fn inherit_summary_line(resolved: &Resolved) -> Option<String> {
    let Inherited { effort, model } = &resolved.inherited;
    if effort.is_none() && model.is_none() {
        return None;
    }
    let source = match (resolved.effort_source, resolved.model_source) {
        (Some(e), Some(m)) if e == m => e.label().to_string(),
        (Some(e), Some(m)) => format!("{} and {}", e.label(), m.label()),
        (Some(s), None) | (None, Some(s)) => s.label().to_string(),
        (None, None) => return None,
    };
    Some(format!(
        "amplihack: steps inherit effort={} model={} (from {source})",
        effort.as_deref().unwrap_or("unset"),
        model.as_deref().unwrap_or("unset"),
    ))
}

/// Print `line` to stderr at most once per process (D7, outside a run).
pub(crate) fn print_once(line: &str) {
    static PRINTED: Once = Once::new();
    PRINTED.call_once(|| eprintln!("{line}"));
}

/// C1: export the launched agent's effort and model for its children.
///
/// The effort is read from the agent's final argv (`--effort`, or codex's
/// `-c model_reasoning_effort=`); the model is `model`, which
/// `command::model_args` sets only for a model the launch was given or
/// inherited, never for amplihack's own default. Both variables are removed
/// first, so a stale value is never passed on, and a variable is set only
/// with a valid value (I1, I2, S4). Called after the launch environment is
/// applied; these variables are not managed by `EnvBuilder`.
pub(crate) fn export_to_child(cmd: &mut Command, model: Option<&str>) {
    let argv: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let effort = flag_value(&argv, "--effort")
        .or_else(|| codex_effort_from_argv(&argv))
        .as_deref()
        .and_then(checked);
    set_parent_env(
        cmd,
        &Inherited {
            effort,
            model: model.and_then(checked),
        },
    );
}

/// Replace both parent variables on `cmd` with `values`: each is removed
/// first, so a stale value is never passed on, and set only when present.
pub(crate) fn set_parent_env(cmd: &mut Command, values: &Inherited) {
    cmd.env_remove(PARENT_EFFORT_ENV);
    cmd.env_remove(PARENT_MODEL_ENV);
    if let Some(effort) = &values.effort {
        cmd.env(PARENT_EFFORT_ENV, effort);
    }
    if let Some(model) = &values.model {
        cmd.env(PARENT_MODEL_ENV, model);
    }
}
