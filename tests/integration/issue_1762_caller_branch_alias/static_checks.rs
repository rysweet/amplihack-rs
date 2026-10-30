//! Static guards for #1762: the two helpers contain no write commands, step 4
//! adds nothing destructive, the consensus recipe keeps the #858 refusal, and
//! the documentation is linked (D8, D17, R16, S6, S8, AC15).

use regex::Regex;

use super::fixture::*;

/// `git` as a command word (start of line, after a separator, or in `$(`),
/// with its global options, followed by `sub`. A `git` inside a quoted
/// message ('git push -u origin <B>') is preceded by a quote and not matched.
fn git_invocation(sub: &str) -> Regex {
    let sub = sub.replace(' ', r"\s+");
    Regex::new(&format!(
        r#"(?:^|[\s;&|(]|\$\()git(?:\s+(?:-C|-c)\s+\S+|\s+--no-optional-locks|\s+--no-pager)*\s+{sub}(?:\s|$|[;)|&])"#
    ))
    .unwrap()
}

#[test]
fn helpers_contain_no_write_commands() {
    let forbidden = [
        "fetch",
        "push",
        "pull",
        "update-ref",
        "reset",
        "checkout",
        "switch",
        "stash",
        "clean",
        "commit",
        "merge",
        "rebase",
        "tag",
        "worktree add",
        "worktree remove",
        "worktree prune",
        "worktree move",
        "branch -D",
        "branch -d",
        "branch -m",
        "branch -M",
        "branch -c",
        "branch -f",
        "branch --set-upstream-to",
        "remote set-head",
    ];
    let forbidden: Vec<(&str, Regex)> = forbidden
        .into_iter()
        .map(|sub| (sub, git_invocation(sub)))
        .collect();
    let config = git_invocation("config");
    let file_writes =
        Regex::new(r"(?:^|[\s;&|(]|\$\()(?:rm|mkdir|touch|mv|cp|ln|mktemp)\s").unwrap();
    for name in [PLANNER, RESOLVER] {
        let text = read(&tool_path(name));
        for line in code_lines(&text) {
            for (sub, pattern) in &forbidden {
                assert!(
                    !pattern.is_match(line),
                    "{name} must only read, but runs `git {sub}`: {line}"
                );
            }
            assert!(
                !config.is_match(line) || line.contains("--get"),
                "{name}: every `git config` must be a `--get` read: {line}"
            );
            assert!(
                !file_writes.is_match(line),
                "{name} writes no files: {line}"
            );
            assert!(
                !line.contains("awk -v"),
                "{name}: no `awk -v` on branch or path values (S8): {line}"
            );
        }
    }
    let planner = read(&tool_path(PLANNER));
    let status = git_invocation("status");
    assert!(
        code_lines(&planner).any(|l| status.is_match(l)
            && l.contains("--no-optional-locks")
            && l.contains("--untracked-files=no")),
        "the dirty check is `git --no-optional-locks status --porcelain --untracked-files=no` (D4)"
    );
}

/// D8: step 4 adds no `exit 2`, `branch -D`, `reset --hard`, `clean -f` or
/// `checkout -- .`, and its only `--force` is the existing one in
/// wt_add_idempotent.
#[test]
fn step04_adds_nothing_destructive() {
    let body = step_text("workflow-worktree", "step-04-setup-worktree", "command");
    for pattern in [
        "exit 2",
        "branch -D",
        "reset --hard",
        "clean -f",
        "checkout -- .",
    ] {
        assert!(
            code_lines(&body).all(|l| !l.contains(pattern)),
            "step 4 must not contain `{pattern}`"
        );
    }
    let forced: Vec<&str> = code_lines(&body)
        .filter(|l| l.contains("--force"))
        .collect();
    assert_eq!(
        forced.len(),
        1,
        "step 4 has exactly one --force, in wt_add_idempotent: {forced:?}"
    );
    assert!(
        forced[0].contains("git worktree remove --force \"$wt\""),
        "{forced:?}"
    );
}

