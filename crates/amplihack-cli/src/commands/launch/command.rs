//! Command building for tool binaries: argument injection, UVX plugin
//! handling, Docker launcher args, and Claude-specific env augmentation.

use super::system_prompt_append;
use crate::binary_finder::BinaryInfo;
use crate::commands::uvx_help::is_uvx_deployment;
use crate::env_builder::EnvBuilder;

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) const COPILOT_HOME_ENV: &str = "COPILOT_HOME";

/// Reject repository custom agents before a routed Copilot process starts.
///
/// `COPILOT_HOME` isolates user configuration, but Copilot independently
/// discovers `.github/agents` from the workspace. Copilot has no supported
/// switch that disables that discovery, and an agent may select its own model.
pub(crate) fn validate_routed_copilot_workspace(
    execution_dir: &Path,
    routed_copilot: bool,
) -> Result<()> {
    if !routed_copilot {
        return Ok(());
    }

    let start = execution_dir.canonicalize().with_context(|| {
        format!(
            "failed to resolve routed Copilot workspace {}",
            execution_dir.display()
        )
    })?;
    for directory in start.ancestors() {
        let agents = directory.join(".github").join("agents");
        match std::fs::symlink_metadata(&agents) {
            Ok(_) => bail!(
                "routed Copilot cannot launch from a workspace containing {}; repository custom agents can override the configured model",
                agents.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to inspect routed Copilot custom-agent path {}",
                        agents.display()
                    )
                });
            }
        }

        if std::fs::symlink_metadata(directory.join(".git")).is_ok() {
            break;
        }
    }
    Ok(())
}

/// Give a routed Copilot process a fresh configuration root for its lifetime.
///
/// Copilot discovers installed plugins without an argv flag, so rejecting
/// `--plugin-dir` is not sufficient. Keeping the returned guard alive prevents
/// user-scoped plugin, hook, agent, and MCP configuration from entering the
/// routed session and ensures the isolated state is removed after exit.
/// Repository configuration is a separate scope enforced by
/// [`validate_routed_copilot_workspace`].
pub(super) fn isolate_routed_copilot_home(
    command: &mut Command,
    routed_copilot: bool,
) -> std::io::Result<Option<tempfile::TempDir>> {
    if !routed_copilot {
        return Ok(None);
    }

    let home = tempfile::Builder::new()
        .prefix("amplihack-routed-copilot-")
        .tempdir()?;
    command.env(COPILOT_HOME_ENV, home.path());
    Ok(Some(home))
}

#[cfg(test)]
pub(super) fn build_command(
    binary: &BinaryInfo,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    extra_args: &[String],
) -> Command {
    build_command_for_dir(
        binary,
        resume,
        continue_session,
        skip_permissions,
        extra_args,
        None,
        false, // subprocess_safe
    )
}

