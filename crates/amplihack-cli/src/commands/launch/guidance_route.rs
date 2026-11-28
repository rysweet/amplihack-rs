//! Where an agent launch puts the run's operator guidance (issues #2535 and
//! #3162), and how a launch that cannot take it is named in the drop record.
//!
//! The decision is pure: it takes whether the caller's stdin may hold a
//! prompt instead of looking at the process, so every launch form can be
//! tested.
//!
//! | Tool    | Launch form                                | Route                         |
//! |---------|--------------------------------------------|-------------------------------|
//! | Claude  | `--input-format` other than `text`         | `LaunchFormUnsupported` drop  |
//! | Claude  | `-p <text>` (rewritten to stdin)           | `Prompt`                      |
//! | Claude  | `-p`/`--print`, stdin may hold a prompt    | `StdinTrailer`                |
//! | Claude  | `-p`/`--print`, nothing on stdin           | `NoPromptOnStdin` drop        |
//! | Claude  | interactive                                | `LaunchFormUnsupported` drop  |
//! | Codex   | `exec … -`, stdin may hold a prompt        | `StdinTrailer`                |
//! | Codex   | `exec … -`, nothing on stdin               | `NoPromptOnStdin` drop        |
//! | Codex   | any other form                             | `LaunchFormUnsupported` drop  |
//! | Copilot | `-p <text>` (prompt file)                  | `Prompt`                      |
//! | Copilot | `-p` pointer to a prompt file (the runner) | `Prompt`, in a private copy   |
//! | Copilot | `-p` pointer, file not read or copied      | `PromptFileNotCopied` drop    |
//! | Copilot | no `-p`/`-i`, stdin may hold a prompt      | `StdinTrailer`                |
//! | Copilot | no `-p`/`-i`, nothing on stdin             | `NoPromptOnStdin` drop        |
//! | Copilot | `-i`, or no private file Copilot can open  | `LaunchFormUnsupported` drop  |
//!
//! Issue #3435: a `LaunchFormUnsupported` route is settled once the launch
//! knows whether the step has a hook that gives a running step new guidance
//! ([`step_has_hook`]). With a hook, nothing is dropped: the step's first
//! UserPromptSubmit or PostToolUse hook delivers the entries
//! ([`GuidanceRoute::Hook`]). Without one, the drop is recorded as before.
//!
//! | Tool    | Has a hook that gives a running step new guidance              |
//! |---------|----------------------------------------------------------------|
//! | Claude  | when the launch passes amplihack's hooks settings file         |
//! | Codex   | when the Codex home holds amplihack's PostToolUse and          |
//! |         | UserPromptSubmit handlers and hooks are not switched off       |
//! | Copilot | never                                                          |
//!
//! The recipe runner starts every Copilot step with `-p` and a pointer
//! sentence naming its own prompt file. The launcher copies that file to a
//! private one with the block after the prompt, and points Copilot at the
//! copy; the runner's file is never written (PR #3190 review).
//!
//! A Claude launch with `--input-format stream-json` (#3036) reads JSON
//! lines on stdin, so plain text after them would corrupt the stream; it is
//! a recorded drop, as before this route existed.
//!
//! "May hold a prompt" is all the route can know before the child starts:
//! `fstat` says stdin is a pipe, a socket or a non-empty file, not what is
//! in it. A terminal, `/dev/null` or an empty file holds nothing, and the
//! launch is a `NoPromptOnStdin` drop at once. For the rest, the decision
//! is made by reading: the forwarder writes the block only after it has
//! copied a byte of the caller's stdin that is not white space
//! (`spawn_stdin_forwarder_with_trailer`). A pipe that reaches its end
//! empty or blank, or that has given no text when the step exits, gets no
//! block, and the launch records `NoPromptOnStdin` then, after the step
//! (PR #3190 review). It is never reported as read.
//!
//! Copilot and stdin: Copilot CLI 1.0.114 and 1.0.115, launched with no
//! prompt flag and a piped stdin, took the whole of stdin as their prompt,
//! including a block after the caller's text (prompt and trailer each asked
//! for a token; both were printed). The launcher's half of this route (the
//! block after the prompt, `guidance_injected` only after a full write,
//! `trailer_not_written` when Copilot closes stdin first, `no_prompt_on_stdin`
//! with nothing on stdin) is checked in CI against a stand-in Copilot by
//! `tests/issue_3162_copilot_stdin_launcher.rs`. Copilot's half, that it
//! reads the block at all, is checked only by
//! `tests/issue_3162_copilot_stdin_live.rs`, which is run by hand against
//! the installed Copilot because it needs a signed-in Copilot and a model
//! call; run it after a Copilot upgrade. For this route, "read" therefore
//! means the block was written in full to Copilot's stdin, not that a
//! check has shown the installed Copilot reads it. Issue #3225 tracks an
//! automated form of that check.