/// R16/D8: only default-workflow step 4 creates aliases. Its MIRROR comment
/// names the real consensus recipe, which keeps the #858 refusal.
#[test]
fn consensus_recipe_keeps_858_refusal() {
    let step4 = step_text("workflow-worktree", "step-04-setup-worktree", "command");
    assert!(
        !step4.contains("consensus-workflow.yaml"),
        "the MIRROR comment names a recipe that does not exist"
    );
    assert!(
        step4
            .lines()
            .any(|l| l.trim_start().starts_with('#')
                && l.contains("consensus-issue-worktree.yaml")),
        "the MIRROR comment names consensus-issue-worktree.yaml"
    );
    assert!(
        step4
            .lines()
            .any(|l| l.trim_start().starts_with('#') && l.contains("#1762")),
        "step 4's comments say why the two recipes now differ (#1762)"
    );
    assert!(
        !step4.contains("shell metachars"),
        "check-ref-format does not reject shell metacharacters; the comment must not say it does"
    );
    let consensus = step_text("consensus-issue-worktree", "step3-setup-worktree", "prompt");
    assert!(
        consensus
            .contains("refusing to use the caller checkout as a recipe task worktree (issue #858)"),
        "consensus-issue-worktree.yaml keeps the #858 refusal"
    );
    assert!(
        !consensus.contains(PLANNER),
        "consensus-issue-worktree.yaml does not create aliases"
    );
}

#[test]
fn alias_behaviour_is_documented_and_linked() {
    let root = workspace_root();
    let page = "issue-1762-caller-branch-worktree-alias.md";
    let howto = read(&root.join("docs/recipes").join(page));
    for needle in [PLANNER, RESOLVER, "--wt-", "HEAD:", "#858"] {
        assert!(howto.contains(needle), "the how-to covers `{needle}`");
    }
    assert!(
        read(&root.join("docs/recipes/README.md")).contains(page),
        "docs/recipes/README.md links the how-to"
    );
    assert!(
        read(&root.join("mkdocs.yml")).contains(&format!("recipes/{page}")),
        "mkdocs.yml has the how-to in its nav"
    );
    for doc in [
        "docs/features/branch-name-generation.md",
        "docs/recipes/issue-1121-relative-repo-path.md",
    ] {
        assert!(
            read(&root.join(doc)).contains("#1762"),
            "{doc} describes the alias"
        );
    }
    let changelog = read(&root.join("CHANGELOG.md"));
    let unreleased = changelog
        .split("\n## [")
        .find(|s| s.starts_with("Unreleased]"))
        .expect("CHANGELOG.md has an [Unreleased] section");
    let fixed = unreleased
        .split("\n### ")
        .find(|s| s.starts_with("Fixed"))
        .expect("[Unreleased] has a Fixed section");
    assert!(
        fixed.contains("#1762"),
        "CHANGELOG.md [Unreleased] Fixed names #1762"
    );
}

/// No caller hides a missing resolver or falls back to the local branch name,
/// which from an alias worktree is `<B>--wt-<n>` (philosophy review of #1782).
#[test]
fn resolver_callers_do_not_fall_back_to_local_branch() {
    let call = Regex::new(r#"\$\(bash "[^\n]*?remote_branch(?:\.sh)?"[^)\n]*\)""#).unwrap();
    let mut calls = Vec::new();
    for dir in ["amplifier-bundle/recipes", "amplifier-bundle/tools"] {
        let dir = workspace_root().join(dir);
        for entry in std::fs::read_dir(&dir).expect("read bundle dir").flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            for line in text.replace("\\\n", " ").lines() {
                for found in call.find_iter(line) {
                    let rest = line[found.end()..].trim_start();
                    let fallback = rest
                        .strip_prefix("||")
                        .map(|r| r.split(';').next().unwrap_or(""))
                        .unwrap_or("");
                    calls.push((path.clone(), found.as_str().to_owned(), fallback.to_owned()));
                }
            }
        }
    }
    assert_eq!(calls.len(), 15, "resolver calls found: {calls:#?}");
    for (path, found, fallback) in &calls {
        assert!(
            !found.contains("2>/dev/null"),
            "{}: the resolver's stderr must stay visible: {found}",
            path.display()
        );
        assert!(
            !fallback.contains("--show-current"),
            "{}: a failed resolver must not fall back to the local branch: {found} || {fallback}",
            path.display()
        );
    }
}
