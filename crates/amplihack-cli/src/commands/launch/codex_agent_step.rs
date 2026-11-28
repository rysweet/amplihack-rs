//! Issues #1985 and #2317: flags amplihack adds when a recipe agent step runs
//! on Codex.
//!
//! A recipe agent step starts `amplihack codex -- exec …`. Codex agent steps
//! run with the same trust as Claude agent steps, which run with
//! `--dangerously-skip-permissions`: amplihack adds
//! `--sandbox danger-full-access`, so a step in a linked git worktree can write
//! the shared `.git` directory and commit (#2317), and
//! `--dangerously-bypass-hook-trust`, because Codex silently skips hooks whose
//! trust is not recorded. The amplihack hooks enforce the same deny rules for
//! both harnesses. A sandbox the caller chose is never overridden. This module
//! is pure: it reads only the argv tail. It describes the injection as one stderr line
//! for issue #2010, which the caller prints; the module itself still does no
//! I/O.
//!
//! Whether an option takes a value comes from the per-scope Codex option
//! tables in `amplihack-launcher/src/prompt_delivery/codex_options.rs`, the
//! same tables the prompt rewrite (`codex_exec_args.rs`) reads.

use amplihack_launcher::prompt_delivery::codex_options;

const SANDBOX: [&str; 2] = ["--sandbox", "danger-full-access"];
const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
/// Every value of `--sandbox` in codex-cli 0.166.0. `workspace-write` stays
/// listed so that a caller's attached `-sworkspace-write` is recognised as a
/// sandbox choice and never widened; amplihack does not inject it.
const SANDBOX_MODES: &[&str] = &["read-only", "workspace-write", "danger-full-access"];
/// Flags that choose a sandbox on their own, taken from `codex exec --help`
/// for codex-cli 0.166.0 (frozen in `fixtures/codex-exec-help-0.166.0.txt`
/// and checked by `tests_codex_agent_step.rs`):
///
/// - `--approve-for-me` selects the workspace-write sandbox, and clap rejects
///   it together with `--sandbox` (exit 2), so adding `--sandbox` would stop
///   the launch.
/// - `--dangerously-bypass-approvals-and-sandbox` runs with no sandbox.
/// - `--yolo` is the hidden alias of the bypass flag; it is accepted but not
///   listed in the help output.
///
/// `--full-auto` is not listed: codex-cli 0.140.0 to 0.166.0 reject it as an
/// unexpected argument.
pub(super) const SANDBOX_FLAGS: &[&str] = &[
    "--approve-for-me",
    "--dangerously-bypass-approvals-and-sandbox",
    "--yolo",
];

/// One parsed argv item before the end-of-options marker.
enum Item<'a> {
    Positional(usize, &'a str),
    /// `name`, its value (if the option takes one) and whether the value was
    /// attached to a short option (`-sread-only`).
    Opt {
        end: usize,
        name: &'a str,
        value: Option<&'a str>,
        attached: bool,
    },
}

/// Walks `args` up to a bare `--`, pairing each option with its value so an
/// option's value is never read as a flag or a subcommand.
fn items(args: &[String]) -> Vec<Item<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            break;
        }
        if arg == "-" || !arg.starts_with('-') {
            out.push(Item::Positional(i, arg));
        } else if let Some((name, value)) = arg.split_once('=').filter(|_| arg.starts_with("--")) {
            out.push(Item::Opt {
                end: i + 1,
                name,
                value: Some(value),
                attached: false,
            });
        } else if arg.starts_with("--") || arg.len() == 2 {
            let takes_value = codex_options::takes_value(arg);
            let value = if takes_value {
                args.get(i + 1).map(String::as_str)
            } else {
                None
            };
            i += usize::from(value.is_some());
            out.push(Item::Opt {
                end: i + 1,
                name: arg,
                value,
                attached: false,
            });
        } else if let Some((name, rest)) = arg.get(..2).zip(arg.get(2..)) {
            // Like clap, drop one `=` between a short option and its value.
            let value =
                codex_options::takes_value(name).then(|| rest.strip_prefix('=').unwrap_or(rest));
            out.push(Item::Opt {
                end: i + 1,
                name,
                value,
                attached: value.is_some(),
            });
        } else {
            // A multi-byte character after `-`: pass it through for Codex to reject.
            out.push(Item::Opt {
                end: i + 1,
                name: arg,
                value: None,
                attached: false,
            });
        }
        i += 1;
    }
    out
}

/// Explicit config values and their insertion points, in command-line order.
pub(super) fn config_overrides(args: &[String]) -> impl Iterator<Item = (usize, &str)> {
    items(args).into_iter().filter_map(|item| match item {
        Item::Opt {
            end,
            name: "-c" | "--config",
            value: Some(value),
            ..
        } => Some((end, value)),
        _ => None,
    })
}

