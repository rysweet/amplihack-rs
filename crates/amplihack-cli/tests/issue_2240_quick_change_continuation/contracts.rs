use serde_json::json;

use super::fixture::{BRANCH, Fixture, recipe, stderr, step, success};

#[test]
fn child_context_forwards_branch_and_policy_but_clears_delivery_targets() {
    let s = step("quick-change", "quick-change-worktree");
    assert_eq!(
        s["context"]["existing_branch"].as_str(),
        Some("{{existing_branch}}")
    );
    assert_eq!(
        s["context"]["worktree_policy"].as_str(),
        Some("quick-change")
    );
    for key in ["pr_number", "base_branch", "forbidden_base_branches"] {
        assert_eq!(
            s["context"][key].as_str(),
            Some(""),
            "inherited {key} must be cleared"
        );
    }
    assert!(
        s["context"].get("strict_delivery").is_none(),
        "strict_delivery stays inherited"
    );
    let handover = step("quick-change", "quick-change-handover");
    assert_eq!(
        handover["context"]["existing_branch"].as_str(),
        Some("{{worktree_setup.branch_name}}")
    );
    assert_eq!(
        handover["context"]["worktree_policy"].as_str(),
        Some("quick-change")
    );
    assert_ne!(
        recipe("workflow-worktree")["context"]["worktree_policy"].as_str(),
        Some("quick-change"),
        "other workflows keep legacy selection by default"
    );
}

#[test]
fn builder_result_is_parsed_as_structured_data() {
    let builder = step("quick-change", "quick-change-implement");
    assert_eq!(builder["parse_json"].as_bool(), Some(true));
    let prompt = builder["prompt"].as_str().unwrap();
    assert!(prompt.contains("commit_subject") && prompt.contains("summary"));
}

#[test]
fn branch_guard_accepts_supplied_non_quick_branch_but_refuses_substitution() {
    let mut f = Fixture::new("");
    success(&f.select());
    f.ctx.set("existing_branch", json!(BRANCH));
    f.git(&f.selected(), &["checkout", "-q", "-b", BRANCH]);
    assert_eq!(success(&f.common("qc_branch")), BRANCH);
    f.git(&f.selected(), &["checkout", "-q", "-b", "quick/substitute"]);
    let out = f.common("qc_branch");
    assert!(
        !out.status.success(),
        "a quick/ alias must not satisfy existing_branch"
    );
    assert!(stderr(&out).contains(BRANCH), "{}", stderr(&out));
}

#[test]
fn ordinary_guard_still_requires_quick_prefix() {
    let mut f = Fixture::new("");
    success(&f.select());
    f.git(&f.selected(), &["checkout", "-q", "-b", BRANCH]);
    assert!(!f.common("qc_branch").status.success());
}

#[test]
fn protected_branches_and_custom_origin_default_are_refused_before_setup_pushes() {
    for branch in ["main", "master", "develop", "trunk", "release-default"] {
        let mut f = Fixture::new(branch);
        if branch != "main" {
            f.git(&f.caller, &["branch", branch]);
            f.git(&f.caller, &["push", "-q", "origin", branch]);
        }
        if branch == "release-default" {
            f.git(
                &f.origin,
                &["symbolic-ref", "HEAD", "refs/heads/release-default"],
            );
            f.git(&f.caller, &["remote", "set-head", "origin", branch]);
        }
        let refs = f.git(&f.origin, &["show-ref"]);
        std::fs::write(f.home.join("hook-calls"), "").unwrap();
        let out = f.select_direct();
        assert!(
            !out.status.success(),
            "protected branch {branch} was selected"
        );
        assert!(stderr(&out).contains(branch), "{}", stderr(&out));
        assert_eq!(f.git(&f.origin, &["show-ref"]), refs);
        assert!(!f.read_home("hook-calls").contains("pre-push"));
    }
}

#[test]
fn invalid_branch_is_rejected_before_any_origin_lookup() {
    for branch in ["-bad", "feat/../bad", "feat/bad\nref", "@{-1}"] {
        let mut f = Fixture::new(branch);
        let out = f.select_direct();
        assert!(!out.status.success(), "accepted {branch:?}");
        let trace = f.read_home("git-trace");
        assert!(
            !trace.contains("ls-remote")
                && !trace.contains("git fetch")
                && !trace.contains("git push"),
            "origin contacted before ref validation: {trace}"
        );
    }
}

