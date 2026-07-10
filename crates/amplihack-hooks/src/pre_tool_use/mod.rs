//! Pre-tool-use hook: validates bash commands and XPIA security before execution.
//!
//! Blocks:
//! - CWD deletion (rm -rf, rmdir targeting CWD or parent)
//! - CWD rename/move (mv targeting CWD or parent)
//! - Direct commits to main/master branch
//! - Use of --no-verify flag on git commands
//! - XPIA prompt injection attacks (all tools)

mod command;
mod cwd;
mod git;
pub mod launcher;
mod xpia;

use crate::protocol::{FailurePolicy, Hook};
use amplihack_types::HookInput;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// Error messages for blocked operations.
const CWD_DELETION_ERROR: &str = "\
🚫 OPERATION BLOCKED - Working Directory Deletion Prevented\n\
\n\
You attempted to delete a directory that contains your current working directory:\n\
  Target: {target}\n\
  CWD:    {cwd}\n\
\n\
Deleting the CWD would break the current session. If you need to clean up\n\
this directory, first change to a different working directory.\n\
\n\
🔒 This protection cannot be disabled programmatically.";

const CWD_RENAME_ERROR: &str = "\
🚫 OPERATION BLOCKED - Working Directory Rename Prevented\n\
\n\
You attempted to move/rename a directory that contains your current working directory:\n\
  Source: {source}\n\
  CWD:    {cwd}\n\
\n\
Moving or renaming the CWD would break the current session. To rename this directory:\n\
  1. First change to a different working directory (e.g., cd ..)\n\
  2. Then perform the rename operation\n\
  3. Change back into the renamed directory if needed\n\
\n\
🔒 This protection cannot be disabled programmatically.";

const MAIN_BRANCH_ERROR: &str = "\
⛔ Direct commits to '{branch}' branch are not allowed.\n\
\n\
Please use the feature branch workflow:\n\
  1. Create a feature branch: git checkout -b feature/your-feature-name\n\
  2. Make your commits on the feature branch\n\
  3. Create a Pull Request to merge into {branch}\n\
\n\
This protection cannot be bypassed with --no-verify.";

const NO_VERIFY_ERROR: &str = "\
🚫 OPERATION BLOCKED\n\
\n\
You attempted to use --no-verify which bypasses critical quality checks:\n\
- Code formatting (ruff, prettier)\n\
- Type checking (pyright)\n\
- Secret detection\n\
- Trailing whitespace fixes\n\
\n\
This defeats the purpose of our quality gates.\n\
\n\
✅ Instead, fix the underlying issues:\n\
1. Run: pre-commit run --all-files\n\
2. Fix the violations\n\
3. Commit without --no-verify\n\
\n\
For true emergencies, ask a human to override this protection.\n\
\n\
🔒 This protection cannot be disabled programmatically.";

/// Guidance returned when a `Skill` invocation names an amplihack *agent*
/// (e.g. `prompt-writer`) rather than a skill. Without this redirect the
/// copilot runtime hard-fails with "Skill not found: <name>", silently
/// skipping the step (issue #838). The `{name}` placeholder is replaced with
/// the requested (sanitized) agent name.
const SKILL_IS_AGENT_REDIRECT: &str = "\
🔁 WRONG INTERFACE - '{name}' is an amplihack AGENT, not a skill.\n\
\n\
You invoked the Skill tool with name '{name}', but '{name}' is provided as an\n\
agent, not a skill. Invoking it as a skill fails with \"Skill not found\" and\n\
silently skips this step.\n\
\n\
✅ Instead, run '{name}' through the agent interface (the Task/agent tool),\n\
e.g. reference it as an agent such as \"amplihack:{name}\".\n\
\n\
This redirect prevents the requirements-clarification phase from being skipped.";