pub(super) fn build_command_for_dir(
    binary: &BinaryInfo,
    resume: bool,
    continue_session: bool,
    skip_permissions: bool,
    extra_args: &[String],
    add_dir_override: Option<&Path>,
    subprocess_safe: bool,
) -> Command {
    let mut cmd = Command::new(&binary.path);

    // SEC-2: Only inject --dangerously-skip-permissions when the caller has
    // explicitly opted in via `--skip-permissions`.  This flag bypasses
    // Claude's interactive confirmation prompts and must not be on by default.
    // Only inject for Claude-compatible tools — Copilot and Codex don't support it.
    let is_claude_compatible = is_claude_compatible_tool(&binary.name);
    if skip_permissions && is_claude_compatible {
        cmd.arg("--dangerously-skip-permissions");
    }

    inject_uvx_plugin_args(&mut cmd, &binary.name, extra_args, add_dir_override);

    // Issues #1421, #1527: every decision about `--model`, and every stderr
    // line about it, is made in `model_args`, which the tests call directly
    // with the environment set. Nothing here may add a condition of its own:
    // print what it returns, add what it returns, and nothing else.
    let model = model_args(&binary.name, extra_args);
    for line in &model.stderr {
        eprintln!("{line}");
    }
    cmd.args(&model.argv);

    if resume {
        cmd.arg("--resume");
    }
    if continue_session {
        cmd.arg("--continue");
    }

    // Inject --allow-all for Copilot by default (issue #303). Copilot's
    // `--allow-all` is shorthand for `--allow-all-tools + --allow-all-paths +
    // --allow-all-urls`. Without it, Copilot prompts for tool/path/url
    // permission on its first action, which blocks unattended orchestrator
    // loops launched by amplihack. Skip injection if the user already set any
    // allow-all-* flag, or if AMPLIHACK_COPILOT_NO_ALLOW_ALL=1.
    if binary.name == "copilot" && should_inject_copilot_allow_all(extra_args) {
        cmd.arg("--allow-all");
    }

    // Issue #621: Subprocess-safe defense-in-depth — granular allow-all flags.
    //
    // When subprocess_safe is active, we ALSO inject the granular
    // `--allow-all-tools` and `--allow-all-paths` flags. This is intentional
    // layering on top of the broader `--allow-all` (issue #303): both
    // appearing in argv is accepted by the copilot CLI without conflict, and
    // the granular flags are the explicit contract callers (Simard engineers,
    // recipe-runner agents) audit for.
    //
    // The `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` opt-out (originally introduced
    // for the broader #303 flag) ALSO suppresses these granular injections —
    // a user who has explicitly disabled auto-permission grants for copilot
    // wants permission gates back across the board, not partially restored.
    //
    // Scope: copilot binary only. Other tools (claude, codex, amplifier)
    // are excluded — these flags are copilot-specific.
    if binary.name == "copilot" && subprocess_safe {
        let (inject_tools, inject_paths) = should_inject_subprocess_safe_flags(extra_args);
        if inject_tools {
            cmd.arg("--allow-all-tools");
        }
        if inject_paths {
            cmd.arg("--allow-all-paths");
        }
    }

    if binary.name == "claude" && amplihack_utils::litellm_proxy::proxy_requested() {
        cmd.arg("--setting-sources");
        cmd.arg("");
        cmd.arg("--safe-mode");
    }

    // Inject --remote for Copilot by default. Remote mode offloads compute to
    // GitHub's cloud, which is the preferred mode for amplihack orchestration.
    // Skip injection if the user already passed --remote, or if
    // AMPLIHACK_COPILOT_NO_REMOTE=1.
    if binary.name == "copilot" && should_inject_copilot_remote(extra_args) {
        cmd.arg("--remote");
    }

    // Issue #1265 Option 3: deliver amplihack's routing contract on a channel
    // the base system prompt cannot outrank. The hook and CLAUDE.md are content
    // the agent reads; the system prompt is the frame it reads them in, so a
    // contrary line there silently wins and amplihack's router is ignored with
    // no error and no warning. See docs/SYSTEM_PROMPT_APPEND.md.
    //
    // Emits the fragment's CONTENTS: `--append-system-prompt` takes a prompt
    // string. Injected here, before `cmd.args(extra_args)`, so the user's own
    // arguments stay last as with every other injection.
    // The fragment is compiled in, so the only question is whether this binary
    // and this argv want it: `amplihack copilot` and `amplihack codex` do not
    // support the flag and the gate answers no for them.
    let opt_out = std::env::var(system_prompt_append::OPT_OUT_ENV).ok();
    if system_prompt_append::should_inject_system_prompt_append(
        &binary.name,
        extra_args,
        opt_out.as_deref(),
    ) && let Some(fragment) = system_prompt_append::installed_fragment()
    {
        cmd.arg("--append-system-prompt");
        cmd.arg(fragment);
    }

    cmd.args(extra_args);

    // These routed-Copilot restrictions deliberately come last. Copilot has
    // enabling counterparts for remote control and export, so user arguments
    // must not override the gateway isolation contract. It has no enabling
    // counterpart for `--no-auto-update`.
    if binary.name == "copilot" && amplihack_utils::litellm_proxy::proxy_requested() {
        cmd.arg("--no-remote");
        cmd.arg("--no-remote-export");
        cmd.arg("--no-auto-update");
        cmd.arg("--secret-env-vars=COPILOT_PROVIDER_API_KEY");
    }
    cmd
}

/// The tools amplihack treats as Claude Code: they get
/// `--dangerously-skip-permissions`, a `--model` of amplihack's choosing, and
/// the dotted `--model` warning. `amplihack copilot` and `amplihack codex` get
/// none of those.
pub(super) fn is_claude_compatible_tool(binary_name: &str) -> bool {
    matches!(binary_name, "claude" | "rusty" | "rustyclawd" | "amplifier")
}

/// What amplihack adds to one launch for `--model`, and what it prints about
/// it. See [`model_args`].
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ModelArgs {
    /// `["--model", <id>]`, or empty when amplihack adds no model.
    pub(super) argv: Vec<String>,
    /// Lines for stderr, in order: the dotted explicit `--model` warning
    /// (#1527) and the line naming a model amplihack chose (#1421).
    pub(super) stderr: Vec<String>,
}

