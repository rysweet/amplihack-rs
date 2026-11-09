//! #1969 static guards on the recipe text (R1, R2, R3, R9, R10, R12).
//!
//! The behaviour is covered by `issue_1969_step04.rs` and
//! `issue_1969_publish.rs`; these checks keep a later edit from quietly
//! reintroducing an implicit upstream, a bare push or pull, or the
//! `no-upstream` skip.

use std::process::Command;

use regex::Regex;

use super::fixture::*;

/// The step-15 command and the 16b prompt, as the recipe holds them.
fn step15() -> String {
    step_text("workflow-publish", "step-15-commit-push", "command")
}

fn step16b_block() -> String {
    let prompt = step_text("workflow-publish", "step-16b-outside-in-fix-loop", "prompt");
    prompt
        .split("```bash")
        .nth(1)
        .and_then(|rest| rest.split("```").next())
        .expect("step 16b's prompt has a bash code block")
        .to_owned()
}

/// Lines that are neither `#` comments nor `echo` statements.
fn executable_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with('#') && !t.starts_with("echo ")
        })
        .collect()
}

/// R1: every `git worktree add -b` names its tracking mode. The create modes
/// use `--no-track`; the `track` mode and the #1762 alias keep `--track`.
#[test]
fn new_branches_are_created_with_no_track() {
    let worktree = read(&recipe_path("workflow-worktree"));
    let consensus = read(&recipe_path("consensus-issue-worktree"));
    let add_b = Regex::new(r"git\s+worktree\s+add\b[^\n]*\s-b\s").unwrap();
    for (name, text) in [
        ("workflow-worktree", &worktree),
        ("consensus-issue-worktree", &consensus),
    ] {
        let adds: Vec<&str> = code_lines(text).filter(|l| add_b.is_match(l)).collect();
        assert!(!adds.is_empty(), "{name}: has `git worktree add -b`");
        for line in adds {
            assert!(
                line.contains("--no-track") || line.contains("--track "),
                "{name}: `git worktree add -b` must say --no-track or --track, never rely on branch.autoSetupMerge: {line}"
            );
        }
    }

    let step4 = step_text("workflow-worktree", "step-04-setup-worktree", "command");
    let create = code_lines(&step4)
        .find(|l| l.trim_start().starts_with("create)"))
        .expect("wt_add_idempotent has a create mode");
    assert!(
        create.contains("git worktree add --no-track -b"),
        "create mode cuts the branch with --no-track: {create}"
    );
    let track = code_lines(&step4)
        .find(|l| l.trim_start().starts_with("track)"))
        .expect("wt_add_idempotent has a track mode");
    assert!(
        track.contains("--track -b") && !track.contains("--no-track"),
        "track mode is unchanged: {track}"
    );
    assert!(
        code_lines(&step4).any(
            |l| l.contains(r#"git worktree add --track -b "$LOCAL_BRANCH""#)
                && l.contains(r#""origin/$BRANCH_NAME""#)
        ),
        "the #1762 alias still tracks origin/<B>"
    );

    let base_create = code_lines(&consensus)
        .find(|l| l.contains("git worktree add") && l.contains(r#""$BASE_REF""#))
        .expect("consensus-issue-worktree creates from $BASE_REF");
    assert!(
        base_create.contains("--no-track -b"),
        "consensus mirror cuts the branch with --no-track: {base_create}"
    );
}

/// R2, R12: in `workflow-publish.yaml`, after `#` and `echo` lines are
/// dropped, every `git push` is `git push -u origin "HEAD:refs/heads/…` and
/// every `git pull` is `git pull --rebase origin "…`.
#[test]
fn publish_pushes_and_pulls_name_origin_and_refspec() {
    let text = read(&recipe_path("workflow-publish"));
    let push = Regex::new(r"git\s+push\b").unwrap();
    let pull = Regex::new(r"git\s+pull\b").unwrap();
    let mut pushes = 0;
    for line in executable_lines(&text) {
        for m in push.find_iter(line) {
            pushes += 1;
            assert!(
                line[m.start()..].starts_with(r#"git push -u origin "HEAD:refs/heads/"#),
                "bare or implicit push in workflow-publish.yaml: {line}"
            );
        }
        for m in pull.find_iter(line) {
            assert!(
                line[m.start()..].starts_with(r#"git pull --rebase origin ""#),
                "pull without an explicit source in workflow-publish.yaml: {line}"
            );
        }
        for flag in [
            "--force",
            "--force-with-lease",
            "--no-verify",
            "--mirror",
            "--delete",
        ] {
            assert!(
                !line.contains(flag),
                "workflow-publish.yaml must not use {flag}: {line}"
            );
        }
    }
    assert!(pushes >= 2, "step 15 and step 16b both push");
}

/// R2, R3, R4, D5: step 15 pushes only to `refs/heads/$TARGET`, never reads
/// `@{u}` in code, has no `no-upstream` skip, fetches after the commit and
/// before the push, and verifies `$TARGET`.
#[test]
fn step15_text_has_explicit_target_and_no_upstream_skip() {
    let text = step15();
    assert!(
        !text.contains("no-upstream"),
        "the `no-upstream` skip is gone from step 15 (R3)"
    );
    let lines = executable_lines(&text);
    for line in &lines {
        assert!(
            !line.contains("@{u}") && !line.contains("@{upstream}"),
            "step 15 never reads @{{u}} (R4): {line}"
        );
    }
    let blanking = Regex::new(r#"\|\|\s*(REMOTE_BRANCH|TARGET)="""#).unwrap();
    assert!(
        lines.iter().all(|l| !blanking.is_match(l)),
        "the push target is never blanked (D5.1)"
    );
    assert!(
        lines.iter().any(
            |l| l.contains(r#"git push -u origin "HEAD:refs/heads/$TARGET""#) && l.contains("--")
        ),
        "step 15 pushes HEAD:refs/heads/$TARGET through hook_repair_run"
    );
    assert!(
        text.contains("hook_repair_run push"),
        "the push still goes through hook repair (#1671)"
    );
    let verify = Regex::new(r#"bash "\$\w+" origin "\$TARGET""#).unwrap();
    assert!(
        lines.iter().any(|l| verify.is_match(l)),
        "verify_pushed_ref.sh receives $TARGET (D5.5)"
    );
    assert!(
        text.contains(PUSH_PLAN),
        "step 15 classifies with {PUSH_PLAN}"
    );

    let pos = |needle: &str| {
        text.lines()
            .position(|l| !l.trim_start().starts_with('#') && l.contains(needle))
            .unwrap_or_else(|| panic!("step 15 has a code line with `{needle}`"))
    };
    let commit = pos("hook_repair_run commit");
    let fetch = pos("git fetch origin");
    let push = pos(r#"git push -u origin "HEAD:refs/heads/$TARGET""#);
    assert!(
        commit < fetch && fetch < push,
        "order: commit ({commit}), then fetch ({fetch}), then push ({push})"
    );
}

/// D6: step 16b pulls from and pushes to the resolved branch by name.
#[test]
fn step16b_pulls_and_pushes_resolved_branch() {
    let block = step16b_block();
    assert!(
        block.lines().any(|l| l.contains(
            r#"git pull --rebase origin "$rb" && git push -u origin "HEAD:refs/heads/$rb""#
        )),
        "step 16b's retry is explicit:\n{block}"
    );
    assert!(
        !block.contains(r#"rb="""#),
        "the resolved branch is never blanked:\n{block}"
    );
    assert!(
        block.contains("workflow_remote_branch.sh"),
        "the branch comes from the resolver:\n{block}"
    );
}

/// R9: the detection helper is in STEP_TOOLS, so the #1808 complete-tools
/// rule accounts for it.
#[test]
fn detection_helper_is_listed_in_step_tools() {
    let step4 = step_text("workflow-worktree", "step-04-setup-worktree", "command");
    let tools = code_lines(&step4)
        .find(|l| l.trim_start().starts_with("STEP_TOOLS="))
        .expect("step 4 defines STEP_TOOLS");
    assert!(
        tools.split(['"', ' ']).any(|w| w == DETECT),
        "{DETECT} is in STEP_TOOLS: {tools}"
    );
    assert!(
        code_lines(&step4).any(|l| l.contains(DETECT) && l.contains("TOOLS_DIR")),
        "step 4 runs the helper from TOOLS_DIR"
    );
}

/// Crusty round 1: the default-branch rule has one copy,
/// `workflow_default_branch.sh`. Each caller runs it and none reads
/// origin/HEAD, names main/master or guesses develop itself; step 4 lists
/// the helper in STEP_TOOLS so the #1808 complete-tools rule accounts for it.
#[test]
fn default_branch_rule_has_one_copy() {
    let step4 = step_text("workflow-worktree", "step-04-setup-worktree", "command");
    let tools = code_lines(&step4)
        .find(|l| l.trim_start().starts_with("STEP_TOOLS="))
        .expect("step 4 defines STEP_TOOLS");
    assert!(
        tools.split(['"', ' ']).any(|w| w == DEFAULT_RULE),
        "{DEFAULT_RULE} is in STEP_TOOLS: {tools}"
    );
    let callers = [
        (DETECT.to_owned(), read(&tool_path(DETECT))),
        (PLANNER.to_owned(), read(&tool_path(PLANNER))),
        (RESOLVER.to_owned(), read(&tool_path(RESOLVER))),
        ("workflow-publish.yaml step 15".to_owned(), step15()),
    ];
    let inline =
        Regex::new(r#"refs/remotes/origin/HEAD|main\|master|"develop"|ls-remote"#).unwrap();
    for (name, text) in &callers {
        assert!(
            code_lines(text).any(|l| l.contains(DEFAULT_RULE)),
            "{name} runs {DEFAULT_RULE}"
        );
        for line in code_lines(text) {
            assert!(
                !inline.is_match(line),
                "{name} carries no copy of the default-branch rule: {line}"
            );
        }
    }
    assert!(
        code_lines(&step15()).any(|l| l.contains(DEFAULT_RULE) && l.contains("--ask-origin")),
        "step 15 asks origin through the helper"
    );
    let status =
        Regex::new(r"(?:^|[\s;&|(]|\$\()git(?:\s+-C\s+\S+|\s+--no-optional-locks)*\s+status\b")
            .unwrap();
    for line in code_lines(&read(&tool_path(DEFAULT_RULE))) {
        assert!(
            !status.is_match(line),
            "{DEFAULT_RULE} never runs git status (fsmonitor): {line}"
        );
    }
}

/// R9: both recipes stay within the brick limit (`< 400`, at most 399).
#[test]
fn recipes_stay_under_brick_limit() {
    for name in ["workflow-worktree", "workflow-publish"] {
        let lines = read(&recipe_path(name)).lines().count();
        assert!(
            lines < 400,
            "{name}.yaml has {lines} lines; the limit is < 400"
        );
    }
}

/// R10: `_recipe_manifest.json` carries the first 16 hex digits of each
/// changed recipe's sha256.
#[test]
fn manifest_hashes_match_changed_recipes() {
    let manifest: serde_json::Value = serde_json::from_str(&read(
        &workspace_root().join("amplifier-bundle/recipes/_recipe_manifest.json"),
    ))
    .expect("_recipe_manifest.json is JSON");
    for name in [
        "workflow-worktree",
        "workflow-publish",
        "consensus-issue-worktree",
    ] {
        // `sha256sum <file> | cut -c1-16`, as the manifest is generated.
        let out = Command::new("sha256sum")
            .arg(recipe_path(name))
            .output()
            .expect("run sha256sum");
        let hex = String::from_utf8_lossy(&out.stdout).to_string();
        assert_eq!(
            manifest[name].as_str(),
            Some(&hex[..16]),
            "{name}: manifest hash is stale"
        );
    }
}

/// D2, SR-3, S7: the new helpers only read, never use `@{u}` or `git status`,
/// and use fully qualified remote refs.
#[test]
fn new_helpers_read_only_and_qualified() {
    let detect = read(&tool_path(DETECT));
    let plan = read(&tool_path(PUSH_PLAN));
    let status =
        Regex::new(r"(?:^|[\s;&|(]|\$\()git(?:\s+-C\s+\S+|\s+--no-optional-locks)*\s+status\b")
            .unwrap();
    for line in code_lines(&detect) {
        assert!(
            !status.is_match(line),
            "{DETECT} never runs git status (fsmonitor): {line}"
        );
    }
    for line in code_lines(&plan) {
        assert!(
            !line.contains("@{u}") && !line.contains("@{upstream}"),
            "{PUSH_PLAN} never reads @{{u}}: {line}"
        );
    }
    assert!(
        plan.contains("refs/remotes/origin/"),
        "{PUSH_PLAN} names refs/remotes/origin/<T>"
    );
    assert!(
        plan.contains("--remotes=origin"),
        "{PUSH_PLAN} counts a first push against every origin branch"
    );
}