/// Detect a `Skill` invocation that names an agent rather than a skill and, if
/// so, return a non-fatal block instructing the model to use the agent
/// interface instead. Returns `None` (pass-through) for every other case:
/// genuine skills, names that are both skill and agent (skills take
/// precedence), unknown names, and malformed payloads.
///
/// Parsing is total and panic-free: a missing/non-string/null name simply
/// passes through.
fn check_skill_redirect(tool_name: &str, tool_input: &Value) -> Option<Value> {
    if tool_name != "Skill" {
        return None;
    }

    // Host payloads use either the `skill` key or the `name` key.
    let name = tool_input
        .get("skill")
        .and_then(Value::as_str)
        .or_else(|| tool_input.get("name").and_then(Value::as_str))?;

    // Skills take precedence: only redirect agent-only names. This keeps
    // overlapping names (e.g. gherkin-expert) resolving as skills. The set of
    // skills is derived from the on-disk skills directory (issue #863) — the
    // directory is the single source of truth, not a hardcoded list.
    //
    // Order matters for performance: `is_amplihack_agent` is a cheap
    // `binary_search` over a static array, while `bundled_skill_names` walks the
    // skills directory and reads every `SKILL.md`. Gating the expensive scan
    // behind the cheap check means the directory is only scanned when the name
    // actually collides with a known agent — the common case (genuine skills and
    // unknown names) pays zero I/O. `&&`/`||` short-circuit and both operands are
    // side-effect-free, so this is logically identical to the naive order.
    if !crate::known_agents::is_amplihack_agent(name) || bundled_skill_names().contains(name) {
        return None;
    }

    Some(serde_json::json!({
        "block": true,
        "message": SKILL_IS_AGENT_REDIRECT.replace("{name}", name)
    }))
}

/// Maximum directory depth walked when scanning for bundled `SKILL.md` files.
/// The bundle nests skills at most two levels deep
/// (`category/skill/SKILL.md`); this bound guards against pathological or
/// cyclic directory trees.
const MAX_SKILL_SCAN_DEPTH: usize = 8;

/// Maximum number of `SKILL.md` files read during a single scan. The bundle
/// ships on the order of a hundred skills; this generous cap bounds the
/// per-tool-call hot path against a pathological or hostile directory tree
/// (broad, shallow fan-out that the depth cap alone does not bound) without
/// ever tripping for a legitimate bundle.
const MAX_SKILL_FILES: usize = 10_000;

/// Maximum accepted length of a frontmatter `name:` value. Skill and agent
/// identities are short kebab-case names; anything longer cannot match a real
/// agent and is rejected so a malformed or hostile `SKILL.md` cannot force an
/// unbounded string into the membership set.
const MAX_SKILL_NAME_LEN: usize = 256;

/// Derive the set of bundled skill names by scanning the on-disk skills
/// directory at runtime.
///
/// The skills DIRECTORY is the single source of truth (issue #863): each skill
/// is a directory containing a `SKILL.md` whose YAML frontmatter `name:` field
/// is the skill's identity. That name may differ from the directory name (for
/// example `migrate/` publishes `amplihack-migrate`), so the frontmatter value
/// — never the directory path — is used as the key.
///
/// Roots are resolved via [`iter_runtime_roots`] so the scan matches how every
/// other bundled asset is located. Both the source-bundle layout
/// (`<root>/amplifier-bundle/skills/`) and the installed layout
/// (`<root>/skills/`) are scanned.
///
/// Panic-free and fail-open: unreadable roots or files are skipped and
/// symlinked directories are not traversed. The returned set is empty only when
/// no skills directory is reachable.
fn bundled_skill_names() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut files_scanned = 0usize;
    for root in amplihack_cli::runtime_assets::iter_runtime_roots() {
        for skills_dir in [root.join("amplifier-bundle/skills"), root.join("skills")] {
            collect_skill_frontmatter_names(&skills_dir, &mut names, 0, &mut files_scanned);
        }
    }
    names
}

/// Recursively collect frontmatter `name:` values from every `SKILL.md` under
/// `dir`, up to [`MAX_SKILL_SCAN_DEPTH`] and [`MAX_SKILL_FILES`]. Symlinks are
/// not followed and unreadable entries are skipped.
fn collect_skill_frontmatter_names(
    dir: &Path,
    names: &mut BTreeSet<String>,
    depth: usize,
    files_scanned: &mut usize,
) {
    if depth > MAX_SKILL_SCAN_DEPTH || *files_scanned >= MAX_SKILL_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // Bound the total scan regardless of tree shape (hot-path DoS guard).
        if *files_scanned >= MAX_SKILL_FILES {
            return;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Do not follow symlinks — avoids cycles and out-of-tree escapes.
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_skill_frontmatter_names(&path, names, depth + 1, files_scanned);
        } else if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
            *files_scanned += 1;
            if let Some(name) = skill_frontmatter_name(&path) {
                names.insert(name);
            }
        }
    }
}

/// Extract the frontmatter `name:` value from a `SKILL.md` file, if present and
/// non-empty. Returns `None` for unreadable files or missing frontmatter.
fn skill_frontmatter_name(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let frontmatter = content
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .map(|(frontmatter, _)| frontmatter)?;
    frontmatter
        .lines()
        .find_map(|line| line.trim().strip_prefix("name:"))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty() && name.len() <= MAX_SKILL_NAME_LEN)
}