#[test]
fn remote_default_is_refused_even_with_stale_or_missing_origin_head() {
    for stale in [false, true] {
        let mut f = Fixture::new(BRANCH);
        f.branch(true);
        f.git(
            &f.origin,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{BRANCH}")],
        );
        if !stale {
            f.git(
                &f.caller,
                &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
            );
        }
        let refs = f.git(&f.origin, &["show-ref"]);
        let out = f.select_direct();
        assert!(!out.status.success());
        assert!(stderr(&out).contains(BRANCH), "{}", stderr(&out));
        assert_eq!(f.git(&f.origin, &["show-ref"]), refs);
        assert!(!f.read_home("git-trace").contains("built-in: git fetch"));
    }
}

#[test]
fn legacy_selection_retains_its_caller_alias_behaviour() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.ctx.set("worktree_policy", json!(""));
    success(&f.run("workflow-worktree", "step-04-setup-worktree"));
    assert_ne!(f.selected(), f.caller.canonicalize().unwrap());
    assert_ne!(f.git(&f.selected(), &["branch", "--show-current"]), BRANCH);
    assert_eq!(f.git(&f.caller, &["branch", "--show-current"]), BRANCH);
}

#[test]
fn pinned_head_excludes_prior_commits_but_counts_staged_unstaged_and_untracked_work() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true); // More than the 150-line cap already exists at HEAD.
    f.write(&f.caller, "notes.md", "staged\n");
    f.git(&f.caller, &["add", "notes.md"]);
    f.write(&f.caller, "notes.md", "staged\nunstaged\n");
    f.write(&f.caller, "untracked.md", "untracked\n");
    let head = f.git(&f.caller, &["rev-parse", "HEAD"]);
    success(&f.select());
    assert_eq!(success(&f.qc("record-base")), head);
    f.builder(super::fixture::SUBJECT);
    assert_eq!(
        success(&f.qc("diff-limits")),
        "proceed",
        "old branch work is excluded"
    );
    let measured = std::fs::read_to_string(f.state().join("diff-limits")).unwrap();
    let stats: serde_json::Value = serde_json::from_str(measured.lines().nth(1).unwrap()).unwrap();
    assert_eq!(
        stats["files"], 3,
        "staged, unstaged and untracked paths count"
    );
    let names = f.git(&f.selected(), &["diff", "--name-only", &head]);
    for path in ["notes.md", "untracked.md", "README.md"] {
        assert!(
            names.lines().any(|name| name == path),
            "missing {path}: {names}"
        );
    }
    assert!(
        !names.contains("old-work.md"),
        "historical diff counted: {names}"
    );
    assert_eq!(f.ctx.get("quick_change_base").unwrap(), &json!(head));
}

#[test]
fn pinned_base_must_match_runner_output_and_cannot_disappear() {
    let mut f = Fixture::new("");
    success(&f.select());
    let base = success(&f.qc("record-base"));
    f.builder(super::fixture::SUBJECT);
    f.ctx.set("quick_change_base", json!("0".repeat(40)));
    assert!(
        !f.qc("diff-limits").status.success(),
        "state/runner disagreement accepted"
    );
    f.ctx.set("quick_change_base", json!(base));
    std::fs::remove_file(f.state().join("base")).unwrap();
    assert!(
        !f.qc("diff-limits").status.success(),
        "missing base was recomputed"
    );
}

#[test]
fn oversized_continuation_stops_before_unrestricted_delegation_with_patch() {
    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    f.write(
        &f.caller,
        "notes.md",
        &"pre-existing review work\n".repeat(151),
    );
    success(&f.select());
    let base = success(&f.qc("record-base"));
    assert_eq!(success(&f.qc("diff-limits")), "handover");
    crate::support::exec_stub::write_executable(
        &f.tmp.path().join("bin/gh"),
        "#!/bin/bash\nprintf '%s\\n' \"$*\" >> \"$HOME/gh-calls\"\necho 'PR operations forbidden' >&2\nexit 1\n",
        0o755,
    );
    let head = f.git(&f.selected(), &["rev-parse", "HEAD"]);
    let out = f.qc("write-handover");
    assert!(
        !out.status.success(),
        "continuation allowed unrestricted handover"
    );
    let err = stderr(&out);
    assert!(
        err.contains("stopping before delegation") && err.contains("may create or modify a PR"),
        "{err}"
    );
    let note = std::fs::read_to_string(f.state().join("handover-note")).unwrap();
    assert!(note.contains(BRANCH) && note.contains(&base), "{note}");
    let patch = std::fs::read_to_string(f.state().join("quick-change-handover.patch")).unwrap();
    assert!(patch.contains("pre-existing review work"));
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), head);
    assert_eq!(
        std::fs::read_to_string(f.selected().join("notes.md")).unwrap(),
        "pre-existing review work\n".repeat(151)
    );
    let steps = recipe("quick-change");
    let ids: Vec<_> = steps["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.iter()
            .position(|id| *id == "quick-change-write-handover")
            .unwrap()
            < ids
                .iter()
                .position(|id| *id == "quick-change-handover")
                .unwrap()
    );
    assert!(f.read_home("gh-calls").is_empty());
}

