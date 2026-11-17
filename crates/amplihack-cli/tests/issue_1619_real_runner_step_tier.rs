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
//! The same two steps, with no `[step_tiers]`, also show what a declared tier
//! becomes on each harness when the runner launches it: on Codex no `--model`
//! and `-c model_reasoning_effort=high` for `strong`, on Copilot no
//! `--model`, and a Claude id in `AMPLIHACK_DEFAULT_MODEL` reaches neither.
//! The fixture recipes are this file's own; no bundled recipe is read.
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

/// The effort the shipped Codex column gives `strong`, written out for the
/// same reason as the two models above.
const CODEX_STRONG_EFFORT: &str = "high";

/// A Claude id an operator exports for their Claude launches.
const CLAUDE_ID: &str = "claude-sonnet-5-5";

/// A fake agent CLI. It answers the launcher's `--version` probe with
/// `@VERSION@`, writes its own argv NUL-separated to a file of its own under
/// `$AMPLIHACK_1619_ARGV_DIR`, drains stdin so the launcher's writer is never
/// left on a full pipe, and prints `done`. Where the command line names an
/// `--output-last-message` file, as the runner's Codex adapter does, it
/// writes `done` there too, under umask 077: the runner reads that file only
/// when it is private and owned by the same user.
const AGENT_STUB: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' "@VERSION@"
  exit 0
fi
umask 077
final=""
prev=""
for a in "$@"; do
  if [ "$prev" = "--output-last-message" ]; then final="$a"; fi
  prev="$a"
done
for a in "$@"; do printf '%s\000' "$a"; done > "$AMPLIHACK_1619_ARGV_DIR/launch.$$"
cat > /dev/null
if [ -n "$final" ]; then printf 'done\n' > "$final"; fi
printf 'done\n'
exit 0
"#;

/// The helper amplihack's health check expects beside `codex`. It answers a
/// version probe and records nothing, so it is never counted as a launch.
const CODEX_HELPER_STUB: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  printf '%s\n' "codex-cli 0.166.0"
fi
exit 0
"#;

/// The `--version` answer amplihack's probe accepts for `harness`. Claude
/// must be at least 2.1.319 (issue #2030).
fn version_of(harness: &str) -> &'static str {
    match harness {
        "codex" => "codex-cli 0.166.0",
        "copilot" => "0.0.400",
        _ => "2.1.319 (Claude Code)",
    }
}

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
    harness: &'static str,
    project: PathBuf,
    home: PathBuf,
    amplihack_home: PathBuf,
    codex_home: PathBuf,
    tools: PathBuf,
    work_root: PathBuf,
    runner: PathBuf,
    agent: PathBuf,
    recipe: PathBuf,
    launches: PathBuf,
}