/// Strip leading env-variable assignments (`VAR=value ...`) and an optional
/// `env` prefix so that `GIT_DIR=/x git commit` is normalized to `git commit`.
///
/// Returns a borrowed suffix of `command`: normalization only removes a leading
/// prefix, never rewrites content, so no allocation is needed on this per-Bash
/// hot path.
fn normalize_command(command: &str) -> &str {
    let mut rest = command.trim();

    // Strip `VAR=value ` prefixes (no quotes in key, value runs until space).
    while let Some(eq_pos) = rest.find('=') {
        let prefix = &rest[..eq_pos];
        // Key must be a valid env var name (alphanumeric + underscore, not starting with digit).
        if prefix.is_empty()
            || !prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || prefix.as_bytes()[0].is_ascii_digit()
        {
            break;
        }
        // Skip past the value (until next unquoted space).
        let after_eq = &rest[eq_pos + 1..];
        let value_end = after_eq.find([' ', '\t']).unwrap_or(after_eq.len());
        let after_value = after_eq[value_end..].trim_start();
        if after_value.is_empty() {
            break; // nothing left after the value — not a prefix
        }
        rest = after_value;
    }

    // Strip optional `env` command prefix.
    if let Some(after_env) = rest.strip_prefix("env ") {
        rest = after_env.trim_start();
        // Also strip any additional VAR=val pairs after `env`.
        while let Some(eq_pos) = rest.find('=') {
            let prefix = &rest[..eq_pos];
            if prefix.is_empty()
                || !prefix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || prefix.as_bytes()[0].is_ascii_digit()
            {
                break;
            }
            let after_eq = &rest[eq_pos + 1..];
            let value_end = after_eq.find([' ', '\t']).unwrap_or(after_eq.len());
            let after_value = after_eq[value_end..].trim_start();
            if after_value.is_empty() {
                break;
            }
            rest = after_value;
        }
    }

    rest
}

/// The pre-tool-use hook.
pub struct PreToolUseHook;

impl Hook for PreToolUseHook {
    fn name(&self) -> &'static str {
        "pre_tool_use"
    }

    fn failure_policy(&self) -> FailurePolicy {
        FailurePolicy::Open
    }

    fn process(&self, input: HookInput) -> anyhow::Result<Value> {
        let (tool_name, tool_input) = match input {
            HookInput::PreToolUse {
                tool_name,
                tool_input,
                ..
            } => (tool_name, tool_input),
            _ => return Ok(Value::Object(serde_json::Map::new())),
        };

        // XPIA security validation for all tools.
        if let Some(block) = xpia::check_xpia(&tool_name, &tool_input) {
            return Ok(block);
        }

        // Issue #838: a Skill invocation that names an amplihack *agent* (not a
        // skill) must be redirected to the agent interface rather than letting
        // the runtime hard-fail with "Skill not found", which silently skips
        // the step (e.g. the requirements-clarification phase).
        if let Some(block) = check_skill_redirect(&tool_name, &tool_input) {
            return Ok(block);
        }

        // Only process Bash tool invocations for CWD/git checks.
        if tool_name != "Bash" {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        let command = tool_input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("");

        if command.is_empty() {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // Check CWD deletion.
        if let Some(block) = cwd::check_cwd_deletion(command)? {
            return Ok(block);
        }

        // Check CWD rename/move.
        if let Some(block) = cwd::check_cwd_rename(command)? {
            return Ok(block);
        }

        let normalized = normalize_command(command);
        let is_git_commit = normalized.contains("git commit");
        let is_git_push = normalized.contains("git push");
        let is_git_rebase = normalized.contains("git rebase");
        let is_git_merge = normalized.contains("git merge");
        let is_git_cherry_pick = normalized.contains("git cherry-pick");
        let is_git_am = normalized.contains("git am");
        let has_no_verify = command.contains("--no-verify");
        let is_git_command = is_git_commit
            || is_git_push
            || is_git_rebase
            || is_git_merge
            || is_git_cherry_pick
            || is_git_am;

        if !is_git_command {
            return Ok(Value::Object(serde_json::Map::new()));
        }

        // Check main branch protection for git commit.
        if is_git_commit && let Some(block) = git::check_main_branch()? {
            return Ok(block);
        }

        // Check --no-verify flag.
        if has_no_verify && is_git_command {
            return Ok(serde_json::json!({
                "block": true,
                "message": NO_VERIFY_ERROR
            }));
        }

        Ok(Value::Object(serde_json::Map::new()))
    }
}

#[cfg(test)]
#[path = "tests/pre_tool_use_tests.rs"]
mod tests;