use super::codex_copilot_rewrite;
use super::passthrough_rewrite::{self, PromptBlock};
use crate::run_guidance::{DropReason, GuidanceForLaunch, StepLabel};
use amplihack_launcher::flag_matrix::AgentBinary;

/// Where this launch puts the run's operator guidance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuidanceRoute {
    /// No guidance for this run.
    None,
    /// After the `-p` prompt: on stdin (Claude) or in the prompt file
    /// (Copilot). Reported as soon as the agent starts.
    Prompt,
    /// After the caller's stdin, which may hold the prompt (Claude print
    /// mode, Codex `exec … -`, Copilot with no prompt flag). Written only
    /// after text has been copied from it; reported only after the child
    /// exits, and only if every byte was written.
    StdinTrailer,
    /// There is guidance, no prompt can take it, and the step's hooks give
    /// it at its first prompt or tool call (issue #3435).
    Hook,
    /// There is guidance, and this launch cannot take it.
    Undeliverable(DropReason),
}

/// Whether a step of `tool` has a hook that gives it guidance appended
/// while it runs (issue #3435): Claude when amplihack's hooks settings file
/// is passed to it, Codex when its Codex home has amplihack's handlers, and
/// no other tool. A launch whose settings file was skipped counts as having
/// none, even if other settings may still run the hooks, because delivery
/// cannot be counted on.
pub(crate) fn step_has_hook(
    tool: &str,
    claude_settings_passed: bool,
    codex_hooks_ready: bool,
) -> bool {
    match agent_binary_for_tool(tool) {
        Some(AgentBinary::Claude) => claude_settings_passed,
        Some(AgentBinary::Codex) => codex_hooks_ready,
        _ => false,
    }
}

/// `route`, with a launch form that no prompt can take given to the step's
/// hooks when it has them.
pub(crate) fn settle_route(route: GuidanceRoute, hooked: bool) -> GuidanceRoute {
    match route {
        GuidanceRoute::Undeliverable(DropReason::LaunchFormUnsupported) if hooked => {
            GuidanceRoute::Hook
        }
        other => other,
    }
}

pub(crate) fn agent_binary_for_tool(tool: &str) -> Option<AgentBinary> {
    match tool {
        "claude" | "rusty" | "rustyclawd" => Some(AgentBinary::Claude),
        "copilot" => Some(AgentBinary::Copilot),
        "codex" => Some(AgentBinary::Codex),
        "amplifier" => Some(AgentBinary::Amplifier),
        _ => None,
    }
}

/// Whether a stdin that may hold a prompt takes the trailer, or the launch
/// is a drop because nothing can be there.
fn on_stdin(stdin_may_hold_prompt: bool) -> GuidanceRoute {
    if stdin_may_hold_prompt {
        GuidanceRoute::StdinTrailer
    } else {
        GuidanceRoute::Undeliverable(DropReason::NoPromptOnStdin)
    }
}

/// See the module documentation for the table this implements.
pub(crate) fn guidance_route(
    tool: &str,
    guidance: &GuidanceForLaunch,
    rewritten: &passthrough_rewrite::Rewritten,
    original_args: &[String],
    stdin_may_hold_prompt: bool,
) -> GuidanceRoute {
    if guidance.block().is_none() {
        return GuidanceRoute::None;
    }
    let unsupported = GuidanceRoute::Undeliverable(DropReason::LaunchFormUnsupported);
    match agent_binary_for_tool(tool) {
        Some(AgentBinary::Claude) if !passthrough_rewrite::claude_input_is_text(original_args) => {
            unsupported
        }
        Some(AgentBinary::Claude) if rewritten.stdin_payload.is_some() => GuidanceRoute::Prompt,
        Some(AgentBinary::Claude) if passthrough_rewrite::claude_print_mode(original_args) => {
            on_stdin(stdin_may_hold_prompt)
        }
        Some(AgentBinary::Copilot) => match rewritten.prompt_block {
            PromptBlock::Added => GuidanceRoute::Prompt,
            PromptBlock::PromptFileNotCopied => {
                GuidanceRoute::Undeliverable(DropReason::PromptFileNotCopied)
            }
            PromptBlock::Absent
                if !codex_copilot_rewrite::copilot_has_prompt_flag(original_args) =>
            {
                on_stdin(stdin_may_hold_prompt)
            }
            PromptBlock::Absent => unsupported,
        },
        Some(AgentBinary::Codex)
            if rewritten.stdin_payload.is_none()
                && crate::run_guidance::codex_trailer_applies(original_args) =>
        {
            on_stdin(stdin_may_hold_prompt)
        }
        _ => unsupported,
    }
}