/// Every decision amplihack makes about `--model` for one launch of
/// `binary_name` with the operator's `extra_args`: the arguments to add and the
/// stderr lines to print. `build_command_for_dir` adds and prints exactly what
/// this returns and decides nothing about the model itself, so the tests on
/// this function cover what an operator sees.
///
/// Reads the LiteLLM gateway variables, `AMPLIHACK_LITELLM_MODEL` and
/// `AMPLIHACK_DEFAULT_MODEL` from the environment.
///
/// Issue #1421: amplihack requests a CONCRETE model id, never an alias. It used
/// to force `--model opus[1m]`. An alias is resolved by the CLI, so its meaning
/// depends on the CLI version: on a reporter's install it resolved to the
/// retired `claude-opus-4-1-20250805` and every agent step 404'd naming an id
/// the user had never chosen and could not find anywhere, because it existed
/// only at resolution time. [`DEFAULT_MODEL`] is concrete for that reason.
///
/// Only a Claude-compatible tool ([`is_claude_compatible_tool`]) gets a model
/// from amplihack. Precedence, highest first:
///
/// 1. `--model` on the command line: the operator's, forwarded untouched, even
///    when dotted (#1527 asks that an explicit value be left alone). A dotted
///    Claude id gets one stderr warning naming the hyphenated spelling; see
///    [`explicit_model_warnings`] for when.
/// 2. `AMPLIHACK_LITELLM_MODEL`, when the launch goes through the LiteLLM
///    gateway, which is when
///    [`proxy_requested`](amplihack_utils::litellm_proxy::proxy_requested) is
///    true: `AMPLIHACK_LITELLM_ENDPOINT`, `AMPLIHACK_LITELLM_API_KEY` or
///    `AMPLIHACK_LITELLM_MODEL` is set. No other `AMPLIHACK_LITELLM_*` variable
///    selects this path. The gateway routes on the model name and has no
///    default of its own.
/// 3. `AMPLIHACK_DEFAULT_MODEL`, which pins every launch; an empty value means
///    "pass nothing and let the CLI decide". A dotted Claude id (Copilot's
///    spelling) is rewritten to the hyphenated id (#1527).
/// 4. [`DEFAULT_MODEL`].
///
/// This still outranks a `"model"` set in `~/.claude/settings.json`, which is
/// why that lever appeared to do nothing in the #1421 report. That is a
/// deliberate, documented precedence, and the stderr line names both the value
/// and where it came from, so the id in any later error traces straight back
/// to this decision instead of looking like a secret inside the binary.
pub(super) fn model_args(binary_name: &str, extra_args: &[String]) -> ModelArgs {
    let is_claude_compatible = is_claude_compatible_tool(binary_name);
    let through_gateway = is_claude_compatible && amplihack_utils::litellm_proxy::proxy_requested();
    let mut out = ModelArgs {
        argv: Vec::new(),
        stderr: explicit_model_warnings(binary_name, extra_args, through_gateway),
    };

    let user_has_model = extra_args
        .iter()
        .any(|arg| arg == "--model" || arg.starts_with("--model="));
    if user_has_model || !is_claude_compatible {
        return out;
    }
    let selection = if through_gateway {
        // `ProxyConfig::from_env()` refuses a gateway launch without
        // `AMPLIHACK_LITELLM_MODEL` before the command is built, so the
        // fallback is never reached by a real launch.
        Some(ModelSelection {
            model: std::env::var(amplihack_utils::litellm_proxy::MODEL_ENV)
                .unwrap_or_else(|_| "amplihack-default".to_string()),
            source: ModelSource::LiteLlmGateway,
            normalised_from: None,
        })
    } else {
        configured_default_model()
    };
    if let Some(selection) = selection {
        out.stderr
            .push(model_selection_notice(&selection, binary_name));
        out.argv = vec!["--model".to_string(), selection.model];
    }
    out
}

