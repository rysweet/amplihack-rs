//! Issue #1551: inside an auto-drive run, refuse the direct merge commands an
//! agent types into its Bash tool.
//!
//! The primary guard is the `gh` shim (`amplifier-bundle/tools/gh-merge-guard/gh`),
//! which the recipe runner puts first on `PATH` and re-asserts through
//! `BASH_ENV` after a login shell's profile has run. This check is a second,
//! deliberately small layer for the same ordinary forms, independent of which
//! `gh` `PATH` finds:
//!
//! * `gh … pr … merge` where `gh` (any path) is the program being run;
//! * `gh api <endpoint>` whose endpoint is `…/pulls/<n>/merge` or a
//!   `…/merges` endpoint.
//!
//! Running `autodrive_merge_gate.sh` is not one of them. The shim trusts only
//! a parent that is `/bin/bash -p <gate>` with a clean environment, and the
//! runner exports `BASH_ENV` into every agent shell, so a gate run from an
//! agent's shell has its merge refused by the shim. Clearing the environment
//! first to get past that is deliberate evasion, the stated non-goal; reading
//! shell options here to spot gate runs only produced false positives
//! (`bash -n`, `command -v`, `bash -c '… gate'`; crusty rounds 11–15).
//!
//! The program word is the first word of each command, after `VAR=value`
//! assignments and the wrappers in [`super::shell::WRAPPERS`]. Nothing else
//! is read: quoted text, arguments of other programs, heredoc bodies and
//! comments are data, so `git commit -m '… gh pr merge …'`,
//! `echo gh pr merge 38 > notes`, `grep -rn 'gh pr merge' .` and
//! `cat <<'EOF' … EOF` pass in any context.
//!
//! It splits commands with the tokeniser shared with the tmux guard
//! ([`super::shell`]) but does not look inside embedded scripts. Code hidden from
//! the program word (`bash -c '…'`, a script file, `eval`, an alias, a
//! variable, `curl` with the token) is outside what this check reads. The
//! ordinary forms are covered by the shim; deliberate evasion with the same
//! token is a stated non-goal (docs/reference/auto-drive-to-merge.md, "What
//! this does not stop"), backstopped by branch protection and the `MERGED`
//! checks after each phase.
//!
//! Active only when `AMPLIHACK_MERGE_GUARD` is non-empty, which the recipe
//! runner sets for every auto-drive run and every agent CLI inside one.

use super::shell::{basename, commands, program};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Set by `amplihack recipe run` for auto-drive runs (see
/// `crates/amplihack-cli/src/commands/recipe/run/merge_guard.rs`).
pub(super) const MERGE_GUARD_ENV: &str = "AMPLIHACK_MERGE_GUARD";

/// `gh api` options that take a value (so the value is not the endpoint).
const GH_API_VALUE_OPTS: &[&str] = &[
    "-X",
    "--method",
    "-f",
    "--raw-field",
    "-F",
    "--field",
    "-H",
    "--header",
    "--input",
    "-q",
    "--jq",
    "-t",
    "--template",
    "-p",
    "--preview",
    "--cache",
    "--hostname",
];

/// `gh` global options that take a value.
const GH_VALUE_OPTS: &[&str] = &["-R", "--repo"];

const BLOCK_MESSAGE: &str = "\
🚫 OPERATION BLOCKED - merge outside auto-drive's merge gate (amplihack #1551)\n\
\n\
This is an auto-drive run (AMPLIHACK_MERGE_GUARD is set) and the command\n\
runs {label}. Only autodrive_merge_gate.sh may merge: leave the PR open.\n\
Auto-drive's merge gate merges it after a Crusty CLEAN and the merge-ready loop,\n\
from its own recipe step; an agent's shell never runs the gate.\n\
Do not retry with another gh path, PATH, alias, API client or token.";

/// REST merge endpoints: `…/pulls/<n>/merge` and `repos/<o>/<r>/merges` (or
/// `repositories/<id>/merges`), any case, with an optional trailing slash,
/// query or fragment.
static ENDPOINT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|/)(?:pulls/[^/?#\s]+/merge|(?:repos/[^/?#\s]+/[^/?#\s]+|repositories/[^/?#\s]+)/merges)/?(?:$|[?#])",
    )
    .expect("static regex")
});

/// Whether `value` (the inherited [`MERGE_GUARD_ENV`]) switches the check on:
/// any non-empty value does, matching the recipe runner.
pub(super) fn guard_active(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// The block response for a Bash `command` that merges, or `None`.
pub(super) fn check(command: &str) -> Option<Value> {
    let label = commands(command).iter().find_map(|c| merge_form(c))?;
    Some(serde_json::json!({
        "block": true,
        "message": BLOCK_MESSAGE.replace("{label}", label),
    }))
}

/// The label of the merge form `words` (one simple command) runs, if any.
fn merge_form(words: &[String]) -> Option<&'static str> {
    let rest = program(words)?;
    let (name, args) = rest.split_first()?;
    if basename(name) != "gh" {
        return None;
    }
    gh_merge_form(args)
}

/// `gh`'s arguments: `pr merge`, or `api` with a merge endpoint.
fn gh_merge_form(args: &[String]) -> Option<&'static str> {
    let mut positional = operands(args, GH_VALUE_OPTS);
    match positional.next().map(|(_, w)| w.as_str()) {
        Some("pr") => (positional.next()?.1 == "merge").then_some("gh pr merge"),
        Some("api") => {
            let (at, _) = operands(args, GH_VALUE_OPTS).find(|(_, w)| w.as_str() == "api")?;
            let (_, endpoint) = operands(&args[at + 1..], GH_API_VALUE_OPTS).next()?;
            ENDPOINT_RE
                .is_match(endpoint)
                .then_some("gh api on a merge endpoint")
        }
        _ => None,
    }
}

/// The non-option words of `args` with their indexes, skipping the value of
/// each option in `value_opts`.
fn operands<'a>(
    args: &'a [String],
    value_opts: &'a [&'a str],
) -> impl Iterator<Item = (usize, &'a String)> + 'a {
    let mut skip_next = false;
    args.iter().enumerate().filter(move |(_, w)| {
        if std::mem::take(&mut skip_next) {
            return false;
        }
        if value_opts.contains(&w.as_str()) {
            skip_next = true;
            return false;
        }
        !w.starts_with('-')
    })
}

#[cfg(test)]
#[path = "tests/merge_guard_tests.rs"]
mod tests;
