//! #2769: the #2663 outcome rule belongs to the drive's top-level
//! invocations, not to its nested `autodrive-*` sub-recipes.
//!
//! Since #2637 (first released in 1.2.165) `apply_autodrive_outcome` is
//! applied to every invocation `trusted::applies` accepts, which includes
//! every `autodrive-*` round recipe. A round never merges, so `report()`
//! reads its `stop_reason` as `unreadable` and the round is reported failed
//! however well its own steps went. `autodrive-completion dispatch` then
//! refuses `complete` for a failed round and the drive ends indeterminate.
//!
//! Test 1 drives a nested round the way `amplifier-bundle/tools/autodrive_loop.sh`
//! does: as its own `amplihack recipe run` process started from a bash step
//! of the calling run. That separation is what the defect needs. A `type:
//! recipe` step runs in process and never re-enters
//! `execute_recipe_via_rust_fresh`, so it would pass before and after the
//! change and prove nothing.
//!
//! Test 2 holds the other direction of #2663 in place: a top-level drive that
//! did not merge is still reported unsuccessful, and its process exit status
//! says so, because the exit status is what the loop script acts on.
//!
//! Test 3 holds the narrowing itself in place: it only ever leaves a
//! sub-recipe's verdict to its own steps, and never raises one. The same
//! drive over a round whose own step fails still stops at it.
//!
//! What is real: the CLI, the pinned recipe runner, authorisation capture and
//! the launch-bundle copy, the state-folder binding, `report()` and the
//! nested process boundary. What is a stand-in: `gh` and the agent binary
//! (neither is reached; both fail loudly if they are), and the round's own
//! review and fix agents, which are one-line bash steps in the test's copy of
//! the bundle. Nothing reaches GitHub, an agent, the network, the developer's
//! `~/.claude` or the developer's `~/.amplihack`.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;

fn checkout_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Copies the checkout's bundle as an install stages it: group and other
/// write are dropped, so the trusted-bundle mode check does not depend on the
/// checkout's umask. Symbolic links are kept as links.
fn copy_bundle(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    fs::set_permissions(to, fs::Permissions::from_mode(0o755)).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let dest = to.join(entry.file_name());
        if kind.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &dest).unwrap();
        } else if kind.is_dir() {
            copy_bundle(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest).unwrap();
            let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o755;
            fs::set_permissions(&dest, fs::Permissions::from_mode(mode)).unwrap();
        }
    }
}

/// A CLEAN round: the review and the fix work are one-line bash steps, so
/// every step of the round succeeds. The `name` is what the outcome rule
/// reads, and it is the production one.
const CRUSTY_ROUND: &str = r##"name: autodrive-crusty-round
description: "#2769 stand-in: one round whose own steps all succeed"
version: 1.0.0
context:
  pr_number: ""
  no_merge: "true"
  autodrive_dry_run: "false"
  autodrive_authorization: ""
  autodrive_round_label: "round-1"
  crusty_verdict: ""
recursion:
  max_total_steps: 10
steps:
  - id: review
    type: bash
    output: crusty_verdict
    command: echo CLEAN
  - id: address-concerns
    type: bash
    command: "true"
"##;

/// The same round with its own work failing. It merges nothing either, so
/// only its own steps can decide it: the narrowed rule must leave this round
/// failed, not raise it.
const CRUSTY_ROUND_FAILING: &str = r##"name: autodrive-crusty-round
description: "#2769 stand-in: one round whose own fix step fails"
version: 1.0.0
context:
  pr_number: ""
  no_merge: "true"
  autodrive_dry_run: "false"
  autodrive_authorization: ""
  autodrive_round_label: "round-1"
  crusty_verdict: ""
recursion:
  max_total_steps: 10
steps:
  - id: review
    type: bash
    output: crusty_verdict
    command: echo CONCERNS
  - id: address-concerns
    type: bash
    command: "exit 1"
"##;

/// The calling drive. Its first step starts the round as its own process, the
/// way `autodrive_loop.sh` does, and carries the round's exit status, as the
/// loop does when it hands `--child-exit-code` to the completion dispatcher.
/// The `merge` step stands for everything the loop reaches once a round is
/// allowed to complete; it merges nothing.
const TOP_LEVEL_WITH_ROUND: &str = r##"name: auto-drive-to-merge
description: "#2769 stand-in: a drive whose crusty round runs in its own process"
version: 1.0.0
context:
  pr_number: ""
  no_merge: "true"
  autodrive_dry_run: "false"
  autodrive_state_dir: ""
  autodrive_authorization: ""
  round_result: ""
