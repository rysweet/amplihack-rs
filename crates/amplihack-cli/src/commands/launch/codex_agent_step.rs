//! Issue #1985: flags amplihack adds when a recipe agent step runs on Codex.
//!
//! A recipe agent step starts `amplihack codex -- exec …`. Without these flags
//! the step inherits the host `config.toml`, which may set
//! `sandbox_mode = "danger-full-access"`, and Codex silently skips hooks whose
//! trust is not recorded. This module is pure: it reads only the argv tail.
//!
//! The value-taking option tables are shared with the launcher's parser in
//! `amplihack-launcher/src/prompt_delivery/codex_args.rs`.

use amplihack_launcher::prompt_delivery::{LONG_OPTIONS_WITH_VALUE, SHORT_OPTIONS_WITH_VALUE};

const SANDBOX: [&str; 2] = ["--sandbox", "workspace-write"];
const NETWORK: [&str; 2] = ["-c", "sandbox_workspace_write.network_access=true"];
const HOOK_TRUST: &str = "--dangerously-bypass-hook-trust";
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
                name,
                value: Some(value),
                attached: false,
            });
        } else if arg.starts_with("--") || arg.len() == 2 {
            let takes_value =
                LONG_OPTIONS_WITH_VALUE.contains(&arg) || SHORT_OPTIONS_WITH_VALUE.contains(&arg);
            let value = if takes_value {
                args.get(i + 1).map(String::as_str)
            } else {
                None
            };
            i += usize::from(value.is_some());
            out.push(Item::Opt {
                name: arg,
                value,
                attached: false,
            });
        } else if let Some((name, rest)) = arg.get(..2).zip(arg.get(2..)) {
            // Like clap, drop one `=` between a short option and its value.
            let value = SHORT_OPTIONS_WITH_VALUE
                .contains(&name)
                .then(|| rest.strip_prefix('=').unwrap_or(rest));
            out.push(Item::Opt {
                name,
                value,
                attached: value.is_some(),
            });
        } else {
            // A multi-byte character after `-`: pass it through for Codex to reject.
            out.push(Item::Opt {
                name: arg,
                value: None,
                attached: false,
            });
        }
        i += 1;
    }
    out
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
        } => {
            SANDBOX_FLAGS.contains(&name)
                || (name == "--sandbox" && value.is_some())
                || (name == "-s" && value.is_some_and(|v| !attached || SANDBOX_MODES.contains(&v)))
                || config_key(name, value).is_some_and(|key| key == "sandbox_mode")
        }
        Item::Positional(..) => false,
    })
}

fn caller_set_network(items: &[Item<'_>]) -> bool {
    items.iter().any(|item| match *item {
        Item::Opt { name, value, .. } => config_key(name, value)
            .is_some_and(|key| key == "sandbox_workspace_write.network_access"),
        Item::Positional(..) => false,
    })
}

fn caller_bypassed_hook_trust(items: &[Item<'_>]) -> bool {
    items
        .iter()
        .any(|item| matches!(item, Item::Opt { name, value: None, .. } if *name == HOOK_TRUST))
}

/// Returns the Codex argv tail with the agent-step flags spliced in directly
/// after `exec`/`e` (before `resume`, which rejects `--sandbox` in codex-cli
/// 0.166.0). Argv without an `exec` subcommand is returned unchanged. Never
/// adds the approvals bypass, `--yolo` or `danger-full-access`.
pub(super) fn codex_agent_step_args(args: &[String]) -> Vec<String> {
    let Some(index) = exec_subcommand_index(args) else {
        return args.to_vec();
    };
    let parsed = items(args);
    let mut injected: Vec<&str> = Vec::new();
    if !caller_chose_sandbox(&parsed) {
        injected.extend(SANDBOX);
        if !caller_set_network(&parsed) {
            injected.extend(NETWORK);
        }
    }
    if !caller_bypassed_hook_trust(&parsed) {
        injected.push(HOOK_TRUST);
    }
    let mut out = args.to_vec();
    out.splice(index + 1..index + 1, injected.into_iter().map(String::from));
    out
}
