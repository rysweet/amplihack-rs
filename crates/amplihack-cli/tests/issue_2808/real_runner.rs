//! CLI -> pinned runner -> real shell/agent launcher. The only agent is a
//! Git-observing stub: preflight routing and tier delivery, not live model
//! judgment. Whole builds/drives belong to #2806's CI-only release scenario.
use super::fixture::{Fixture, private, succeeds, workspace};
use crate::support::{amplihack_bin::amplihack_bin, exec_stub::write_executable};
use recipe_runner_rs::parser::RecipeParser;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn command(f: &Fixture, recipe: &str) -> Command {
    let path = f.temp.path().join("real-recipe.yaml");
    fs::write(&path, recipe).unwrap();
    let runner = crate::pinned_runner::pinned_runner(&workspace());
    let mut c = f.command(amplihack_bin());
    c.env("RECIPE_RUNNER_RS_PATH", runner)
        .env("AMPLIHACK_LAUNCHER_BINARY", amplihack_bin())
        .env("REAL_AMPLIHACK", amplihack_bin())
        .env("FIXTURE_STATE", &f.state)
        .env("OBSERVED_HEAD", &f.observed_head)
        .env("AMPLIHACK_GOVERNOR", "off")
        .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
        .args(["recipe", "run"])
        .arg(&path)
        .args(["--no-detach", "-w"])
        .arg(&f.repo)
        .arg("-c")
        .arg(format!("repo_path={}", f.repo.display()));
    c
}

#[test]
fn real_runner_build_phase_writes_in_named_clone_and_sees_its_linked_pr() {
    let f = Fixture::new();
    let recipe = r#"name: issue-2808-build-phase
version: 1.0.0
steps:
  - id: build-in-named-checkout
    type: bash
    command: |
      set -eu
      pwd -P > "$FIXTURE_STATE/build-cwd"
      git rev-parse --show-toplevel > "$FIXTURE_STATE/repository"
      git worktree list --porcelain > "$FIXTURE_STATE/worktrees"
      printf 'real build output\n' > build-result
"#;
    succeeds(&command(&f, recipe).output().unwrap());
    assert_eq!(
        PathBuf::from(
            fs::read_to_string(f.state.join("build-cwd"))
                .unwrap()
                .trim()
        ),
        f.repo
    );
    assert_eq!(
        fs::read(f.repo.join("build-result")).unwrap(),
        b"real build output\n"
    );
    let worktrees = fs::read_to_string(f.state.join("worktrees")).unwrap();
    assert!(
        worktrees.contains(f.pr.to_str().unwrap()),
        "named clone's linked PR was hidden: {worktrees}"
    );
    assert!(worktrees.contains(&f.observed_head));
    f.assert_no_private_repositories();
}

const PREFLIGHT: &str = r#"#!/bin/bash
set -eu
if [ "${1:-}" = --version ]; then echo '2.1.319 (Claude Code)'; exit 0; fi
printf '%s\0' "$@" > "$FIXTURE_STATE/agent-argv"
cat > /dev/null
pwd -P > "$FIXTURE_STATE/gate-cwd"
# Resolve through THIS repository's registration, not a fixture-supplied path.
git worktree list --porcelain > "$FIXTURE_STATE/gate-worktrees"
pr="$(awk -v expected="$OBSERVED_HEAD" '
  /^worktree / { path=substr($0, 10); head="" }
  /^HEAD / { head=$2 }
  /^branch refs\/heads\/feature\/2808$/ { if (head == expected) print path }
' "$FIXTURE_STATE/gate-worktrees")"
[ -n "$pr" ] || { echo 'PR worktree absent from the execution repository' >&2; exit 44; }
"$REAL_AMPLIHACK" orch helper autodrive worktree --repo "$pr" --head "$OBSERVED_HEAD" --branch feature/2808 > "$FIXTURE_STATE/verified-worktree"
jq -cn --arg repo "$pr" --arg head "$OBSERVED_HEAD" --arg state "$FIXTURE_STATE" \
  '{pr_found:"true",pr:"2808",pr_url:"https://example.invalid/pull/2808",pr_state:"OPEN",should_run:"true",skip_reason:"",branch:"feature/2808",head_sha:$head,state_dir:$state,repo:$repo,blockers:[],attempts:[]}'
"#;

#[test]
fn all_three_entry_preflights_resolve_pr_from_named_clone_on_declared_fast_tier() {
    for (name, id) in [
        ("autodrive-build", "step-01-build-preflight"),
        ("autodrive-crusty-loop", "step-01-crusty-loop-preflight"),
        ("autodrive-merge-loop", "step-01-merge-loop-preflight"),
    ] {
        let f = Fixture::new();
        let source = workspace()
            .join("amplifier-bundle/recipes")
            .join(format!("{name}.yaml"));
        let mut recipe = RecipeParser::new()
            .parse(&fs::read_to_string(source).unwrap())
            .unwrap();
        let gate = recipe
            .steps
            .iter()
            .position(|step| step.id == id)
            .expect("declared entry preflight");
        recipe.steps.truncate(gate + 1);
        // Execute this component's prefix without impersonating an authorized
        // top-level drive; invocation authorization has its own existing suite.
        recipe.name = format!("issue-2808-entry-{name}");
        let agents = f.home.join(".amplihack/.claude/agents/core");
        private(&agents);
        fs::write(
            agents.join("builder.md"),
            "---\nname: builder\ndescription: fixture\n---\nRead local Git facts.\n",
        )
        .unwrap();
        write_executable(&f.bin.join("claude"), PREFLIGHT, 0o700);
        let out = command(&f, &serde_yaml::to_string(&recipe).unwrap())
            .env("CLAUDE_BINARY_PATH", f.bin.join("claude"))
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", f.bin.join("claude"))
            .output()
            .unwrap();
        succeeds(&out);
        assert_eq!(
            fs::read_to_string(f.state.join("gate-cwd")).unwrap().trim(),
            f.repo.to_str().unwrap(),
            "{id}"
        );
        let argv = fs::read(f.state.join("agent-argv")).unwrap();
        let args: Vec<&[u8]> = argv.split(|byte| *byte == 0).collect();
        // Shipped fast mapping, asserted at the child process boundary.
        assert!(
            args.windows(2)
                .any(|pair| pair == [b"--model".as_slice(), b"claude-haiku-5-5".as_slice()]),
            "declared fast tier did not reach {id}: {args:?}"
        );
        let verified: serde_json::Value =
            serde_json::from_slice(&fs::read(f.state.join("verified-worktree")).unwrap()).unwrap();
        assert_eq!(verified["head_sha"], f.observed_head);
        assert_eq!(verified["worktree"], f.pr.to_str().unwrap());
        assert!(!f.home.join(".amplihack/model-tiers.toml").exists());
        f.assert_no_private_repositories();
    }
}
