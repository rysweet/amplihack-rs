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
        // Crusty round 3: wrappers, `source`/`.`, stdin and interpreters.
        "env -u AMPLIHACK_MERGE_GUARD amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38",
        "exec /bin/bash -p amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38",
        "timeout -s KILL 600 bash -p amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38",
        "sudo -u ryan nohup bash -o pipefail amplifier-bundle/tools/autodrive_merge_gate.sh",
        "X=1 . amplifier-bundle/tools/autodrive_merge_gate.sh",
        "source amplifier-bundle/tools/autodrive_merge_gate.sh",
        "bash -s < amplifier-bundle/tools/autodrive_merge_gate.sh",
        "bash -- amplifier-bundle/tools/autodrive_merge_gate.sh",
        "cat amplifier-bundle/tools/autodrive_merge_gate.sh | bash",
        "find amplifier-bundle/tools -name '*.sh' -exec autodrive_merge_gate.sh {} \\;",
        "python3 -c \"import subprocess; subprocess.run(['amplifier-bundle/tools/autodrive_merge_gate.sh', '--pr', '38'])\"",
        "bash -n amplifier-bundle/tools/autodrive_merge_gate.sh",
    ] {
        let block = check(cmd).unwrap_or_else(|| panic!("not refused: {cmd}"));
        let message = block["message"].as_str().unwrap();
        assert!(message.contains("merge gate"), "{message}");
    }
}

#[test]
fn naming_the_gate_without_running_it_passes() {
    // Crusty round 3: only a run of the gate is refused. Reading, diffing,
    // staging and searching it from the shell are routine work on this PR
    // and its follow-ups, and an agent has no other tool for the git ones.
    let gate = "amplifier-bundle/tools/autodrive_merge_gate.sh";
    for cmd in [
        format!("cat {gate}"),
        format!("git show HEAD:{gate}"),
        format!("git -C /tmp/wt show HEAD:{gate}"),
        format!("git diff origin/main -- {gate}"),
        format!("git add {gate}"),
        format!("git checkout -- {gate}"),
        format!("git log -p -- {gate}"),
        "grep -n autodrive_merge_gate.sh amplifier-bundle/recipes/autodrive-merge-loop.yaml".into(),
        format!("rg -n 'mergePullRequest' {gate}"),
        format!("shellcheck {gate}"),
        format!("git show HEAD:{gate} > /tmp/autodrive_merge_gate.sh"),
        format!("env LC_ALL=C grep -c gh {gate}"),
    ] {
        assert!(
            !blocked(&cmd),
            "refused a command that does not run the gate: {cmd}"
        );
    }
    // The documented cost: with an interpreter as a program in the same
    // command, the gate's name may be code, so the compound is refused and
    // has to be split (docs/reference/auto-drive-to-merge.md).
    assert!(blocked(&format!("bash test.sh && git add {gate}")));
    assert!(!blocked("bash test.sh"));
}