/// The model amplihack requests when the operator has not chosen one.
///
/// Issue #1421: this is a CONCRETE model id, deliberately not an alias.
///
/// The bug was `opus[1m]`. An alias is resolved by the CLI, not by amplihack,
/// so what it means depends on the CLI version installed — and on a reporter's
/// machine `opus[1m]` resolved to the retired `claude-opus-4-1-20250805`. Every
/// agent step then 404'd naming a model id the user had never chosen and could
/// not find in any config or in the binary, because it was never written down
/// anywhere: it was invented at resolution time.
///
/// A concrete id cannot drift that way. Two CLI versions given
/// `claude-opus-5[1m]` either both honour it or one fails naming the exact
/// string amplihack asked for — which is a searchable, diagnosable failure
/// rather than a phantom. The `[1m]` suffix keeps the 1M context window that
/// `opus[1m]` provided, so this is not a downgrade.
///
/// It will eventually need updating, and that is accepted: the failure mode of
/// a stale concrete id is a 404 that names itself. The failure mode of a stale
/// alias is a 404 naming something the user cannot trace. Prefer the loud one.
pub(crate) const DEFAULT_MODEL: &str = "claude-opus-5[1m]";

/// A model amplihack is about to put on the command line, and where it came
/// from, for the #1421 stderr line.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ModelSelection {
    pub(crate) model: String,
    pub(crate) source: ModelSource,
    /// The operator's spelling, when it was rewritten into `model` (#1527).
    pub(crate) normalised_from: Option<String>,
}

/// Where a [`ModelSelection`] came from. It decides both the source named in
/// the stderr line and the variable that line tells the operator to change, so
/// the two cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelSource {
    /// `AMPLIHACK_LITELLM_MODEL`, on a launch routed through the LiteLLM
    /// gateway. `AMPLIHACK_DEFAULT_MODEL` is not read on that path.
    LiteLlmGateway,
    /// `AMPLIHACK_DEFAULT_MODEL`, set to a non-blank value.
    DefaultModelEnv,
    /// [`DEFAULT_MODEL`], because `AMPLIHACK_DEFAULT_MODEL` is unset or is not
    /// valid UTF-8.
    BuiltInDefault,
}

impl ModelSource {
    /// How the stderr line names this source.
    fn label(self) -> &'static str {
        match self {
            Self::LiteLlmGateway => amplihack_utils::litellm_proxy::MODEL_ENV,
            Self::DefaultModelEnv => "AMPLIHACK_DEFAULT_MODEL",
            Self::BuiltInDefault => "amplihack's built-in default",
        }
    }
}

/// The model to request, or `None` to let the CLI choose.
///
/// `AMPLIHACK_DEFAULT_MODEL` overrides [`DEFAULT_MODEL`]. Setting it to an
/// empty or whitespace-only value — which is how a shell delivers an unset-ish
/// value — means "pass no `--model` at all", so an operator can hand the choice
/// back to the CLI without editing amplihack.
///
/// Issue #1527: operators set this to a Claude id in the dotted spelling GitHub
/// Copilot CLI uses (`claude-opus-5.5`). Only amplihack reads the variable, and
/// only for claude-compatible launches, and Claude model ids use hyphens. So a
/// dotted Claude id is rewritten to the hyphenated one and the selection
/// records the original. See [`normalize_dotted_claude_model_id`].
///
/// `ANTHROPIC_BASE_URL` is deliberately not read here, nor by
/// [`explicit_model_warnings`]. The usual reason to set it is a gateway in
/// front of Anthropic, which wants Anthropic's hyphenated ids. And amplihack
/// cannot see every place it is set: Claude Code also takes it from the `env`
/// block of `~/.claude/settings.json`. An operator whose endpoint serves the
/// dotted spelling passes an explicit `--model`, which is forwarded as typed.
/// Only the LiteLLM gateway is exempt: amplihack configures it, and
/// [`model_args`] does not call this function on that path.
///
/// The variable is read once, and the source is decided in the same match arm,
/// so the stderr line cannot name a source that disagrees with the value. A
/// value that is not valid UTF-8 falls back to [`DEFAULT_MODEL`] rather than
/// being forwarded lossily.
pub(crate) fn configured_default_model() -> Option<ModelSelection> {
    match std::env::var("AMPLIHACK_DEFAULT_MODEL") {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (model, normalised_from) = match normalize_dotted_claude_model_id(trimmed) {
                Some(hyphenated) => (hyphenated, Some(trimmed.to_string())),
                None => (trimmed.to_string(), None),
            };
            Some(ModelSelection {
                model,
                source: ModelSource::DefaultModelEnv,
                normalised_from,
            })
        }
        Err(_) => Some(ModelSelection {
            model: DEFAULT_MODEL.to_string(),
            source: ModelSource::BuiltInDefault,
            normalised_from: None,
        }),
    }
}

