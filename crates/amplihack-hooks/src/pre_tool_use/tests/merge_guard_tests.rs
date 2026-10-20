//! Issue #1551: the hook refuses the direct merge forms by their program
//! word, and lets every command that only mentions them as data through.

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
fn refuses_gh_pr_merge_as_the_program_whatever_gh_path_finds() {
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
        "GH_TOKEN=x env -u GH_HOST PATH=/x gh pr merge 38",
        "env gh pr merge 38",
        "nice gh pr merge 38",
        "exec gh pr merge 38",
        "command gh pr merge 38",
        "timeout 60 gh pr merge 38",
        "nohup nice -n 5 gh pr merge 38",
        "cd repo && gh pr merge 38",
        "gh pr view 38; gh pr merge 38",
        "gh pr view 38\ngh pr merge 38",
        "true || gh pr merge 38",
        "(gh pr merge 38)",
        "{ gh pr merge 38; }",
        "if true; then gh pr merge 38; fi",
        "'gh' pr merge 38",
        "g\\h pr merge 38",
        "cat <<'EOF'\nprose\nEOF\ngh pr merge 38",
    ] {
        assert!(blocked(cmd), "not refused: {cmd}");
    }
}

#[test]
fn refuses_gh_api_on_a_merge_endpoint() {
    for cmd in [
        "gh api -X PUT repos/o/r/pulls/38/merge",
        "gh api --method PUT /repos/o/r/pulls/38/merge/",
        "gh api -X PUT 'https://api.github.com/repos/o/r/pulls/38/merge?x=1'",
        "gh api -X POST repos/o/r/merges -f base=main -f head=x",
        "gh api -X POST repositories/212613049/merges",
        "gh api -X PUT REPOS/O/R/PULLS/38/MERGE",
        "gh -R o/r api -H 'Accept: x' repos/o/r/pulls/38/merge -X PUT",
    ] {
        assert!(blocked(cmd), "not refused: {cmd}");
    }
}

/// Running the merge gate is not a form this hook reads (crusty round 15):
/// the shim refuses the merge of a gate run from an agent shell, since its
/// environment carries `BASH_ENV`, and a cleared environment is the
/// non-goal. So gate runs and inspections pass the hook alike, with no shell
/// option parsing to get wrong.
#[test]
fn leaves_merge_gate_runs_to_the_shim() {
    for cmd in [
        "./amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38",
        "bash -p amplifier-bundle/tools/autodrive_merge_gate.sh --pr 38",
        "bash -n amplifier-bundle/tools/autodrive_merge_gate.sh",
        "bash -s < amplifier-bundle/tools/autodrive_merge_gate.sh",
        "zsh --no-rcs amplifier-bundle/tools/autodrive_merge_gate.sh",
        "command -v amplifier-bundle/tools/autodrive_merge_gate.sh",
        "bash -c 'wc -l amplifier-bundle/tools/autodrive_merge_gate.sh'",
    ] {
        assert!(!blocked(cmd), "the hook read a gate run: {cmd}");
    }
}

/// crusty round 8: commands that mention the merge words as data pass with
/// the guard on, alone and in every compound, including after a segment that
/// runs an interpreter.
#[test]
fn data_mentions_pass_in_every_context() {
    let data = [
        "grep -rn 'gh pr merge' .",
        "rg 'pulls/1/merge'",
        "echo gh pr merge 38 > notes",
        "echo gh pr merge 38 >> notes.md",
        "cat <<'EOF'\nrun gh pr merge 38 here\ngh api -X PUT repos/o/r/pulls/38/merge\nEOF",
        "cat <<-EOF\n\tgh pr merge 38\n\tEOF",
        "git add amplifier-bundle/tools/autodrive_merge_gate.sh",
        "git commit -m 'refuse gh pr merge outside the gate'",
        "git commit -m \"refuse gh pr merge outside the gate\"",
        "git commit -F - <<'EOF'\ngh pr merge is refused\nEOF",
        "git show HEAD:amplifier-bundle/tools/autodrive_merge_gate.sh",
        "cat amplifier-bundle/tools/autodrive_merge_gate.sh",
        "gh pr create --title t --body 'never gh pr merge; the gate merges'",
        "gh pr create --body \"$(cat <<'EOF'\ngh pr merge 38\nEOF\n)\"",
        // Claude Code's commit form, with an odd `"` and a `)` in the body
        // and a body line that starts with the merge words (round 9).
        "git commit -m \"$(cat <<'EOF'\nfix: handle the 5\" case\n\ngh pr merge is now refused\nEOF\n)\"",
        "git commit -m \"$(cat <<'EOF'\nfix: step a) and the 5\" case\ngh pr merge 38\nEOF\n)\"",
        // A quote that spans two body lines.
        "gh pr create --body \"$(cat <<'EOF'\nsay \"stop\ngh pr merge 38\" twice\ngh pr merge 38\nEOF\n)\"",
        "echo \"$(printf '%s' \"a) gh pr merge 38\")\"",
        "# gh pr merge 38",
        "cat x <<< 'gh pr merge 38'",
    ];
    let before = [
        "",
        "bash test.sh && ",
        "cargo test 2>&1 | awk '{print}'; ",
        "python3 -c 'print(1)'; ",
        "sh -c 'true' || ",
    ];
    for prefix in before {
        for cmd in data {
            let cmd = format!("{prefix}{cmd}");
            assert!(!blocked(&cmd), "refused a data mention: {cmd}");
        }
    }
}

#[test]
fn lets_every_non_merge_gh_call_through() {
    for cmd in [
        "gh pr view 38 --json state,mergeable,mergeStateStatus",
        "gh pr checks 38",
        "gh api repos/o/r/pulls/38/files",
        "gh api repos/o/r/pulls/38/merge-something",
        "gh api repos/o/r/branches/merge-train",
        "gh api repos/o/r/issues -f body='see repos/o/r/pulls/1/merge'",
        "gh api graphql -f query='query { viewer { login } }'",
        "gh search prs merge",
        "git merge origin/main",
        "cargo test merge_guard",
        "",
    ] {
        assert!(!blocked(cmd), "refused a non-merge command: {cmd}");
    }
}

#[test]
fn the_block_names_the_form_without_echoing_the_command() {
    let block = check("gh pr merge 38 --body SECRET").unwrap();
    let message = block["message"].as_str().unwrap();
    assert!(message.contains("gh pr merge"), "{message}");
    assert!(message.contains("#1551"), "{message}");
    assert!(!message.contains("SECRET"), "{message}");
}

#[test]
fn unbalanced_input_does_not_panic() {
    for cmd in [
        "'gh pr merge",
        "\"gh",
        "$(gh pr",
        "`gh",
        "cat <<",
        "\\",
        "gh pr",
    ] {
        let _ = check(cmd);
    }
}

#[test]
fn a_merge_after_a_quoted_command_substitution_is_still_refused() {
    for cmd in [
        "git commit -m \"$(cat <<'EOF'\nthe 5\" case\nEOF\n)\" && gh pr merge 38",
        "git commit -m \"$(cat <<'EOF'\nthe 5\" case\nEOF\n)\"\ngh pr merge 38",
        "echo \"$(date)\"; gh pr merge 38",
    ] {
        assert!(blocked(cmd), "let a merge through: {cmd}");
    }
}