recursion:
  max_total_steps: 20
steps:
  - id: crusty-round
    type: bash
    output: round_result
    command: |
      set -u
      round="${AMPLIHACK_HOME}/amplifier-bundle/recipes/autodrive-crusty-round.yaml"
      rc=0
      amplihack recipe run "$round" --format json \
        >"$FAKE_LOG/round.json" 2>"$FAKE_LOG/round.err" || rc=$?
      printf '%s\n' "$rc" >"$FAKE_LOG/round-rc"
      # The loop goes on only for a round that reported success; a failed
      # round cannot complete the objective (dispatch.rs).
      exit "$rc"
  - id: merge
    type: bash
    command: printf 'reached\n' >"$FAKE_LOG/merge-reached"
"##;

/// A drive that asks to merge, whose own steps all succeed, and which merges
/// nothing. Its `stop_reason` cannot be `completed`.
const TOP_LEVEL_WITHOUT_MERGE: &str = r##"name: auto-drive-to-merge
description: "#2769 stand-in: a drive that did not merge"
version: 1.0.0
context:
  pr_number: ""
  no_merge: "false"
  autodrive_dry_run: "false"
  autodrive_state_dir: ""
  autodrive_authorization: ""
recursion:
  max_total_steps: 10
steps:
  - id: work
    type: bash
    command: "true"
"##;

/// Neither stand-in is reached. Both say so and fail, so a test that reaches
/// one fails on that rather than passing on a silent call.
const FAKE_GH: &str =
    "#!/bin/sh\necho 'fixture gh: no GitHub call belongs in this test' >&2\nexit 1\n";
const FAKE_AGENT: &str =
    "#!/bin/sh\necho 'fixture agent: no agent runs in this test' >&2\nexit 1\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    trusted: PathBuf,
    repo: PathBuf,
    state: PathBuf,
    recipe: PathBuf,
    bin: PathBuf,
    log: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = amplihack_git::command_in(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {o:?}");
    String::from_utf8_lossy(&o.stdout).trim().to_owned()
}

impl Fixture {
    /// `top_level` is written over the bundle's own `auto-drive-to-merge.yaml`
    /// because an auto-drive run reads its recipe from the launch bundle at
    /// `recipes/<invocation name>.yaml`, never from the path on the command
    /// line (`execute.rs`, `pinned_recipe`).
    fn new(top_level: &str) -> Self {
        Self::with_round(top_level, CRUSTY_ROUND)
    }

    /// The same fixture with the nested round chosen by the caller, so one
    /// drive can be run over a round that succeeds and a round that fails.
    fn with_round(top_level: &str, round: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();

        let trusted = root.join("trusted");
        let bundle = trusted.join("amplifier-bundle");
        copy_bundle(&checkout_root().join("amplifier-bundle"), &bundle);
        let recipes = bundle.join("recipes");
        for (name, body) in [
            ("auto-drive-to-merge.yaml", top_level),
            ("autodrive-crusty-round.yaml", round),
        ] {
            fs::write(recipes.join(name), body).unwrap();
            fs::set_permissions(recipes.join(name), fs::Permissions::from_mode(0o644)).unwrap();
        }

        let repo = root.join("pr");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "feat/issue-2769"]);
        fs::write(repo.join("README.md"), "fixture\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-qm", "fixture"]);

        let state = root.join("state");
        fs::create_dir_all(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();

        let bin = root.join("bin");
        let log = root.join("log");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&log).unwrap();
        write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        write_executable(&bin.join("codex"), FAKE_AGENT, 0o755);
        std::os::unix::fs::symlink(amplihack_bin(), bin.join("amplihack")).unwrap();

        Self {
            _tmp: tmp,
            home,
            trusted,
            repo,
            state,
            recipe: recipes.join("auto-drive-to-merge.yaml"),
            bin,
            log,
        }
    }

    fn run(&self, no_merge: &str) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        crate::isolated_amplihack::isolated(amplihack_bin())
            .args(["recipe", "run"])
            .arg(&self.recipe)
            .args(["-c", "pr_number=2769"])
            .args(["-c", &format!("no_merge={no_merge}")])
            .args(["-c", "autodrive_dry_run=false"])
            .args([
                "-c",
                &format!("autodrive_state_dir={}", self.state.display()),
            ])
            .args(["--format", "json", "-w"])
            .arg(&self.repo)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.trusted)
            .env("RECIPE_RUNNER_RS_PATH", runner())
            .env("PATH", path)
            .env("FAKE_LOG", &self.log)
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .env("AMPLIHACK_DISABLE_BLARIFY", "1")
            .output()
            .expect("amplihack recipe run")
    }

    fn log(&self, name: &str) -> String {
        fs::read_to_string(self.log.join(name)).unwrap_or_default()
    }
}