/// Issue #1527: the hyphenated spelling of a dotted Claude model id, or `None`
/// when `id` is not one.
///
/// Rewrites exactly `claude-<family>-<major>.<minor><suffix>`, where `family`
/// is one or more ASCII lowercase letters, `major` and `minor` are one or more
/// ASCII digits, and `suffix` is empty or starts with `[` or `-`. The one `.`
/// between `major` and `minor` becomes `-`; everything else, the suffix
/// included, is copied as it is. So `claude-opus-5.5[1m]` becomes
/// `claude-opus-5-5[1m]`.
///
/// Everything else is `None`: aliases, ids that are already hyphenated, non-
/// Claude ids such as `gpt-5.1`, legacy version-first ids such as
/// `claude-3.5-sonnet`, and anything not lowercase. There is no fuzzy matching,
/// so the output can never name a different model than the input did.
///
/// `id` is expected to be trimmed already.
pub(super) fn normalize_dotted_claude_model_id(id: &str) -> Option<String> {
    let rest = id.strip_prefix("claude-")?;
    let (family, rest) = split_leading(rest, u8::is_ascii_lowercase)?;
    let rest = rest.strip_prefix('-')?;
    let (major, rest) = split_leading(rest, u8::is_ascii_digit)?;
    let rest = rest.strip_prefix('.')?;
    let (minor, suffix) = split_leading(rest, u8::is_ascii_digit)?;
    if !(suffix.is_empty() || suffix.starts_with('[') || suffix.starts_with('-')) {
        return None;
    }
    Some(format!("claude-{family}-{major}-{minor}{suffix}"))
}

/// Split `s` after its leading run of bytes matching `pred`, or `None` when
/// that run is empty. `pred` only ever matches ASCII, so the split always
/// lands on a char boundary.
fn split_leading(s: &str, pred: fn(&u8) -> bool) -> Option<(&str, &str)> {
    let n = s.bytes().take_while(pred).count();
    (n > 0).then(|| s.split_at(n))
}

/// The one stderr line naming the model amplihack passes and where it came
/// from (issue #1421), plus the original spelling when it was rewritten
/// (issue #1527).
///
/// The advice at the end names the variable that controls this source. On the
/// LiteLLM gateway path that is `AMPLIHACK_LITELLM_MODEL`: telling the operator
/// to set `AMPLIHACK_DEFAULT_MODEL` there would send them to a variable that is
/// not read, and an empty gateway model is refused rather than meaning "let the
/// tool choose".
///
/// For the other two sources, and without a rewrite, the line is exactly what
/// it was before #1527, so anything that reads stderr sees no change.
pub(super) fn model_selection_notice(selection: &ModelSelection, binary_name: &str) -> String {
    let ModelSelection {
        model,
        source,
        normalised_from,
    } = selection;
    let label = source.label();
    let provenance = match normalised_from {
        Some(original) => {
            format!("{label}, normalised from `{original}`: Claude model ids use hyphens, not dots")
        }
        None => label.to_string(),
    };
    let advice = match source {
        ModelSource::LiteLlmGateway => format!(
            "Set {} to change it. AMPLIHACK_DEFAULT_MODEL is not read while a LiteLLM \
             gateway variable is set.",
            amplihack_utils::litellm_proxy::MODEL_ENV
        ),
        ModelSource::DefaultModelEnv | ModelSource::BuiltInDefault => format!(
            "Set AMPLIHACK_DEFAULT_MODEL to override it, or to an empty value to \
             let {binary_name} choose its own default model."
        ),
    };
    format!("amplihack: passing `--model {model}` to `{binary_name}` (from {provenance}). {advice}")
}

