//! Issue #1619, the acceptance condition for `[step_tiers]`: a per-step tier
//! override reaches the model the step actually launches on, through the
//! runner built at the revision in `claude-plugin/recipe-runner.rev`.
//!
//! Every other `[step_tiers]` test checks what staging writes to disk or what
//! amplihack puts on the runner's command line; none asked the runner to
//! resolve a recipe and report which copy it read.
//!
//! ```text
//! amplihack recipe run <recipes>/issue-1619-tiered.yaml
//!   recipe-runner-rs
//!     step-08-implement   an agent step of the recipe named by path
//!     step-child          type: recipe, resolving `issue-1619-child` BY NAME
//!       step-20-final-cleanup   an agent step of the recipe found by name
//! ```
//!
//! `issue-1619-child.yaml` exists in two directories: the one amplihack
//! forwards as `-R`, and the staging directory holding the patched copy. Both
//! steps declare `model: "strong"`; `[step_tiers]` sets both to `fast`. A
//! launch on `claude-opus-5[1m]` means the runner read the unpatched copy.
//!
//! There is no skip path and no `#[ignore]`: a skip would go green against a
//! runner that resolves recipes in some other order and prove nothing.

#![cfg(unix)]

use crate::isolated_amplihack::HOME_GUARD_ENV;
use crate::support::amplihack_bin::amplihack_bin;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The model the `fast` tier names for the `claude` column in the shipped
/// table, and the one the `strong` tier names. Written out rather than read
/// from the crate: this test is about what reaches a process, so it must
/// fail if the shipped mapping changes under it.
const FAST_MODEL: &str = "claude-haiku-5-5";
const STRONG_MODEL: &str = "claude-opus-5[1m]";

/// The recipe named on the command line. Its own agent step is reachable
/// only by substituting the positional argument; its sub-recipe step is
/// reachable only through the first `-R`.
const TIERED_RECIPE: &str = r#"name: "issue-1619-tiered"
description: "one agent step, and one sub-recipe resolved by name"
version: "1.0.0"
context: {}
steps:
  - id: "step-08-implement"
    type: "agent"
    model: "strong"
    prompt: |
      Reply with the single word done and stop. Change nothing.
  - id: "step-child"
    type: "recipe"
    recipe: "issue-1619-child"
"#;

/// The sub-recipe, resolved by name out of a search directory.
const CHILD_RECIPE: &str = r#"name: "issue-1619-child"
description: "one agent step, reached by name"
version: "1.0.0"
context: {}
steps:
  - id: "step-20-final-cleanup"
    type: "agent"
    model: "strong"
    prompt: |
      Reply with the single word done and stop. Change nothing.
"#;

const TIER_FILE: &str = r#"version = 1

[step_tiers]
step-08-implement = "fast"
step-20-final-cleanup = "fast"
"#;

/// A fake agent CLI. It answers the launcher's `--version` probe, appends
/// its own argv to one record file, drains stdin so the launcher's writer is
/// never left on a full pipe, and exits 0.
const AGENT_STUB: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' "2.1.319 (Claude Code)"
  exit 0
fi
for a in "$@"; do printf '%s\n' "$a" >> "$AMPLIHACK_1619_ARGV"; done
printf -- '--- launch ---\n' >> "$AMPLIHACK_1619_ARGV"
cat > /dev/null
printf 'done\n'
exit 0
"#;