/// The runner at the pinned revision, built once and shared with the other
/// runner-backed tests (#2658). A stub runner cannot be used here: it never
/// executes a step, so it cannot start the nested process this test is about.
fn runner() -> PathBuf {
    crate::issue_2539_step_recovery_fixture::real_runner()
}

fn show(f: &Fixture, o: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}\nround.json: {}\nround.err: {}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr),
        f.log("round.json"),
        f.log("round.err"),
    )
}

/// #2769: a nested round whose own steps all succeed reports success, so the
/// drive goes on. Before the fix the round exits 1 with `stop_reason`
/// `unreadable`, because it merged nothing, and the drive stops at it.
#[test]
fn a_nested_round_whose_steps_succeed_reports_success_and_the_drive_goes_on() {
    let f = Fixture::new(TOP_LEVEL_WITH_ROUND);
    let o = f.run("true");

    assert_eq!(
        f.log("round-rc").trim(),
        "0",
        "the nested autodrive-crusty-round process must exit 0 when its own \
         steps succeed: {}",
        show(&f, &o)
    );

    // The round still carries its outcome as data, for the loop script and
    // the completion helpers; only the verdict's scope changed.
    let round: Value = serde_json::from_str(f.log("round.json").trim()).unwrap_or_else(|e| {
        panic!(
            "structured result of the nested round ({e}): {}",
            show(&f, &o)
        )
    });
    let outcome = &round["autodrive_outcome"];
    assert!(
        outcome.get("stop_reason").and_then(Value::as_str).is_some(),
        "the nested round must still report its outcome: {}",
        show(&f, &o)
    );
    assert!(
        outcome.get("run_id").and_then(Value::as_str).is_some(),
        "the nested round must still report its run id: {}",
        show(&f, &o)
    );
    assert_ne!(
        outcome["stop_reason"].as_str(),
        Some("completed"),
        "a round merges nothing; its own steps, not a merge, make it a success: {}",
        show(&f, &o)
    );

    assert!(
        f.log.join("merge-reached").is_file(),
        "the drive must reach its merge step after a successful round: {}",
        show(&f, &o)
    );
}

/// #2663 in the other direction, unchanged by #2769: a top-level drive that
/// asked to merge and did not merge is not a success, and its process says so.
#[test]
fn a_top_level_drive_that_did_not_merge_is_still_unsuccessful() {
    let f = Fixture::new(TOP_LEVEL_WITHOUT_MERGE);
    let o = f.run("false");

    let result: Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("structured run result ({e}): {}", show(&f, &o)));
    let outcome = &result["autodrive_outcome"];
    assert_eq!(
        outcome["requested_action"].as_str(),
        Some("merge"),
        "{}",
        show(&f, &o)
    );
    assert_ne!(
        outcome["stop_reason"].as_str(),
        Some("completed"),
        "this drive merged nothing: {}",
        show(&f, &o)
    );
    assert_ne!(
        o.status.code(),
        Some(0),
        "a drive that did not merge must exit non-zero, because the exit \
         status is what its caller acts on: {}",
        show(&f, &o)
    );
}

/// #2769: the narrowing only ever leaves a sub-recipe's verdict to its own
/// steps. It never raises one. A nested round whose own step fails still
/// exits non-zero, and the drive stops at it: the stand-in's round step
/// propagates the child's exit status, which is the place the real loop
/// hands that status to `autodrive-completion dispatch`. The dispatcher
/// itself is not exercised here.
#[test]
fn a_nested_round_whose_own_step_fails_is_still_failed_and_stops_the_drive() {
    let f = Fixture::with_round(TOP_LEVEL_WITH_ROUND, CRUSTY_ROUND_FAILING);
    let o = f.run("true");

    assert_ne!(
        f.log("round-rc").trim(),
        "0",
        "a round whose own step failed must still exit non-zero: {}",
        show(&f, &o)
    );
    assert!(
        !f.log.join("merge-reached").is_file(),
        "the drive must not go on past a failed round: {}",
        show(&f, &o)
    );
    assert_ne!(
        o.status.code(),
        Some(0),
        "the drive carries the failed round's status: {}",
        show(&f, &o)
    );
}