/// Issue #1527: the stderr warnings for an explicit `--model` on the command
/// line. One line for each value that is a dotted Claude id, naming the
/// hyphenated spelling; empty when there is none. [`model_args`] passes these
/// through unfiltered, so every condition on the warning is here:
///
/// - Only for a Claude-compatible tool ([`is_claude_compatible_tool`]). The
///   dotted spelling is GitHub Copilot CLI's own, so telling an
///   `amplihack copilot` user to change `claude-opus-4.5` would be wrong.
/// - Not when `through_gateway` is true. The LiteLLM gateway routes on the
///   exact model name, and a dot in a route name may be correct.
///
/// The value is still forwarded as typed: the issue asks that an explicit
/// `--model` not be rewritten. The warning exists because the launched tool may
/// not report the problem itself; issue #1527 records what one Claude Code
/// version did. The line says only what amplihack can vouch for, that Claude
/// model ids use hyphens, and names no other tool's behaviour or error text:
/// those change with the tool's version, and the line would then be wrong on
/// every launch.
///
/// Values are found the way [`model_args`] detects an explicit model: a
/// `--model` argument followed by its value, or `--model=<value>`.
pub(super) fn explicit_model_warnings(
    binary_name: &str,
    extra_args: &[String],
    through_gateway: bool,
) -> Vec<String> {
    if !is_claude_compatible_tool(binary_name) || through_gateway {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    let mut args = extra_args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        let value = if arg == "--model" {
            args.next()
        } else {
            arg.strip_prefix("--model=")
        };
        let Some(value) = value else { continue };
        if let Some(hyphenated) = normalize_dotted_claude_model_id(value.trim()) {
            warnings.push(format!(
                "amplihack: warning: passing `--model {value}` to `{binary_name}` as typed, \
                 but Claude model ids use hyphens, not dots. Use `--model {hyphenated}`."
            ));
        }
    }
    warnings
}

/// Pure decision function (issue #621): is this Copilot invocation
/// subprocess-safe?
///
/// Subprocess-safe is true when ANY of:
///   * the user explicitly passed `--subprocess-safe` (`explicit_flag`)
///   * `AMPLIHACK_NONINTERACTIVE=1` is set (`env_amplihack_noninteractive`)
///   * any of stdin/stdout/stderr is not a TTY (`any_stream_non_tty`)
///
/// `AMPLIHACK_AGENT_BINARY` is deliberately not an input (issue #1525). It
/// names which agent CLI to run and says nothing about whether anyone is at
/// the terminal. While it counted, anyone who exported it to choose a CLI had
/// every interactive `amplihack copilot` launched with reflection, launcher
/// staging and the power-steering prompt turned off. Delegated runs do not need
/// it: `amplihack recipe run` sets `AMPLIHACK_NONINTERACTIVE=1` for the runner,
/// and every step runs with piped stdio. The parameter was removed rather than
/// ignored so that the caller cannot quietly start passing it in again.
///
/// All inputs are passed as parameters (rather than read from env / stdio
/// inside) so this function is unit-testable deterministically without
/// depending on the test harness's actual env or stdio state. The
/// production caller in `commands/mod.rs` reads the three signals once and
/// hands them to this resolver — see also [`crate::util::any_stream_is_non_tty`].
pub(crate) fn resolve_subprocess_safe(
    explicit_flag: bool,
    env_amplihack_noninteractive: bool,
    any_stream_non_tty: bool,
) -> bool {
    if explicit_flag {
        return true;
    }
    if env_amplihack_noninteractive {
        return true;
    }
    any_stream_non_tty
}

/// Pure decision function (issue #621): which subprocess-safe granular flags
/// should `build_command_for_dir` inject for Copilot?
///
/// Returns `(inject_allow_all_tools, inject_allow_all_paths)`. Callers must
/// have already gated on the surrounding `subprocess_safe == true` and
/// `binary.name == "copilot"` conditions in `build_command_for_dir`.
///
/// Suppression rules (each flag computed independently):
///   * `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` env var → suppress BOTH
///     (the user opted out of amplihack auto-granting permissions to copilot;
///     subprocess-safe defers to that explicit opt-out).
///   * `--allow-all` (broader superset) in user args → suppress BOTH.
///   * `--allow-all-tools` already in user args → suppress tools injection.
///   * `--allow-all-paths` already in user args → suppress paths injection.
pub(super) fn should_inject_subprocess_safe_flags(user_args: &[String]) -> (bool, bool) {
    if std::env::var("AMPLIHACK_COPILOT_NO_ALLOW_ALL").as_deref() == Ok("1") {
        return (false, false);
    }
    let user_has_superset = user_args.iter().any(|a| a == "--allow-all");
    if user_has_superset {
        return (false, false);
    }
    let user_has_tools = user_args.iter().any(|a| a == "--allow-all-tools");
    let user_has_paths = user_args.iter().any(|a| a == "--allow-all-paths");
    (!user_has_tools, !user_has_paths)
}