#[test]
fn interpreters_count_only_as_the_program_being_run() {
    // Crusty round 3: a flag value or argument named after a shell is not an
    // interpreter running the quoted text.
    for cmd in [
        "rg 'gh pr merge' --type sh",
        "rg 'gh pr merge' -g '*.sh' -t sh docs/",
        "grep -rn 'gh pr merge' --include=*.sh . && which node",
        "git log --grep 'gh pr merge' -- bash",
    ] {
        assert!(!blocked(cmd), "refused a non-merge command: {cmd}");
    }
    // In command position, behind wrappers or `find -exec`, it still counts.
    for cmd in [
        "env FOO=1 python3 -c \"import os; os.system('gh pr merge 38')\"",
        "timeout 30 bash <<<'gh pr merge 38'",
        "echo 'gh pr merge 38' | xargs -I{} sh -s",
        "echo 'gh pr merge 38' | sudo -u ryan bash",
        "find . -name x -exec python3 -c \"import os; os.system('gh pr merge 38')\" \\;",
    ] {
        assert!(blocked(cmd), "not refused: {cmd}");
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

#[test]
fn heredoc_bodies_are_data_when_no_interpreter_reads_them() {
    // Crusty round 4: the usual way agents write commit messages and PR
    // bodies. Each of these was refused when heredoc lines were read as
    // commands.
    for cmd in [
        "git commit -F - <<'EOF'\nfix: refuse gh pr merge outside the gate\nEOF",
        "git commit -m \"$(cat <<'EOF'\nfix: refuse gh pr merge outside the gate\nEOF\n)\"",
        "gh pr create --title t --body \"$(cat <<'EOF'\nNever run gh pr merge yourself.\nEOF\n)\"",
        "cat > docs/x.md <<'EOF'\nUse curl -X PUT https://api.github.com/repos/o/r/pulls/1/merge\nEOF",
        // Prose apostrophes and parentheses inside `$(…)` do not unbalance it.
        "git commit -m \"$(cat <<'EOF'\nDon't (ever) run gh pr merge; it's the gate's job.\n\nCo-Authored-By: x <x@y>\nEOF\n)\"",
        // Unquoted, `<<-` with tab-indented delimiter, and a later command.
        "cat <<EOF > notes.md\ngh pr merge 38\nmutation { mergePullRequest }\nEOF\ngit add notes.md",
        "cat <<-EOF\n\tgh pr merge 38\n\tEOF",
        "cat <<\"EOF\" && git status\ngh pr merge 38\nEOF",
        "cat <<\\EOF\ngh pr merge 38\nEOF",
        // Two heredocs on one line.
        "paste - /dev/fd/3 <<A 3<<B\ngh pr merge 1\nA\ngh pr merge 2\nB",
    ] {
        assert!(!blocked(cmd), "refused a heredoc of prose: {cmd:?}");
    }
}

#[test]
fn heredoc_bodies_are_code_when_an_interpreter_reads_them() {
    for cmd in [
        "bash <<'EOF'\ngh pr merge 38\nEOF",
        "python3 - <<'EOF'\nimport os\nos.system('gh pr merge 38')\nEOF",
        "sudo -u ryan sh <<EOF\ngh -R o/r pr merge 38\nEOF",
        "cat <<'EOF' | bash\ngh pr merge 38\nEOF",
        "bash <<-EOF\n\tgh api -X PUT repos/o/r/pulls/38/merge\n\tEOF",
        "bash -s <<'EOF'\nbash -p amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38\nEOF",
        "x=\"$(python3 <<'EOF'\nimport subprocess; subprocess.run(['gh','pr','merge','38'])\nEOF\n)\"",
    ] {
        assert!(blocked(cmd), "not refused: {cmd:?}");
    }
}

#[test]
fn heredoc_bodies_are_code_unless_a_known_data_consumer_reads_them() {
    // Crusty round 5: d21721f3 read a body as data unless `is_interpreter`
    // named its program, so every one of these passed. The rule is now deny
    // by default; each carve-out above has its refused neighbour here.
    for cmd in [
        "source /dev/stdin <<'EOF'\ngh pr merge 38\nEOF",
        ". /dev/stdin <<'EOF'\ngh pr merge 38\nEOF",
        "eval \"$(cat <<'EOF'\ngh pr merge 38\nEOF\n)\"",
        "parallel <<'EOF'\ngh pr merge 38\nEOF",
        "at now <<'EOF'\ngh pr merge 38\nEOF",
        "batch <<'EOF'\ngh pr merge 38\nEOF",
        "ssh localhost <<'EOF'\ngh pr merge 38\nEOF",
        "bash -c \"$(cat <<'EOF'\ngh pr merge 38\nEOF\n)\"",
        // A program nobody listed, and one added to a toolchain later.
        "some-new-runner <<'EOF'\ngh pr merge 38\nEOF",
        "script -q /dev/null <<'EOF'\ngh pr merge 38\nEOF",
        "expect <<'EOF'\nspawn gh pr merge 38\nEOF",
        // A wrapper that leaves no command to find.
        "env -S 'sh' <<'EOF'\ngh pr merge 38\nEOF",
        // `$(cat <<…)` feeding a program that runs its argument.
        "ssh localhost \"$(cat <<'EOF'\ngh pr merge 38\nEOF\n)\"",
        "parallel ::: \"$(cat <<'EOF'\ngh pr merge 38\nEOF\n)\"",
        // Listed consumers in a mode that runs or sends the body.
        "gh api graphql -F query=@- <<'EOF'\nmutation { mergePullRequest(input: {}) { clientMutationId } }\nEOF",
        "git -c alias.m='!sh' m <<'EOF'\ngh pr merge 38\nEOF",
        "sed -f - x <<'EOF'\ne gh pr merge 38\nEOF",
        // A data consumer in the same call as a program off the list.
        "cargo fmt && git commit -F - <<'EOF'\nfix: gh pr merge 38\nEOF",
        // Here-strings to `source`/`eval` run too.
        "source /dev/stdin <<<'gh pr merge 38'",
    ] {
        assert!(blocked(cmd), "not refused: {cmd:?}");
    }
}

#[test]
fn heredocs_hide_neither_substitutions_nor_the_commands_after_them() {
    for cmd in [
        // An unquoted body runs its `$(…)` and backticks.
        "cat <<EOF\n$(gh pr merge 38)\nEOF",
        "cat <<EOF\n`gh pr merge 38`\nEOF",
        "git commit -F - <<EOF\nmerged: $(gh pr merge 38 --squash)\nEOF",
        // The command after the delimiter line is a command again.
        "cat <<'EOF'\nprose\nEOF\ngh pr merge 38",
        "git commit -m \"$(cat <<'EOF'\nprose\nEOF\n)\" && gh pr merge 38",
        // No delimiter line closes it: read as commands, as before.
        "cat <<'EOF'\ngh pr merge 38",
        // A shift is not a heredoc; the next line still counts.
        "echo $((1<<2))\ngh pr merge 38\n2",
        // A here-string is not a heredoc either.
        "bash <<<'gh pr merge 38'",
    ] {
        assert!(blocked(cmd), "not refused: {cmd:?}");
    }
    // A quoted delimiter turns expansion off: the body is prose.
    assert!(!blocked("cat <<'EOF'\n$(gh pr merge 38)\nEOF"));
}