/// The repository root, two levels above this crate's manifest.
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the repo root is two levels above the crate manifest")
        .to_path_buf()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
    amplihack_home: PathBuf,
    tools: PathBuf,
    work_root: PathBuf,
    runner: PathBuf,
    agent: PathBuf,
    recipe: PathBuf,
    argv: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let runner = crate::pinned_runner::pinned_runner(&repo_root());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let amplihack_home = home.join(".amplihack");
        let tools = root.join("tools");
        let work_root = root.join("work");
        let recipes = root.join("recipes");
        // The directory amplihack forwards as a `-R`, and the only place the
        // sub-recipe can be found by name.
        let search = amplihack_home.join(".claude/recipes");
        for dir in [
            &home,
            &amplihack_home,
            &tools,
            &work_root,
            &recipes,
            &search,
            &home.join("profiles"),
        ] {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(amplihack_home.join("model-tiers.toml"), TIER_FILE).unwrap();
        fs::set_permissions(
            amplihack_home.join("model-tiers.toml"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::write(search.join("issue-1619-child.yaml"), CHILD_RECIPE).unwrap();
        let recipe = recipes.join("issue-1619-tiered.yaml");
        fs::write(&recipe, TIERED_RECIPE).unwrap();

        // Since #2096 checkout discovery fails closed without a repository,
        // and #2176's Git recording otherwise ends the run unfinalised.
        let project = root.join("project");
        crate::support::git_fixture::init_with_commit(&project);
        fs::set_permissions(&project, fs::Permissions::from_mode(0o700)).unwrap();

        let agent = tools.join("claude");
        crate::support::exec_stub::write_executable(&agent, AGENT_STUB, 0o700);

        Self {
            _tmp: tmp,
            project,
            home,
            amplihack_home,
            tools,
            work_root,
            runner,
            agent,
            recipe,
            argv: root.join("agent.argv"),
        }
    }

    fn command(&self) -> Command {
        let path = format!("{}:/usr/local/bin:/usr/bin:/bin", self.tools.display());
        let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
        c.args(["recipe", "run"])
            .arg(&self.recipe)
            .arg("-w")
            .arg(&self.project)
            .arg("--no-detach")
            .current_dir(&self.project)
            .env(HOME_GUARD_ENV, "1")
            .env("PATH", &path)
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.amplihack_home)
            .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", self.home.join("profiles"))
            .env("AMPLIHACK_WORK_ROOT", &self.work_root)
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("AMPLIHACK_AGENT_BINARY", "claude")
            .env("AMPLIHACK_CLAUDE_BINARY_PATH", &self.agent)
            // The runner starts an agent step through the wrapper, which it
            // looks for as plain `amplihack` on PATH unless named here. This
            // test's PATH holds only its own tools, and the wrapper is what
            // turns the tier word into a model id.
            .env("AMPLIHACK_LAUNCHER_BINARY", amplihack_bin())
            .env("AMPLIHACK_1619_ARGV", &self.argv)
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_SKIP_POWER_STEERING", "1")
            .env("AMPLIHACK_SKIP_REFLECTION", "1")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_GOVERNOR", "off")
            .env("AMPLIHACK_USAGE_LIMIT_WAIT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    /// The `--model` value of every agent launch this run made, in order.
    fn models(&self) -> Vec<String> {
        let Ok(text) = fs::read_to_string(&self.argv) else {
            return Vec::new();
        };
        let argv: Vec<&str> = text.lines().collect();
        argv.windows(2)
            .filter(|pair| pair[0] == "--model")
            .map(|pair| pair[1].to_string())
            .collect()
    }
}

fn describe(output: &std::process::Output, path: &Path) -> String {
    format!(
        "exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- agent argv ({}) ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        path.display(),
        fs::read_to_string(path).unwrap_or_default()
    )
}

#[test]
fn a_step_tier_override_reaches_the_model_the_step_launches_on() {
    let f = Fixture::new();
    let output = f.command().output().expect("run amplihack recipe run");
    let shown = describe(&output, &f.argv);
    assert!(output.status.success(), "the run must succeed:\n{shown}");

    let models = f.models();
    assert_eq!(
        models.len(),
        2,
        "both agent steps must have launched, or this test proves nothing:\n{shown}"
    );
    assert!(
        models.iter().all(|model| model == FAST_MODEL),
        "every launch must be on the tier `[step_tiers]` names: {models:?}\n{shown}"
    );
    assert!(
        !models.iter().any(|model| model == STRONG_MODEL),
        "a launch on the tier the recipe declares means the runner read an \
         unpatched copy: {models:?}\n{shown}"
    );
}
