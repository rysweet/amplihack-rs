//! Issue #1551, crusty round 1 on PR #1644: the merge check in the
//! pre-tool-use hook holds whatever `gh` the agent shell's PATH resolves.

use super::*;

fn blocked(command: &str) -> bool {
    check(command).is_some()
}

#[test]
fn guard_is_active_for_any_non_empty_marker_only() {
    assert!(guard_active(Some("1")));
    assert!(guard_active(Some("0")));
    assert!(!guard_active(Some("")));
    assert!(!guard_active(None));
}

#[test]
fn refuses_gh_pr_merge_in_every_spelling_the_agent_can_type() {
    for cmd in [
        "gh pr merge 38 --squash",
        "gh -R o/r pr merge 38",
        "gh --repo o/r pr merge 38",
        "gh pr -R o/r merge 38",
        // A `gh` ahead of the shim, as a profile that prepends ~/.local/bin
        // or Homebrew's directory would make it, or typed by path.
        "/home/u/.local/bin/gh pr merge 38",
        "/opt/homebrew/bin/gh pr merge 38 --admin",
        "PATH=/home/u/.npm-global/bin:$PATH gh pr merge 38",
        "env PATH=/x gh pr merge 38",
        "command gh pr merge 38",
        "cd repo && gh pr merge 38",
        "gh pr view 38; gh pr merge 38",
        "echo 38 | xargs gh pr merge",
        "bash -lc 'gh pr merge 38'",
        "sh -c \"gh pr merge 38\"",
        "eval gh pr merge 38",
        "echo \"$(gh pr merge 38)\"",
        "x=`gh pr merge 38`",
        "(gh pr merge 38)",
        "g\\h pr merge 38",
        "'gh' pr merge 38",
    ] {
        assert!(blocked(cmd), "not refused: {cmd}");
    }
}

#[test]
fn refuses_rest_and_graphql_merges_through_gh_api_and_http_clients() {
    for cmd in [
        "gh api -X PUT repos/o/r/pulls/38/merge",
        "gh api --method PUT /repos/o/r/pulls/38/merge/",
        "gh api -X PUT 'https://api.github.com/repos/o/r/pulls/38/merge?x=1'",
        "gh api -X POST repos/o/r/merges -f base=main -f head=x",
        "gh api -X POST repositories/212613049/merges",
        "gh api -X PUT REPOS/O/R/PULLS/38/MERGE",
        "gh api graphql -f query='mutation { mergePullRequest(input:{pullRequestId:\"x\"}) { clientMutationId } }'",
        "gh api graphql -f query='mutation { enablePullRequestAutoMerge(input:{}) { clientMutationId } }'",
        "gh api graphql -F query=@q.graphql",
        "gh api graphql --input q.json",
        "curl -X PUT -H \"Authorization: token $(gh auth token)\" https://api.github.com/repos/o/r/pulls/38/merge",
        "wget --method=PUT https://api.github.com/repos/o/r/pulls/38/merge",
        "curl https://api.github.com/graphql -d '{\"query\":\"mutation{mergeBranch(input:{}){clientMutationId}}\"}'",
    ] {
        assert!(blocked(cmd), "not refused: {cmd}");
    }
}

#[test]
fn lets_every_non_merge_command_through() {
    for cmd in [
        "gh pr view 38 --json state,mergeable,mergeStateStatus",
        "gh pr checks 38",
        "gh pr create --title t --body 'leave mergePullRequest to the gate; never gh pr merge'",
        "gh api repos/o/r/pulls/38/files",
        "gh api repos/o/r/pulls/38/merge-something",
        "gh api repos/o/r/branches/merge-train",
        "gh api graphql -f query='query { repository(owner:\"o\",name:\"r\") { pullRequest(number:38) { mergeable } } }'",
        "git commit -m 'refuse gh pr merge outside the gate'",
        "git merge origin/main",
        "grep -rn 'mergePullRequest' crates/",
        "rg 'gh pr merge' docs/",
        "cargo test merge_guard",
        "curl https://api.github.com/repos/o/r/pulls/38",
        "",
    ] {
        assert!(!blocked(cmd), "refused a non-merge command: {cmd}");
    }
}