#[test]
fn continuation_recovery_patch_restores_binary_untracked_and_tracked_work_privately() {
    use std::os::unix::fs::PermissionsExt;

    let mut f = Fixture::new(BRANCH);
    f.branch(true);
    // Seed an existing binary before pinning the selected branch's base.
    std::fs::write(f.caller.join("diagram.dat"), b"before\0\xff").unwrap();
    f.git(&f.caller, &["add", "diagram.dat"]);
    f.git(&f.caller, &["commit", "-qm", "add review diagram"]);
    f.git(&f.caller, &["push", "-q", "origin", BRANCH]);
    success(&f.select());
    let base = success(&f.qc("record-base"));
    std::fs::write(f.selected().join("diagram.dat"), b"after\0\xfe").unwrap();
    std::fs::write(f.selected().join("new diagram.dat"), b"new\0\xfd").unwrap();
    f.write(
        &f.selected(),
        "notes.md",
        &"oversized review notes\n".repeat(151),
    );
    assert_eq!(success(&f.qc("diff-limits")), "handover");
    let expected = f.git(&f.selected(), &["diff", "--binary", &base]);
    let out = f.qc("write-handover");
    assert!(!out.status.success());
    let patch = f.state().join("quick-change-handover.patch");
    assert_eq!(
        std::fs::metadata(&patch).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let restored = f.tmp.path().join("recovered checkout");
    f.git(
        &f.caller,
        &[
            "worktree",
            "add",
            "--detach",
            restored.to_str().unwrap(),
            &base,
        ],
    );
    f.git(&restored, &["apply", "--index", patch.to_str().unwrap()]);
    assert_eq!(f.git(&restored, &["diff", "--binary", &base]), expected);
    for path in ["diagram.dat", "new diagram.dat", "notes.md"] {
        assert_eq!(
            std::fs::read(restored.join(path)).unwrap(),
            std::fs::read(f.selected().join(path)).unwrap(),
            "recovery lost {path}"
        );
    }
    assert_eq!(f.git(&f.selected(), &["rev-parse", "HEAD"]), base);
    assert_eq!(f.git(&f.origin, &["rev-parse", BRANCH]), base);
    assert!(f.read_home("gh-calls").is_empty());
}

#[test]
fn ordinary_handover_keeps_exact_checkout_without_a_setup_push() {
    let mut f = Fixture::new("");
    success(&f.select());
    let selected = f.selected();
    let branch = f.git(&selected, &["branch", "--show-current"]);
    let refs = f.git(&f.origin, &["show-ref"]);
    success(&f.qc("record-base"));
    f.write(&selected, "notes.md", &"ordinary change\n".repeat(151));
    assert_eq!(success(&f.qc("diff-limits")), "handover");
    assert!(success(&f.qc("write-handover")).contains(&branch));
    let handover = step("quick-change", "quick-change-handover");
    assert!(
        f.ctx
            .evaluate(handover["condition"].as_str().unwrap())
            .unwrap()
    );
    let parent = f.ctx.clone();
    for (key, value) in handover["context"].as_mapping().unwrap() {
        f.ctx.set(
            key.as_str().unwrap(),
            json!(parent.render(value.as_str().unwrap())),
        );
    }
    success(&f.run("workflow-worktree", "step-04-setup-worktree"));
    assert_eq!(f.selected(), selected);
    assert_eq!(f.git(&f.origin, &["show-ref"]), refs);
    assert_eq!(
        std::fs::read_to_string(selected.join("notes.md")).unwrap(),
        "ordinary change\n".repeat(151)
    );
    assert!(f.read_home("gh-calls").is_empty());
}

#[test]
fn empty_diff_preserves_existing_no_change_behaviour() {
    let mut f = Fixture::new("");
    success(&f.select());
    success(&f.qc("record-base"));
    assert_eq!(f.qc("diff-limits").status.code(), Some(3));
    f.ctx.set("allow_no_op", json!("true"));
    assert_eq!(success(&f.qc("diff-limits")), "empty");
    for suffix in ["run-checks", "commit", "push", "create-pr"] {
        let s = step("quick-change", &format!("quick-change-{suffix}"));
        assert!(
            !f.ctx.evaluate(s["condition"].as_str().unwrap()).unwrap(),
            "{suffix} would run after an empty diff"
        );
    }
    assert!(f.read_home("gh-calls").is_empty());
    assert!(!f.read_home("hook-calls").contains("pre-commit"));
}