impl Fixture {
    /// A stage whose agent steps run under `harness`, with `tier_file` as the
    /// instance's `model-tiers.toml` when there is one.
    fn new(harness: &'static str, tier_file: Option<&str>) -> Self {
        let runner = crate::pinned_runner::pinned_runner(&repo_root());
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let home = root.join("home");
        let amplihack_home = home.join(".amplihack");
        let tools = root.join("tools");
        let codex_home = root.join("codex-home");
        let launches = root.join("launches");
        let work_root = root.join("work");
        let recipes = root.join("recipes");
        // The directory amplihack forwards as a `-R`, and the only place the
        // sub-recipe can be found by name.
        let search = amplihack_home.join(".claude/recipes");
        for dir in [
            &home,
            &amplihack_home,
            &codex_home,
            &launches,
            &tools,
            &work_root,
            &recipes,
            &search,
            &home.join("profiles"),
        ] {
            fs::create_dir_all(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        if let Some(text) = tier_file {
            fs::write(amplihack_home.join("model-tiers.toml"), text).unwrap();
            fs::set_permissions(
                amplihack_home.join("model-tiers.toml"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        fs::write(search.join("issue-1619-child.yaml"), CHILD_RECIPE).unwrap();
        let recipe = recipes.join("issue-1619-tiered.yaml");
        fs::write(&recipe, TIERED_RECIPE).unwrap();

        // Since #2096 checkout discovery fails closed without a repository,
        // and #2176's Git recording otherwise ends the run unfinalised.
        let project = root.join("project");
        crate::support::git_fixture::init_with_commit(&project);
        fs::set_permissions(&project, fs::Permissions::from_mode(0o700)).unwrap();

        let agent = tools.join(harness);
        crate::support::exec_stub::write_executable(
            &agent,
            AGENT_STUB.replace("@VERSION@", version_of(harness)),
            0o700,
        );
        if harness == "codex" {
            crate::support::exec_stub::write_executable(
                &tools.join("codex-code-mode-host"),
                CODEX_HELPER_STUB,
                0o700,
            );
        }

        Self {
            _tmp: tmp,
            harness,
            project,
            home,
            amplihack_home,
            codex_home,
            tools,
            work_root,
            runner,
            agent,
            recipe,
            launches,
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
            .env("CODEX_HOME", &self.codex_home)
            .env("RECIPE_RUNNER_RS_PATH", &self.runner)
            .env("AMPLIHACK_AGENT_BINARY", self.harness)
            .env(
                format!("AMPLIHACK_{}_BINARY_PATH", self.harness.to_uppercase()),
                &self.agent,
            )
            // The runner starts an agent step through the wrapper, which it
            // looks for as plain `amplihack` on PATH unless named here. This
            // test's PATH holds only its own tools, and the wrapper is what
            // turns the tier word into a model id.
            .env("AMPLIHACK_LAUNCHER_BINARY", amplihack_bin())
            .env("AMPLIHACK_1619_ARGV_DIR", &self.launches)
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

    /// The argv of every agent launch this run made, one entry per launch.
    fn launches(&self) -> Vec<Vec<String>> {
        let Ok(entries) = fs::read_dir(&self.launches) else {
            return Vec::new();
        };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        files
            .iter()
            .map(|path| {
                let bytes = fs::read(path).unwrap_or_default();
                bytes
                    .split(|b| *b == 0)
                    .filter(|word| !word.is_empty())
                    .map(|word| String::from_utf8_lossy(word).into_owned())
                    .collect()
            })
            .collect()
    }

    /// The `--model` value of every agent launch this run made.
    fn models(&self) -> Vec<String> {
        self.launches()
            .iter()
            .flat_map(|argv| {
                argv.windows(2)
                    .filter(|pair| pair[0] == "--model")
                    .map(|pair| pair[1].clone())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Run the recipe, and the launches it made, failing with everything
    /// the run said when it did not succeed.
    fn run(&self, extra_env: &[(&str, &str)]) -> (Vec<Vec<String>>, String) {
        let mut command = self.command();
        for (name, value) in extra_env {
            command.env(name, value);
        }
        let output = command.output().expect("run amplihack recipe run");
        let shown = describe(&output, &self.launches);
        assert!(output.status.success(), "the run must succeed:\n{shown}");
        let launches = self.launches();
        assert_eq!(
            launches.len(),
            2,
            "both agent steps must have launched {}, or this test proves nothing:\n{shown}",
            self.harness
        );
        (launches, shown)
    }
}

/// The value of `-c model_reasoning_effort=...` on one launch, or `None`.
fn effort(argv: &[String]) -> Option<&str> {
    argv.windows(2)
        .filter(|pair| pair[0] == "-c")
        .filter_map(|pair| pair[1].strip_prefix("model_reasoning_effort="))
        .next_back()
}

/// Whether any word of `argv` is a Claude model id. Matched on the id's own
/// shape rather than on the substring `claude`, which a path amplihack adds
/// to a launch may contain.
fn has_claude_id(argv: &[String]) -> bool {
    argv.iter().any(|a| {
        let id = a.strip_prefix("--model=").unwrap_or(a);
        ["claude-opus", "claude-sonnet", "claude-haiku"]
            .iter()
            .any(|family| id.starts_with(family))
    })
}

/// Whether any word of `argv` is or carries a `--model`.
fn has_model(argv: &[String]) -> bool {
    argv.iter()
        .any(|a| a == "--model" || a.starts_with("--model="))
}

fn describe(output: &std::process::Output, launches: &Path) -> String {
    let mut recorded = String::new();
    if let Ok(entries) = fs::read_dir(launches) {
        for entry in entries.flatten() {
            let text = fs::read(entry.path()).unwrap_or_default();
            recorded.push_str(&format!(
                "{}: {}\n",
                entry.file_name().to_string_lossy(),
                String::from_utf8_lossy(&text).replace('\0', " ")
            ));
        }
    }
    format!(
        "exit {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- agent launches ({}) ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        launches.display(),
        recorded
    )
}

#[test]
fn a_step_tier_override_reaches_the_model_the_step_launches_on() {
    let f = Fixture::new("claude", Some(TIER_FILE));
    let (_, shown) = f.run(&[]);
    let models = f.models();
    assert_eq!(
        models.len(),
        2,
        "each launch must carry one model, or this test proves nothing:\n{shown}"
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

#[test]
fn a_declared_tier_reaches_a_codex_step_as_an_effort_and_no_model() {
    let f = Fixture::new("codex", None);
    let (launches, shown) = f.run(&[]);
    for argv in &launches {
        assert!(
            argv.iter().any(|a| a == "exec"),
            "the runner's Codex adapter launched this: {argv:?}\n{shown}"
        );
        assert!(
            !has_model(argv),
            "the shipped codex column names no model, so the tier word is removed \
             and Codex runs its own: {argv:?}\n{shown}"
        );
        assert!(
            !argv.iter().any(|a| a == "strong"),
            "a tier word reached Codex: {argv:?}\n{shown}"
        );
        assert_eq!(
            effort(argv),
            Some(CODEX_STRONG_EFFORT),
            "the step's `strong` reaches Codex as its own reasoning-effort argument: \
             {argv:?}\n{shown}"
        );
    }
}

#[test]
fn a_declared_tier_reaches_a_copilot_step_with_no_model() {
    let f = Fixture::new("copilot", None);
    let (launches, shown) = f.run(&[]);
    for argv in &launches {
        assert!(
            !has_model(argv),
            "the shipped copilot column is empty, so the runner's `--model strong` \
             is removed and Copilot runs its own model: {argv:?}\n{shown}"
        );
        assert!(
            !argv.iter().any(|a| a == "strong") && !has_claude_id(argv),
            "neither the tier word nor a Claude id reaches Copilot: {argv:?}\n{shown}"
        );
        // Issue #2589: amplihack's own deny rules ride on every Copilot
        // launch. Removing the tier pair must leave each `--deny-tool` with
        // its own value, never with a model word or another flag.
        let denies: Vec<&[String]> = argv
            .windows(2)
            .filter(|pair| pair[0] == "--deny-tool")
            .collect();
        assert!(
            !denies.is_empty(),
            "a Copilot launch carries amplihack's --deny-tool rules: {argv:?}\n{shown}"
        );
        for pair in denies {
            assert!(
                pair[1].starts_with("shell(") && pair[1].ends_with(":*)"),
                "each --deny-tool keeps its own value: {argv:?}\n{shown}"
            );
        }
    }
}

#[test]
fn a_claude_operator_default_never_reaches_a_codex_step() {
    let f = Fixture::new("codex", None);
    let (launches, shown) = f.run(&[("AMPLIHACK_DEFAULT_MODEL", CLAUDE_ID)]);
    for argv in &launches {
        assert!(
            !has_claude_id(argv),
            "AMPLIHACK_DEFAULT_MODEL is the Claude column's variable; a Claude id on \
             a Codex command line fails the step: {argv:?}\n{shown}"
        );
        assert!(!has_model(argv), "{argv:?}\n{shown}");
        assert_eq!(effort(argv), Some(CODEX_STRONG_EFFORT), "{argv:?}\n{shown}");
    }
}