#[test]
fn refuses_an_agent_running_the_merge_gate_in_any_spelling() {
    // Crusty round 2: the gate runs from a recipe bash step only, and trusts
    // Crusty and QA record files an agent could write.
    for cmd in [
        "bash -p \"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge_gate.sh\" --pr 38",
        "/bin/bash -p ~/.amplihack/amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38",
        "./amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38 --round-record r.json",
        "cd amplifier-bundle/tools && bash autodrive_merge_gate.sh --pr 38",
        "sh -c '/bin/bash -p ${AMPLIHACK_HOME}/amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38'",
        "echo \"$(bash -p autodrive_merge_gate.sh --pr 38)\"",
    ] {
        let block = check(cmd).unwrap_or_else(|| panic!("not refused: {cmd}"));
        let message = block["message"].as_str().unwrap();
        assert!(message.contains("merge gate"), "{message}");
    }
}

#[test]
fn refuses_merge_text_fed_to_an_interpreter_by_pipe_here_string_or_inline_code() {
    for cmd in [
        "echo 'gh pr merge 38' | bash",
        "printf '%s\\n' \"gh pr merge 38 --squash\" | sh",
        "bash -s <<<'gh pr merge 38'",
        "bash <<< \"gh -R o/r pr merge 38\"",
        "python3 -c \"import os; os.system('gh pr merge 38')\"",
        "python3 -c \"import subprocess; subprocess.run(['gh', 'api', '-X', 'PUT', 'repos/o/r/pulls/38/merge'])\"",
        "python -c \"import requests; requests.put('https://api.github.com/repos/o/r/pulls/38/merge')\"",
        "perl -e 'system(\"gh pr merge 38\")'",
        "python3 -c \"import subprocess; subprocess.run(['gh','pr','merge','38'])\"",
        "ruby -e 'system(\"gh\", \"pr\", \"merge\", \"38\")'",
        "node -e \"require('child_process').execSync('gh pr merge 38')\"",
        "awk 'BEGIN { system(\"gh pr merge 38\") }'",
        "echo 'mutation { mergePullRequest(input:{}) { clientMutationId } }' | python3 q.py",
        "echo 'echo \"gh pr merge 38\"' | bash | bash",
    ] {
        assert!(blocked(cmd), "not refused: {cmd}");
    }
}

#[test]
fn quoted_merge_text_passes_when_no_interpreter_runs_it() {
    // Without an interpreter in the command, quoted text is prose: commit
    // messages, PR bodies and searches.
    for cmd in [
        "git commit -m 'refuse gh pr merge outside the gate'",
        "gh pr create --title t --body 'never gh pr merge; the gate merges'",
        "rg 'gh pr merge' docs/",
        "echo 'gh pr merge 38' > notes.txt",
        "python3 -c 'print(1)'",
        "bash -n script.sh",
    ] {
        assert!(!blocked(cmd), "refused a non-merge command: {cmd}");
    }
}

#[test]
fn the_block_names_the_form_and_the_gate_without_echoing_the_command() {
    let block = check("gh pr merge 38 --body SECRET_1551").expect("refused");
    assert_eq!(block["block"], true);
    let message = block["message"].as_str().unwrap();
    assert!(message.contains("gh pr merge"), "{message}");
    assert!(message.contains("autodrive_merge_gate.sh"), "{message}");
    assert!(message.contains("#1551"), "{message}");
    assert!(message.contains("PATH"), "{message}");
    assert!(!message.contains("SECRET_1551"), "{message}");
}

#[test]
fn nesting_is_followed_to_the_bound_and_no_further() {
    // `$(…)` nesting grows linearly, unlike re-quoting `bash -c` bodies.
    let nest = |levels: usize, inner: &str| {
        (0..levels).fold(inner.to_string(), |cmd, _| format!("echo $({cmd})"))
    };
    assert!(blocked(&nest(3, "gh pr merge 38")));
    assert!(blocked(&nest(MAX_NESTING, "gh pr merge 38")));
    // Deeper than the bound is left to the shim, and the check still ends.
    assert!(!blocked(&nest(MAX_NESTING + 2, "gh pr merge 38")));
    assert!(!blocked(&nest(5_000, "gh pr view 1")));
    assert!(!blocked("$($($($("));
    assert!(!blocked("`"));
    assert!(!blocked("\\\\"));
}