pub(super) fn has_option(args: &[String], name: &str, value: Option<&str>) -> bool {
    items(args).into_iter().any(|item| {
        matches!(item, Item::Opt { name: option, value: argument, .. } if option == name && argument == value)
    })
}

/// Index of the `exec`/`e` subcommand when it is the first positional argument.
pub(super) fn exec_subcommand_index(args: &[String]) -> Option<usize> {
    items(args).into_iter().find_map(|item| match item {
        Item::Positional(i, arg) => Some((arg == "exec" || arg == "e").then_some(i)),
        Item::Opt { .. } => None,
    })?
}

/// The key of a `-c`/`--config` override, trimmed as Codex trims it.
fn config_key<'a>(name: &str, value: Option<&'a str>) -> Option<&'a str> {
    if name != "-c" && name != "--config" {
        return None;
    }
    value?.split_once('=').map(|(key, _)| key.trim())
}

fn caller_chose_sandbox(items: &[Item<'_>]) -> bool {
    items.iter().any(|item| match *item {
        Item::Opt {
            name,
            value,
            attached,
            ..
        } => {
            SANDBOX_FLAGS.contains(&name)
                || (name == "--sandbox" && value.is_some())
                || (name == "-s" && value.is_some_and(|v| !attached || SANDBOX_MODES.contains(&v)))
                || config_key(name, value).is_some_and(|key| key == "sandbox_mode")
        }
        Item::Positional(..) => false,
    })
}

fn caller_bypassed_hook_trust(items: &[Item<'_>]) -> bool {
    items
        .iter()
        .any(|item| matches!(item, Item::Opt { name, value: None, .. } if *name == HOOK_TRUST))
}

/// Whether `args` (a Codex argv tail) already passes the hook-trust bypass
/// as a flag before any `--`. The local-model launch adds the flag only when
/// this is false, by the same rule as the agent-step injection, so the two
/// agree and the flag appears exactly once.
pub(super) fn has_hook_trust_bypass(args: &[String]) -> bool {
    caller_bypassed_hook_trust(&items(args))
}

/// What the agent-step injection adds to one argv tail. The argv and the
/// stderr notice are both derived from this, so they cannot disagree.
pub(super) struct Plan {
    /// Index of the `exec`/`e` subcommand; flags are spliced in after it.
    index: usize,
    /// `--sandbox danger-full-access` is added.
    sandbox: bool,
    /// `--dangerously-bypass-hook-trust` is added.
    hook_trust: bool,
}

/// Decides what to inject, or `None` when the argv has no `exec` subcommand.
pub(super) fn plan(args: &[String]) -> Option<Plan> {
    let index = exec_subcommand_index(args)?;
    let parsed = items(args);
    Some(Plan {
        index,
        sandbox: !caller_chose_sandbox(&parsed),
        hook_trust: !caller_bypassed_hook_trust(&parsed),
    })
}

impl Plan {
    /// Older releases don't accept the flag. Leave caller arguments alone.
    pub(super) fn omit_hook_trust(&mut self) {
        self.hook_trust = false;
    }

    pub(super) fn arguments(&self, args: &[String]) -> Vec<String> {
        let mut injected: Vec<&str> = Vec::new();
        if self.sandbox {
            injected.extend(SANDBOX);
        }
        if self.hook_trust {
            injected.push(HOOK_TRUST);
        }
        let mut out = args.to_vec();
        out.splice(
            self.index + 1..self.index + 1,
            injected.into_iter().map(String::from),
        );
        out
    }

    pub(super) fn notice(&self) -> Option<String> {
        if !self.sandbox {
            return None;
        }
        let sandbox = SANDBOX.join(" ");
        let mut flags = vec![sandbox.as_str()];
        if self.hook_trust {
            flags.push(HOOK_TRUST);
        }
        Some(format!(
            "amplihack: codex exec: added {} (#1985)",
            flags.join(", ")
        ))
    }
}

/// Returns the Codex argv tail with the agent-step flags spliced in directly
/// after `exec`/`e` (before `resume`, which rejects `--sandbox` in codex-cli
/// 0.166.0). Argv without an `exec` subcommand is returned unchanged. Adds
/// `--sandbox danger-full-access` only when the caller chose no sandbox, and
/// never adds the approvals bypass or `--yolo`.
#[cfg(test)]
pub(super) fn codex_agent_step_args(args: &[String]) -> Vec<String> {
    plan(args).map_or_else(|| args.to_vec(), |plan| plan.arguments(args))
}

/// Issue #2010: the stderr line naming what [`codex_agent_step_args`] adds,
/// for example `amplihack: codex exec: added --sandbox danger-full-access,
/// --dangerously-bypass-hook-trust (#1985)`.
///
/// Returns `None` when the caller chose a sandbox; the hook-trust-only notice
/// remains tracked separately by #2048. Only fixed constants reach stderr.
#[cfg(test)]
pub(super) fn codex_agent_step_notice(args: &[String]) -> Option<String> {
    plan(args)?.notice()
}