/// The longest agent name a label carries.
const MAX_AGENT_BYTES: usize = 64;

/// An agent name safe for a log line: `[A-Za-z0-9._-]`, anything else
/// replaced by `_`, at most [`MAX_AGENT_BYTES`]; `None` when nothing usable
/// is left.
fn sanitize_agent(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_AGENT_BYTES)
        .collect();
    let trimmed = cleaned.trim_matches('_');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The value of `flag` in `args` (`--flag value` or `--flag=value`), before
/// any `--`.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut iter = args.iter().take_while(|a| a.as_str() != "--");
    while let Some(arg) = iter.next() {
        if arg == flag {
            return iter.next().map(String::as_str);
        }
        if let Some(value) = arg
            .strip_prefix(flag)
            .and_then(|rest| rest.strip_prefix('='))
        {
            return Some(value);
        }
    }
    None
}

/// The agent the arguments name: `--agent <name>`, or the file stem of
/// `--system-prompt-file <path>`.
fn agent_from_args(args: &[String]) -> Option<String> {
    if let Some(agent) = flag_value(args, "--agent") {
        return sanitize_agent(agent);
    }
    flag_value(args, "--system-prompt-file")
        .and_then(|path| std::path::Path::new(path).file_stem())
        .and_then(|stem| stem.to_str())
        .and_then(sanitize_agent)
}

/// A fixed label for the launch form. It is chosen from a closed set and
/// holds no argv text.
fn launch_form(tool: &str, args: &[String]) -> &'static str {
    match agent_binary_for_tool(tool) {
        Some(AgentBinary::Claude) => {
            if !passthrough_rewrite::claude_input_is_text(args) {
                "claude, input format other than text"
            } else if passthrough_rewrite::claude_print_mode(args) {
                if passthrough_rewrite::claude_print_prompt_in_argv(args) {
                    "claude -p <prompt>"
                } else {
                    "claude -p, prompt expected on stdin"
                }
            } else {
                "claude, interactive"
            }
        }
        Some(AgentBinary::Codex) => {
            if crate::run_guidance::codex_trailer_applies(args) {
                "codex exec -, prompt expected on stdin"
            } else if args.iter().any(|a| a == "exec") {
                "codex exec <prompt>"
            } else {
                "codex, interactive"
            }
        }
        Some(AgentBinary::Copilot) => {
            if codex_copilot_rewrite::copilot_has_prompt_text(args) {
                "copilot -p <prompt>"
            } else if codex_copilot_rewrite::copilot_has_prompt_pointer(args) {
                "copilot -p <prompt file>"
            } else if codex_copilot_rewrite::copilot_has_prompt_flag(args) {
                "copilot -i"
            } else {
                "copilot, prompt expected on stdin"
            }
        }
        Some(AgentBinary::Amplifier) => "amplifier",
        None => "other",
    }
}

/// What a drop record says about the step: the recipe run id the launcher
/// was started under (`unknown` when it is missing or invalid), the tool,
/// the launch form and the agent name.
pub(crate) fn step_label(tool: &str, args: &[String]) -> StepLabel {
    let run_id = std::env::var("AMPLIHACK_RECIPE_RUN_ID").unwrap_or_default();
    let step_run_id = if crate::commands::recipe::run_id::validate_run_id(&run_id) {
        run_id
    } else {
        "unknown".to_string()
    };
    let tool_label = match agent_binary_for_tool(tool) {
        Some(_) => tool.to_string(),
        None => "other".to_string(),
    };
    StepLabel {
        step_run_id,
        tool: tool_label,
        launch_form: launch_form(tool, args).to_string(),
        agent: agent_from_args(args),
    }
}

/// Says on stderr that the guidance was not delivered, and records the drop
/// in each folder it came from.
pub(crate) fn report_guidance_drop(
    guidance: &GuidanceForLaunch,
    label: &StepLabel,
    reason: DropReason,
) {
    eprintln!(
        "{}",
        crate::run_guidance::sanitize::sanitize_line(&format!(
            "amplihack: warning: operator guidance was not delivered to {} ({}): {}: {}",
            label.tool,
            label.launch_form,
            reason.code(),
            reason.detail()
        ))
    );
    guidance.record_drop(label, reason);
}