/// Pure precedence resolver (issue #621): compute the *effective*
/// no-reflection decision from the three input signals.
///
/// Precedence (highest → lowest):
///   1. `explicit_reflection = true` (user passed `--reflection`)  → reflection ON  → return false
///   2. `no_reflection = true`       (user passed `--no-reflection`)→ reflection OFF → return true
///   3. `subprocess_safe = true`     (auto-detected or `--subprocess-safe`) → reflection OFF → return true
///   4. otherwise (default)                                          → reflection ON  → return false
///
/// `--reflection` and `--no-reflection` are mutually exclusive at the clap
/// layer (`conflicts_with`); the resolver itself is defense-in-depth and
/// gives `--reflection` priority if both somehow arrive as `true`.
pub(crate) fn resolve_no_reflection(
    explicit_reflection: bool,
    no_reflection: bool,
    subprocess_safe: bool,
) -> bool {
    if explicit_reflection {
        return false;
    }
    if no_reflection {
        return true;
    }
    subprocess_safe
}

/// Decide whether `amplihack` should inject `--allow-all` into a Copilot
/// invocation. Returns false if the user already supplied any allow-all-*
/// flag, or if the `AMPLIHACK_COPILOT_NO_ALLOW_ALL=1` env var is set.
pub(crate) fn should_inject_copilot_allow_all(extra_args: &[String]) -> bool {
    if std::env::var("AMPLIHACK_COPILOT_NO_ALLOW_ALL").as_deref() == Ok("1") {
        return false;
    }
    let already_present = extra_args.iter().any(|a| {
        a == "--allow-all"
            || a == "--allow-all-tools"
            || a == "--allow-all-paths"
            || a == "--allow-all-urls"
    });
    !already_present
}

/// Decide whether `amplihack` should inject `--remote` into a Copilot
/// invocation. Returns false if the user already passed `--remote` or
/// `--no-remote`, or if `AMPLIHACK_COPILOT_NO_REMOTE=1` is set.
pub(crate) fn should_inject_copilot_remote(extra_args: &[String]) -> bool {
    if std::env::var("AMPLIHACK_COPILOT_NO_REMOTE").as_deref() == Ok("1") {
        return false;
    }
    if amplihack_utils::litellm_proxy::proxy_requested() {
        return false;
    }
    !extra_args
        .iter()
        .any(|a| a == "--remote" || a == "--no-remote")
}

fn inject_uvx_plugin_args(
    cmd: &mut Command,
    tool: &str,
    extra_args: &[String],
    add_dir_override: Option<&Path>,
) {
    if tool != "claude" || amplihack_utils::litellm_proxy::proxy_requested() || !is_uvx_deployment()
    {
        return;
    }

    if !extra_args.iter().any(|arg| arg == "--plugin-dir")
        && let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
    {
        cmd.arg("--plugin-dir")
            .arg(home.join(".amplihack").join(".claude"));
    }

    if !extra_args.iter().any(|arg| arg == "--add-dir")
        && let Some(original_cwd) = resolve_uvx_add_dir(add_dir_override)
    {
        cmd.arg("--add-dir").arg(original_cwd);
    }
}

fn resolve_uvx_add_dir(add_dir_override: Option<&Path>) -> Option<PathBuf> {
    if std::env::var_os("AMPLIHACK_IS_STAGED").as_deref() == Some(std::ffi::OsStr::new("1"))
        && let Some(original_cwd) = std::env::var_os("AMPLIHACK_ORIGINAL_CWD").map(PathBuf::from)
    {
        return Some(original_cwd);
    }
    add_dir_override
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("AMPLIHACK_ORIGINAL_CWD").map(PathBuf::from))
        .or_else(|| std::env::current_dir().ok())
}

/// May `dir` be moved to the front of the child's `PATH`?
///
/// Yes if the child could already have reached it — it is on `PATH` — or if it
/// is amplihack's own npm prefix, which amplihack installs into and owns, and
/// which is routinely absent from a shell `PATH` captured before the first
/// install (persistent tmux and ssh sessions, minimal Docker shells).
///
/// Anything else is a directory the session could not otherwise see, and
/// promoting it would widen an override's reach from one binary to every
/// binary. See the comment at the call site.
pub(super) fn is_already_reachable(dir: &Path, home: &Path) -> bool {
    // C1 — the spelling of amplihack's own prefix has ONE owner. This function
    // used to hardcode `home.join(".npm-global").join("bin")` to identify the
    // very directory `launch_target` already classifies authoritatively as
    // `TargetSource::AmplihackPrefix`. Move the prefix and nothing failed to
    // compile; `claude` just quietly stopped being reachable in the child.
    if dir == amplihack_utils::launch_target::amplihack_prefix_bin(home) {
        return true;
    }
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|entry| entry == dir))
        .unwrap_or(false)
}

