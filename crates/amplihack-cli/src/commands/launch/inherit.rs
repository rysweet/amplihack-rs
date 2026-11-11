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
//! treated as unset. See docs/reference/effort-and-model-inheritance.md;
//! its "Design labels" section defines the D, S, C, I and F labels below.

use super::command::is_claude_compatible_tool as is_claude_compatible;
use crate::commands::agent::option_check::option_value;
use serde::Deserialize;
use std::process::Command;
use std::sync::Once;

/// The effort the parent session ran with, exported for children.
pub(crate) const PARENT_EFFORT_ENV: &str = "AMPLIHACK_PARENT_EFFORT";
/// The model the parent session ran with, exported for children.
pub(crate) const PARENT_MODEL_ENV: &str = "AMPLIHACK_PARENT_MODEL";
/// The binary for which the parent model was selected. Model ids are not portable.
pub(crate) const PARENT_MODEL_BINARY_ENV: &str = "AMPLIHACK_PARENT_MODEL_BINARY";

const EFFORT_LEVEL_ENV: &str = "CLAUDE_CODE_EFFORT_LEVEL";
const RECIPE_RUN_ID_ENV: &str = "AMPLIHACK_RECIPE_RUN_ID";
const CODEX_EFFORT_KEY: &str = "model_reasoning_effort=";
const COPILOT_EFFORT_FLAG: &str = "--reasoning-effort";

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
    /// The effort the agent runs with, to export to its children: the step's
    /// own effort, or else the inherited one when this binary takes it. Set
    /// even when nothing is inherited, so a step's own effort is passed on.
    pub(crate) exported: Option<String>,
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
/// line per dropped setting, effort first. Models from the recipe context
/// apply to the run's binary; parent-launch models require compatible binary
/// provenance. `through_gateway` is whether a LiteLLM gateway variable is set.
pub(crate) fn vet_for_binary(
    binary: &str,
    resolved: &Resolved,
    through_gateway: bool,
    parent_model_binary: Option<&str>,
) -> (Inherited, Vec<String>) {
    let mut lines = Vec::new();
    if is_claude_compatible(binary) {
        // `model_args` gives Claude-compatible steps the gateway model under
        // the LiteLLM gateway, so the inherited model reaches no step and is
        // neither passed on nor named in the run's summary line.
        let mut passed = resolved.inherited.clone();
        if resolved.model_source == Some(InheritSource::ParentLaunch)
            && !model_matches_binary(binary, parent_model_binary)
            && let Some(model) = passed.model.take()
        {
            lines.push(cannot_pass_line("model", &model, binary));
        }
        if through_gateway && let Some(model) = passed.model.take() {
            lines.push(gateway_model_line(&model, binary));
        }
        return (passed, lines);
    }
    let effort = resolved.inherited.effort.as_deref().and_then(|e| {
        if takes_effort(binary, e) {
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
        let from_same_binary = resolved.model_source == Some(InheritSource::ParentLaunch)
            && parent_model_binary == Some(binary);
        if (from_context || from_same_binary) && !through_gateway {
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
    let own = if is_claude_compatible(binary) {
        flag_value(binary, extra_args, "--effort")
    } else if binary == "codex" {
        codex_effort_from_argv(extra_args)
    } else if binary == "copilot" {
        flag_value(binary, extra_args, COPILOT_EFFORT_FLAG)
    } else {
        None
    };
    out.exported = own.clone();
    let Some(level) = inherited.effort.as_deref() else {
        return out;
    };
    if is_claude_compatible(binary) {
        // Only `claude` is known to accept `--effort`; the others read the
        // variable, and an unknown flag would fail every step.
        if own.is_none() && binary == "claude" {
            out.argv = vec!["--effort".to_string(), level.to_string()];
        }
        let applied = own.unwrap_or_else(|| level.to_string());
        out.env = vec![(EFFORT_LEVEL_ENV, applied.clone())];
        out.exported = Some(applied);
    } else if binary == "codex" {
        if own.is_none() {
            match codex_effort(level) {
                Some(config) => {
                    out.argv = vec!["-c".to_string(), config];
                    out.exported = Some(level.to_string());
                }
                None => out.unsupported = Some(cannot_pass_line("effort", level, binary)),
            }
        }
    } else if binary == "copilot" {
        if own.is_none() {
            match copilot_effort(level) {
                Some(literal) => {
                    out.argv = vec![COPILOT_EFFORT_FLAG.to_string(), literal.to_string()];
                    out.exported = Some(level.to_string());
                }
                None => out.unsupported = Some(cannot_pass_line("effort", level, binary)),
            }
        }
    } else {
        out.unsupported = Some(cannot_pass_line("effort", level, binary));
    }
    out
}

/// Whether a binary that is not Claude-compatible can take `level`.
fn takes_effort(binary: &str, level: &str) -> bool {
    match binary {
        "codex" => codex_effort(level).is_some(),
        "copilot" => copilot_effort(level).is_some(),
        _ => false,
    }
}

/// The copilot `--reasoning-effort` level for `level`, as a matched literal,
/// so only a value copilot lists in `copilot --help` reaches it (S5). Copilot
/// refuses any other value at startup, which would fail every step.
pub(crate) fn copilot_effort(level: &str) -> Option<&'static str> {
    Some(match level {
        "none" => "none",
        "minimal" => "minimal",
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "xhigh" => "xhigh",
        "max" => "max",
        _ => return None,
    })
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

/// Walk launch options without interpreting another option's consumed value as
/// a flag. The value-taking options belong to the supported agent CLIs; unknown
/// options are forwarded untouched. `--` ends option parsing. Codex short model
/// and config aliases (including attached values) use their canonical names.
pub(crate) fn launch_options<'a>(
    binary: &'a str,
    args: &'a [String],
) -> impl Iterator<Item = (&'a str, Option<&'a str>)> {
    let mut args = args.iter().map(String::as_str);
    let mut ended = false;
    std::iter::from_fn(move || {
        if ended {
            return None;
        }
        let arg = args.next()?;
        if arg == "--" {
            ended = true;
            return None;
        }
        let (mut name, mut value) = arg
            .split_once('=')
            .map_or((arg, None), |(name, value)| (name, Some(value)));
        if binary == "codex" {
            // Clap also accepts -mMODEL, -m=MODEL and -cKEY=VALUE.
            for (short, long) in [("-m", "--model"), ("-c", "--config")] {
                if let Some(rest) = arg.strip_prefix(short) {
                    name = long;
                    value = (!rest.is_empty()).then(|| rest.strip_prefix('=').unwrap_or(rest));
                    break;
                }
            }
        }
        let takes_value = name == "--model"
            || if is_claude_compatible(binary) {
                matches!(
                    name,
                    "--effort"
                        | "--system-prompt"
                        | "--system-prompt-file"
                        | "--append-system-prompt"
                        | "--append-system-prompt-file"
                        | "--agent"
                        | "--agents"
                        | "--settings"
                        | "--setting-sources"
                        | "--session-id"
                        | "--name"
                        | "-n"
                        | "--fallback-model"
                        | "--input-format"
                        | "--output-format"
                        | "--json-schema"
                        | "--max-budget-usd"
                        | "--max-turns"
                        | "--permission-mode"
                        | "--permission-prompt-tool"
                        | "--permission-prompts"
                        | "--mcp-config"
                        | "--plugin-dir"
                        | "--plugin-url"
                        | "--add-dir"
                        | "--allowedTools"
                        | "--allowed-tools"
                        | "--disallowedTools"
                        | "--disallowed-tools"
                        | "--tools"
                        | "--betas"
                        | "--file"
                        | "--environment"
                        | "--autocompact"
                        | "--system-prompt-snapshot"
                        | "--remote-control-session-name-prefix"
                )
            } else if binary == "codex" {
                matches!(
                    name,
                    "--config"
                        | "--profile"
                        | "-p"
                        | "--sandbox"
                        | "-s"
                        | "--cd"
                        | "-C"
                        | "--ask-for-approval"
                        | "-a"
                        | "--image"
                        | "-i"
                        | "--add-dir"
                        | "--local-provider"
                        | "--enable"
                        | "--disable"
                        | "--remote"
                        | "--remote-auth-token-env"
                        | "--output-last-message"
                        | "-o"
                        | "--output-schema"
                        | "--color"
                )
            } else if binary == "copilot" {
                matches!(
                    name,
                    "--reasoning-effort"
                        | "--prompt"
                        | "-p"
                        | "--system-prompt"
                        | "--additional-mcp-config"
                        | "--agent"
                        | "--config-dir"
                        | "--log-dir"
                        | "--log-level"
                        | "--output-format"
                        | "--add-dir"
                        | "--allow-tool"
                        | "--deny-tool"
                        | "--allow-url"
                        | "--deny-url"
                )
            } else {
                false
            };
        if value.is_none() && takes_value {
            value = args.next();
            // Even a missing value must not let parsing resume after `--`.
            if value == Some("--") {
                ended = true;
                value = None;
            }
        }
        Some((name, value))
    })
}

/// The last effective `flag` value. Consumed option values and everything
/// after `--` are excluded; a final flag without a value gives `None`.
pub(crate) fn flag_value(binary: &str, args: &[String], flag: &str) -> Option<String> {
    launch_options(binary, args)
        .filter(|(name, _)| *name == flag)
        .last()
        .and_then(|(_, value)| value.map(str::to_owned))
}

/// The level of the last `-c`/`--config model_reasoning_effort=L` in a codex
/// argv, stopping at `--`. Decode TOML strings and trim assignment whitespace
/// as Codex does, so children inherit the effective effort rather than quotes.
pub(crate) fn codex_effort_from_argv(args: &[String]) -> Option<String> {
    let mut out = None;
    for (_, value) in launch_options("codex", args).filter(|(name, _)| *name == "--config") {
        if let Some((key, raw)) = value.and_then(|v| v.split_once('='))
            && key.trim() == "model_reasoning_effort"
        {
            // Codex parses overrides as TOML, falling back to a bare string.
            // Decode quotes and escapes before passing the value to children.
            out = Some(
                String::deserialize(toml::de::ValueDeserializer::new(raw.trim()))
                    .unwrap_or_else(|_| raw.trim().to_string()),
            );
        }
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

/// Whether the inherited model was selected for this binary or a compatible
/// Claude binary. Explicit provenance also works outside recipe runs.
pub(crate) fn model_checked_for(binary: &str) -> bool {
    model_matches_binary(binary, parent_model_binary().as_deref())
}

/// Explicit model provenance takes precedence over the enclosing run's binary:
/// a bash step may launch a different binary with its own model. Older runs
/// carried only AMPLIHACK_AGENT_BINARY, which remains a compatibility fallback.
pub(crate) fn parent_model_binary() -> Option<String> {
    std::env::var(PARENT_MODEL_BINARY_ENV)
        .ok()
        .or_else(|| in_recipe_run().then(run_binary).flatten())
}

/// Whether a model selected for `origin` can be used by `binary`.
/// Missing provenance keeps legacy Claude inheritance working; other binaries
/// require an exact match rather than guessing from the model's spelling.
pub(crate) fn model_matches_binary(binary: &str, origin: Option<&str>) -> bool {
    if is_claude_compatible(binary) {
        origin.is_none_or(is_claude_compatible)
    } else {
        origin == Some(binary)
    }
}

/// The run's agent binary, as `recipe run` exports it in
/// `AMPLIHACK_AGENT_BINARY`. The run exports the canonical lowercase name; a
/// value that is not an allowed binary names no run binary.
fn run_binary() -> Option<String> {
    use amplihack_utils::agent_binary::{BINARY_ENV, validate_binary_name};
    std::env::var(BINARY_ENV)
        .ok()
        .and_then(|v| validate_binary_name(&v))
}

/// Whether `recipe run` has already printed this launch's "cannot be passed"
/// effort line (D7): only when this process is under a run and `binary` is
/// the run's own agent binary, the one binary the run's lines name. A launch
/// of another binary from a bash step (for example `amplihack codex` under a
/// Claude run) prints its own line, because the run never named it.
pub(crate) fn run_printed_lines_for(binary: &str) -> bool {
    run_printed_lines_for_run(binary, run_binary().as_deref(), in_recipe_run())
}

/// [`run_printed_lines_for`] with its environment passed in.
pub(crate) fn run_printed_lines_for_run(
    binary: &str,
    run_binary: Option<&str>,
    in_run: bool,
) -> bool {
    in_run && run_binary == Some(binary)
}

/// The line for a setting `binary` cannot take (D7).
pub(crate) fn cannot_pass_line(setting: &str, value: &str, binary: &str) -> String {
    format!(
        "amplihack: {setting} '{value}' cannot be passed to {binary}; steps use {binary}'s default"
    )
}

/// The line for an inherited model that a Claude-compatible `binary` does not
/// receive because the LiteLLM gateway model takes its place.
pub(crate) fn gateway_model_line(model: &str, binary: &str) -> String {
    format!(
        "amplihack: model '{model}' is not passed to {binary}; steps use the LiteLLM gateway model"
    )
}

/// The warning for an invalid value (D9). The value itself is never shown.
pub(crate) fn ignoring_line(setting: &str, source: InheritSource) -> String {
    format!(
        "amplihack: ignoring {setting} from {}: not a valid option value",
        source.label()
    )
}

/// The once-per-run summary (D8), or `None` when neither value is set.
/// `recipe run` passes the values left after [`vet_for_binary`], so a value
/// the binary cannot take is not reported as inherited.
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
/// `applied` is what `command::build_launch_command` returns: the effort from
/// [`EffortArgs::exported`], which covers binaries that take the effort only
/// through `CLAUDE_CODE_EFFORT_LEVEL`, and the model from
/// `ModelArgs::exported`, which is never amplihack's own default. Both
/// variables are removed first, so a stale value is never passed on, and a
/// variable is set only with a valid value (I1, I2, S4). Called after the
/// launch environment is applied; these variables are not managed by
/// `EnvBuilder`.
pub(crate) fn export_to_child(cmd: &mut Command, applied: &Inherited, binary: &str) {
    set_parent_env(
        cmd,
        &Inherited {
            effort: applied.effort.as_deref().and_then(checked),
            model: applied.model.as_deref().and_then(checked),
        },
        binary,
    );
}

/// Replace parent values and model provenance together, removing stale values
/// first and setting provenance only when a model is present.
pub(crate) fn set_parent_env(cmd: &mut Command, values: &Inherited, binary: &str) {
    cmd.env_remove(PARENT_EFFORT_ENV);
    cmd.env_remove(PARENT_MODEL_ENV);
    cmd.env_remove(PARENT_MODEL_BINARY_ENV);
    if let Some(effort) = &values.effort {
        cmd.env(PARENT_EFFORT_ENV, effort);
    }
    if let Some(model) = &values.model {
        cmd.env(PARENT_MODEL_ENV, model);
        cmd.env(PARENT_MODEL_BINARY_ENV, binary);
    }
}
