//! Group (e): the parser exists once in the bundle. Outside
//! `workflow_github_repo.sh`, no bundle tool or recipe may define
//! `parse_github_repo_identity`, keep `_finalization_github_repo`, or carry a
//! github.com prefix glob / case arm. Comment lines are ignored, and the
//! `${x#*@github.com/}` strip that `parse_github_pr_url` uses after the
//! identity check is allowed.

use super::fixture::workspace_root;
use std::fs;
use std::path::{Path, PathBuf};

const CALLERS: &[&str] = &[
    "amplifier-bundle/tools/workflow_pr_ready.sh",
    "amplifier-bundle/tools/workflow_final_status.sh",
    "amplifier-bundle/tools/workflow_pr_scope.sh",
    "amplifier-bundle/tools/workflow_publish_pr.sh",
    "amplifier-bundle/tools/workflow_terminal_evidence.sh",
    "amplifier-bundle/tools/workflow_finalization_pr_signal.sh",
    "amplifier-bundle/tools/workflow_finalization_collect.sh",
    "amplifier-bundle/recipes/workflow-terminal-state.yaml",
];

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn is_script(path: &Path, text: &str) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("sh" | "bash" | "yaml" | "yml")
    ) || text.starts_with("#!")
}

/// Strip the one allowed form, a `${name#*@github.com/}` expansion.
fn without_allowed_strip(line: &str) -> String {
    let mut rest = line.to_owned();
    while let Some(start) = rest.find("${") {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let expansion = &rest[start..start + len + 1];
        let replacement = if expansion.ends_with("#*@github.com/}") {
            ""
        } else {
            "<x>"
        };
        rest.replace_range(start..start + len + 1, replacement);
    }
    rest
}

fn violation(line: &str) -> Option<&'static str> {
    let code = line.trim_start();
    if code.starts_with('#') {
        return None;
    }
    let def = code.strip_prefix("function ").unwrap_or(code).trim_start();
    if def.starts_with("parse_github_repo_identity()")
        || def.starts_with("parse_github_repo_identity ()")
    {
        return Some("defines parse_github_repo_identity");
    }
    if code.contains("_finalization_github_repo") {
        return Some("references _finalization_github_repo");
    }
    let stripped = without_allowed_strip(code);
    for pattern in [
        "*@github.com",
        "git@github.com:*",
        "ssh://git@github.com/*",
        "https://github.com/*",
        "http://github.com/*",
    ] {
        if stripped.contains(pattern) {
            return Some("has a github.com glob or case pattern");
        }
    }
    None
}

#[test]
fn bundle_has_no_parser_copy_outside_the_shared_helper() {
    let root = workspace_root();
    let mut files = Vec::new();
    walk(&root.join("amplifier-bundle/tools"), &mut files);
    walk(&root.join("amplifier-bundle/recipes"), &mut files);
    let mut failures = Vec::new();
    for path in files {
        if path
            .file_name()
            .is_some_and(|n| n == "workflow_github_repo.sh")
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        if !is_script(&path, &text) {
            continue;
        }
        for (n, line) in text.lines().enumerate() {
            if let Some(why) = violation(line) {
                let rel = path.strip_prefix(&root).unwrap_or(&path).display();
                failures.push(format!("{rel}:{}: {why}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_shared_helper_defines_the_parser_exactly_once() {
    let text = fs::read_to_string(super::fixture::helper_path()).expect("shared helper must exist");
    let defs = text
        .lines()
        .filter(|line| {
            line.trim_start()
                .starts_with("parse_github_repo_identity()")
        })
        .count();
    assert_eq!(
        defs, 1,
        "workflow_github_repo.sh must define the parser once"
    );
}

#[test]
fn every_caller_sources_the_shared_helper() {
    let root = workspace_root();
    let mut failures = Vec::new();
    for rel in CALLERS {
        let text = fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        if !text.contains("workflow_github_repo.sh")
            || !text.contains("WORKFLOW_GITHUB_REPO_HELPER")
        {
            failures.push(format!(
                "{rel} must locate workflow_github_repo.sh (with the WORKFLOW_GITHUB_REPO_HELPER override)"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn guard_rules_flag_the_shapes_they_exist_for() {
    for bad in [
        "parse_github_repo_identity() {",
        "  function parse_github_repo_identity() {",
        "    https://*@github.com/*|http://*@github.com/*) github_remote=true ;;",
        "  git@github.com:*) path=x ;;",
        r#"  [[ $u == https://github.com/* ]]"#,
        r#"  repo="$(_finalization_github_repo "$o")""#,
    ] {
        assert!(violation(bad).is_some(), "guard must flag {bad:?}");
    }
    for ok in [
        "# parse_github_repo_identity() was folded into workflow_github_repo.sh",
        r#"  repo="$(parse_github_repo_identity "$url")" || return 1"#,
        r#"    rest="${rest#*@github.com/}""#,
        r#"  path="${url#git@github.com:}""#,
        "        *github.com:*|*github.com/*)",
    ] {
        assert!(violation(ok).is_none(), "guard must allow {ok:?}");
    }
}

/// A line that would turn on bash xtrace, which echoes every expanded
/// command (raw origin URLs and tokens included) to stderr.
fn enables_xtrace(line: &str) -> bool {
    let code = line.trim_start();
    if code.starts_with('#') && !code.starts_with("#!") {
        return false;
    }
    let words: Vec<&str> = code
        .split(|c: char| c.is_whitespace() || c == ';' || c == '&' || c == '|')
        .filter(|w| !w.is_empty())
        .collect();
    let set_flag = words.windows(2).any(|w| {
        w[0] == "set" && w[1].starts_with('-') && !w[1].starts_with("--") && w[1][1..].contains('x')
    });
    let set_o = words
        .windows(3)
        .any(|w| w[0] == "set" && w[1] == "-o" && w[2] == "xtrace");
    let shebang_or_bash_x = words.windows(2).any(|w| {
        (w[0].ends_with("bash") || w[0].ends_with("/sh") || w[0] == "sh")
            && w[1].starts_with('-')
            && !w[1].starts_with("--")
            && w[1][1..].contains('x')
    });
    set_flag || set_o || shebang_or_bash_x || code.contains("BASH_XTRACEFD")
}

#[test]
fn helper_and_callers_never_enable_xtrace() {
    let root = workspace_root();
    let mut failures = Vec::new();
    let helper = "amplifier-bundle/tools/workflow_github_repo.sh";
    for rel in CALLERS.iter().chain(std::iter::once(&helper)) {
        let text = fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"));
        for (n, line) in text.lines().enumerate() {
            if enables_xtrace(line) {
                failures.push(format!("{rel}:{}: enables xtrace: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn xtrace_rule_flags_the_shapes_it_exists_for() {
    for bad in [
        "set -x",
        "  set -euxo pipefail",
        "set -o xtrace",
        "#!/usr/bin/env bash -x",
        "  bash -x \"$helper\"",
        "exec {BASH_XTRACEFD}>&2",
        "true; set -x; parse_github_repo_identity \"$u\"",
    ] {
        assert!(enables_xtrace(bad), "xtrace rule must flag {bad:?}");
    }
    for ok in [
        "set -euo pipefail",
        "set +x",
        "# set -x is banned here (issue #1642)",
        "set -e",
        "  bash \"$helper\"",
        "  printf '%s\\n' \"$x\"",
    ] {
        assert!(!enables_xtrace(ok), "xtrace rule must allow {ok:?}");
    }
}