/// Build the child's environment for a claude launch.
///
/// `resolved` is the path of the binary
/// [`amplihack_utils::launch_target::resolve`] actually selected, or `None`
/// when nothing healthy was found.
///
/// # Defect 4 (issue #1266) — child-PATH poisoning
///
/// This function used to prepend `~/.npm-global/bin` unconditionally. That is
/// an amplihack-writable directory placed *ahead of the system directories*
/// for the child and for every subagent and shell-out inside that session — so
/// even after amplihack picks a healthy `/usr/bin/claude` by absolute path, a
/// bare `claude` inside the session re-resolves to whatever is in the npm
/// prefix. On the repo owner's WSL machine `~/.npm-global/bin` is already the
/// first PATH entry, which makes the same stub shadow `claude` system-wide.
///
/// The contract now: prepend the directory of the **resolved** target, and
/// prepend the npm-global bin only when that is where the resolved target
/// actually lives. Nothing resolved ⇒ nothing prepended.
pub(super) fn augment_claude_launch_env(
    env_builder: EnvBuilder,
    tool: &str,
    resolved: Option<&Path>,
) -> EnvBuilder {
    if tool != "claude" {
        return env_builder;
    }

    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return env_builder;
    };

    // Prepend the directory of the binary amplihack actually resolved, and
    // nothing else. When that directory happens to be `~/.npm-global/bin` this
    // reproduces the old behaviour exactly — which is the one case the
    // unconditional prepend got right. When nothing healthy resolved there is
    // no directory to prefer, and prepending the prefix that holds the
    // placeholder would be the worst available guess.
    //
    // ...and only when that directory is already reachable. Prepending moves an
    // entry to the front; it must not ADD one. `CLAUDE_BINARY_PATH=/tmp/x/claude`
    // already grants control of the binary amplihack execs — that is what the
    // variable is for — but without this check it would also put `/tmp/x` ahead
    // of `/usr/bin` for the child *and every subagent and shell-out in that
    // session*, so `git`, `node` and `sh` would resolve from there too. Setting
    // one binary is not consent to redirect all of them.
    //
    // The absoluteness filter is defence in depth. `launch_target`'s
    // `cheap_reject` now rejects every relative candidate at the resolution
    // funnel, so `resolved` should already be absolute and this filter should
    // never fire — it asserts the invariant rather than establishing it. It
    // stays because `resolved` is a bare `&Path` from a caller this module does
    // not control, and the cost of being wrong is a leading colon: the current
    // directory at the FRONT of the child's `$PATH`, for the agent, every
    // subagent and every shell-out.
    let env_builder = match resolved
        .and_then(Path::parent)
        .filter(|dir| dir.is_absolute())
        .filter(|dir| is_already_reachable(dir, &home))
    {
        Some(dir) => env_builder.prepend_path(dir.to_path_buf()),
        None => env_builder,
    };
    if std::env::var("AMPLIHACK_PLUGIN_INSTALLED").as_deref() == Ok("true") {
        return env_builder.set(
            "CLAUDE_PLUGIN_ROOT",
            home.join(".claude")
                .join("plugins")
                .join("cache")
                .join("amplihack")
                .join("amplihack")
                .join("0.9.0")
                .display()
                .to_string(),
        );
    }

    let plugin_root = home.join(".amplihack").join(".claude");
    if plugin_root.exists() {
        env_builder.set("CLAUDE_PLUGIN_ROOT", plugin_root.display().to_string())
    } else {
        env_builder
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_docker_launcher_args(
    launcher_command: &str,
    resume: bool,
    continue_session: bool,
    skip_update_check: bool,
    no_reflection: bool,
    subprocess_safe: bool,
    checkout_repo: Option<&str>,
    extra_args: &[String],
) -> Vec<String> {
    let is_launch_surface = launcher_command == "launch";
    let mut args = vec![launcher_command.to_string()];
    if resume {
        args.push("--resume".to_string());
    }
    if continue_session {
        args.push("--continue".to_string());
    }
    if skip_update_check && is_launch_surface {
        args.push("--skip-update-check".to_string());
    }
    if no_reflection {
        args.push("--no-reflection".to_string());
    }
    if subprocess_safe {
        args.push("--subprocess-safe".to_string());
    }
    if let Some(repo) = checkout_repo {
        args.push("--checkout-repo".to_string());
        args.push(repo.to_string());
    }
    if !extra_args.is_empty() {
        args.push("--".to_string());
        args.extend(extra_args.iter().cloned());
    }
    args
}
