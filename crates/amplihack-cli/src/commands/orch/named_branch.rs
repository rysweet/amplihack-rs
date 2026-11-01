//! Whether a task text names its own branch, by the directive rules of
//! `amplifier-bundle/tools/workflow_branch_name.sh explicit` (#1426).
//!
//! `workflow-worktree` gives a task that names its branch (`Branch: <ref>`)
//! that branch, ahead of the `quick/` prefix, so `quick-change` cannot run
//! it (#1740). Only whether a branch is named matters here, not whether it
//! exists, so the shell tool's git lookups are left out and the result is
//! the same for its exit codes 0, 10 and 3. `issue_1740_select_workflow`
//! runs the shell tool and `select-workflow` on the same texts and fails
//! when they disagree.

use std::sync::LazyLock;

use regex::Regex;

const MAX_INPUT_CHARS: usize = 65536;
const MAX_DIRECTIVES: usize = 20;
const RESERVED_REFS: &[&str] = &["main", "master", "develop", "trunk", "head"];

/// `KEY_RE` from `workflow_branch_name.sh`, character for character.
pub const KEY_RE: &str = r"^[[:space:]]*[Bb][Rr][Aa][Nn][Cc][Hh]([[:space:]]+([Nn][Aa][Mm][Ee]|[Rr][Ee][Ff]))?[[:space:]]*[:=]";
/// `DASH_RE` from `workflow_branch_name.sh`, character for character.
pub const DASH_RE: &str =
    r"^[[:space:]]*[Bb][Rr][Aa][Nn][Cc][Hh][[:space:]]+(—|–|-)[^:=]*:[[:space:]]*$";

static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(KEY_RE).expect("static KEY_RE"));
static DASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(DASH_RE).expect("static DASH_RE"));
static SHAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._/-]*$").expect("static ref shape"));

/// `[[:space:]]` in the C locale.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

fn has_text(s: &str) -> bool {
    s.chars().any(|c| !is_space(c))
}

/// The shell tool's `first_token`: the first whitespace-delimited token,
/// without quotes or brackets and without trailing `.`, `,`, `;` or `:`.
fn first_token(s: &str) -> String {
    let token = s
        .trim_start_matches(is_space)
        .split(is_space)
        .next()
        .unwrap_or("");
    let token: String = token
        .chars()
        .filter(|c| !"`\"'<>(){}[]".contains(*c))
        .collect();
    token.trim_end_matches(['.', ',', ';', ':']).to_string()
}

/// The shell tool's `ref_is_valid`. Its shape check leaves only these
/// `git check-ref-format --branch` rules to apply: no component starts with
/// `.` or ends with `.lock`, and the name does not end with `.`.
fn ref_is_valid(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 200
        && SHAPE.is_match(r)
        && !r.contains("..")
        && !r.contains("//")
        && !r.ends_with('/')
        && !r.ends_with('.')
        && r.split('/')
            .all(|c| !c.starts_with('.') && !c.ends_with(".lock"))
}

/// The shell tool's `candidate_ok`.
fn candidate_ok(c: &str) -> bool {
    if !ref_is_valid(c) || !c.contains(['/', '-']) {
        return false;
    }
    let lc = c.to_ascii_lowercase();
    !["origin/", "refs/", "heads/"]
        .iter()
        .any(|p| lc.starts_with(p))
        && !RESERVED_REFS.contains(&lc.as_str())
}

/// True when `task` names a branch in a form `workflow-worktree` acts on:
/// `Branch: <ref>`, `Branch name: <ref>`, `Branch ref: <ref>`,
/// `Branch = <ref>` (the value may be on the next non-blank line instead),
/// or `Branch — <text>:` with the value on the next non-blank line. Only the
/// first 65536 characters and the first 20 directive lines are read.
pub fn task_names_branch(task: &str) -> bool {
    let text: String = task.chars().take(MAX_INPUT_CHARS).collect();
    let mut pending = false;
    let mut seen = 0;
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if pending {
            if !has_text(line) {
                continue;
            }
            pending = false;
            if candidate_ok(&first_token(line)) {
                return true;
            }
        }
        // As in the shell tool, a line that held a pending value is still
        // read as a directive.
        if DASH.is_match(line) {
            pending = true;
        } else if KEY.is_match(line) {
            let rest = line.split_once([':', '=']).map_or("", |(_, rest)| rest);
            if !has_text(rest) {
                pending = true;
            } else if candidate_ok(&first_token(rest)) {
                return true;
            }
        } else {
            continue;
        }
        seen += 1;
        if seen >= MAX_DIRECTIVES {
            break;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_and_bounds_are_the_shell_tools_own() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../amplifier-bundle/tools/workflow_branch_name.sh"
        );
        let shell = std::fs::read_to_string(path).expect("read workflow_branch_name.sh");
        for line in [
            format!("readonly KEY_RE='{KEY_RE}'"),
            format!("readonly DASH_RE='{DASH_RE}'"),
            format!("readonly MAX_INPUT_CHARS={MAX_INPUT_CHARS}"),
            format!("readonly MAX_DIRECTIVES={MAX_DIRECTIVES}"),
            format!("readonly RESERVED_REFS=\" {} \"", RESERVED_REFS.join(" ")),
            r"[[ $ref =~ ^[A-Za-z0-9][A-Za-z0-9._/-]*$ ]] || return 1".to_string(),
        ] {
            assert!(
                shell.lines().any(|l| l.trim() == line),
                "workflow_branch_name.sh no longer has {line:?}; update named_branch.rs to match"
            );
        }
    }

    #[test]
    fn every_directive_form_names_a_branch() {
        for task in [
            "Fix it.\nBranch: feat/x-y\n",
            "Branch name: feat/x-y",
            "  branch ref = feat/x-y",
            "BRANCH = `feat/x-y`.",
            "Branch:\n\n   feat/x-y and more",
            "Branch — use the existing one:\nfeat/x-y",
            "branch – x:\n\n quick-fix",
            "Branch - pinned:\r\nfix/thing\r\n",
            "Branch:\nBranch: feat/x-y",
        ] {
            assert!(task_names_branch(task), "{task:?}");
        }
    }

    #[test]
    fn prose_reserved_and_invalid_refs_do_not() {
        for task in [
            "",
            "Fix the typo in docs/index.md",
            "Branch coverage is low in: src/foo-bar.rs",
            "Use Branch: feat/x-y",
            "Branch: main",
            "Branch: origin/feat-x",
            "Branch: refs/heads/feat-x",
            "Branch: nodashorslash",
            "Branch: feat/x.lock",
            "Branch: feat/.x",
            "Branch: feat/x..y",
            "Branch: -feat/x",
            "Branch — pinned:\n\n",
            "Branch — pinned: feat/x-y",
        ] {
            assert!(!task_names_branch(task), "{task:?}");
        }
    }

    #[test]
    fn only_the_first_twenty_directive_lines_count() {
        let mut task = "Branch: main\n".repeat(20);
        task.push_str("Branch: feat/x-y\n");
        assert!(!task_names_branch(&task));
        let mut task = "Branch: main\n".repeat(19);
        task.push_str("Branch: feat/x-y\n");
        assert!(task_names_branch(&task));
    }

    #[test]
    fn only_the_first_65536_characters_count() {
        let mut task = "x".repeat(MAX_INPUT_CHARS);
        task.push_str("\nBranch: feat/x-y\n");
        assert!(!task_names_branch(&task));
    }
}
