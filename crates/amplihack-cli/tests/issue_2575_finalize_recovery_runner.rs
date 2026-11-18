//! #2575 B3 and B4, through `amplihack recipe run` and the real recipe runner.
//!
//! B3: the first finalisation fails before its merge step. The merge loop's
//! real `step-03r-finalize-recovery` step sends the failure to the recovery
//! driver, which retries it: `amplihack recipe run autodrive-finalize` in a
//! separate process. That child merges. Its result never reaches the calling
//! run's context; the run must still end `completed` and successful, because
//! the merge step also writes its result, bound to the calling run's id, to the
//! runner-bound state folder.
//!
//! The same re-run that finds the branch behind its base hands the PR off:
//! no base sync follows a re-run, so the PR must not stay ready with no drive.
//!
//! B4: the same drive with merging off is a verify-only run that reaches DONE;
//! it ends `completed` and successful.
//!
//! What is real: the CLI, the recipe runner, the runner's authorization and
//! state-folder binding, `report()`, the merge loop's step-03r, the recovery
//! driver and the finalize `merge` step with the trusted `autodrive_merge.sh`.
//! What is a stand-in: `gh` (GitHub reads the PR merged at the head), the
//! recovery agent (it answers retry_after_repair), and the two finalisation
//! agent steps and the hand-off agent step, which become one-line bash steps in the test's copy of the
//! bundle. The real runner is `RECIPE_RUNNER_QA_BINARY`, else
//! `recipe-runner-rs` on `PATH`; without one each test says so and returns.
//! Nothing reaches GitHub or an agent. tests/gadugi/* is not run here.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};
use serde_yaml::Value as Yaml;

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;

const PR: &str = "2575";

fn root() -> PathBuf {
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

fn yaml(path: &Path) -> Yaml {
    serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn steps_mut(doc: &mut Yaml) -> &mut Vec<Yaml> {
    doc.get_mut("steps").unwrap().as_sequence_mut().unwrap()
}

fn bash_step(id: &str, output: &str, command: &str) -> Yaml {
    serde_yaml::from_str(&format!(
        "id: {id}\ntype: bash\noutput: {output}\ncommand: {}\n",
        serde_json::to_string(command).unwrap()
    ))
    .unwrap()
}

/// The finalize recipe with its two agent steps played by bash; the `merge`
/// step and the hand-off are the bundle's own.
fn finalize_without_agents(bundle: &Path) {
    let path = bundle.join("recipes/autodrive-finalize.yaml");
    let mut doc = yaml(&path);
    for step in steps_mut(&mut doc).iter_mut() {
        match step["id"].as_str() {
            Some("observe-finalization-entry") => {
                *step = bash_step(
                    "observe-finalization-entry",
                    "finalization_entry",
                    "echo 'PR open, not a draft'",
                )
            }
            Some("state-outcome") => {
                *step = bash_step(
                    "state-outcome",
                    "autodrive_finalization",
                    "echo 'finalisation ended'",
                )
            }
            _ => {}
        }
    }
    fs::write(&path, serde_yaml::to_string(&doc).unwrap()).unwrap();
    // The hand-off agent: it records that it was reached.
    let path = bundle.join("recipes/autodrive-handoff.yaml");
    let mut doc = yaml(&path);
    for step in steps_mut(&mut doc).iter_mut() {
        if step["id"].as_str() == Some("hand-off") {
            *step = bash_step(
                "hand-off",
                "handoff_action",
                "printf '%s\\n' \"$RECIPE_VAR_handoff_reason\" > \"$FAKE_LOG/handoff\"",
            );
        }
    }
    fs::write(&path, serde_yaml::to_string(&doc).unwrap()).unwrap();
}

/// The recovery agent: it answers retry_after_repair on its first call and
/// hands off on any later one, as an agent does when a retry fails again (a
/// re-run that does not merge exits non-zero).
const RECOVERY_STAND_IN: &str = r#"name: step-failure-recovery
description: test stand-in that retries once, then hands off
version: 1.0.0
context:
  step_recovery_evidence: ""
recursion:
  max_total_steps: 5
steps:
  - id: recover
    type: bash
    command: |
      if [ -e "$FAKE_LOG/recovered" ]; then
        printf '%s\n' '{"step_recovery":{"choice":"handoff","reason":"the re-run did not merge","owner":"run-requester","needs":"read the PR hand-off comment"}}'
      else
        : > "$FAKE_LOG/recovered"
        printf '%s\n' '{"step_recovery":{"choice":"retry_after_repair","reason":"the first finalisation stopped before its merge step","repair":"none needed; run finalisation again"}}'
      fi
"#;

/// GitHub: the PR reads MERGED at the assessed head, so `autodrive_merge`
/// reports `merged` without a merge request. With `$FAKE_LOG/behind` present
/// it reads OPEN and DIRTY with green checks, so `autodrive_merge` reports
/// `base-behind`; the hand-off read-back reads it a draft.
const FAKE_GH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FAKE_LOG/gh.log"
case "${1:-} ${2:-}" in
  "pr view")
    case "$*" in *isDraft*) echo true; exit 0 ;; esac
    if [ -e "$FAKE_LOG/behind" ]; then
      jq -cn --arg h "$(cat "$FAKE_LOG/head")" '{state:"OPEN",title:"Fixture",body:"",mergeStateStatus:"DIRTY",headRefOid:$h}'
    else
      jq -cn --arg h "$(cat "$FAKE_LOG/head")" '{state:"MERGED",title:"Fixture",body:"",mergeStateStatus:"CLEAN",headRefOid:$h}'
    fi ;;
  "pr checks") printf 'build\tpass\t1m\thttps://example.invalid\n' ;;
  *) echo "fixture gh: unexpected call: $*" >&2; exit 1 ;;
esac
"#;

const FIRST_FINALISATION: &str = "name: b3-first-finalisation\ndescription: fails before the merge step\nversion: 1.0.0\ncontext: {}\nsteps:\n  - id: observe-finalization-entry\n    type: bash\n    command: \"echo 'the observing agent failed' >&2; exit 1\"\n";

struct Fixture {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    trusted: PathBuf,
    repo: PathBuf,
    state: PathBuf,
    recovery: PathBuf,
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

fn private(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
}

impl Fixture {
    fn new() -> Self {
        let tmp = crate::support::private_tempdir::private_tempdir();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        crate::support::private_tempdir::private_dir_all(&home);
        let trusted = root.join("trusted");
        let bundle = trusted.join("amplifier-bundle");
        copy_bundle(&super_root_bundle(), &bundle);
        finalize_without_agents(&bundle);
        fs::write(
            bundle.join("recipes/step-failure-recovery.yaml"),
            RECOVERY_STAND_IN,
        )
        .unwrap();
        fs::set_permissions(
            bundle.join("recipes/step-failure-recovery.yaml"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let repo = root.join("pr");
        crate::support::private_tempdir::private_dir_all(&repo);
        git(&repo, &["init", "-q", "-b", "feat/issue-2575"]);
        fs::write(repo.join("README.md"), "fixture\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-qm", "fixture"]);
        let head = git(&repo, &["rev-parse", "HEAD"]);

        let state = root.join("state");
        private(&state);
        fs::write(state.join("merge-ready-latest.head"), format!("{head}\n")).unwrap();
        fs::set_permissions(
            state.join("merge-ready-latest.head"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let recovery = root.join("step-recovery");
        private(&recovery);

        let bin = root.join("bin");
        let log = root.join("log");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&log).unwrap();
        fs::write(log.join("head"), &head).unwrap();
        write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        // No agent runs in this test: an agent step that is reached fails.
        write_executable(
            &bin.join("codex"),
            "#!/bin/sh\necho 'fixture codex: no agent runs in this test' >&2\nexit 1\n",
            0o755,
        );
        std::os::unix::fs::symlink(amplihack_bin(), bin.join("amplihack")).unwrap();

        // An auto-drive run reads its recipes only from the launch bundle, so
        // the fixture recipes sit in the test's copy of it.
        let recipes = bundle.join("recipes");
        fs::write(
            recipes.join("b3-first-finalisation.yaml"),
            FIRST_FINALISATION,
        )
        .unwrap();
        let recipe = recipes.join("autodrive-b3-fixture.yaml");
        fs::write(&recipe, parent_recipe()).unwrap();
        for name in [
            "b3-first-finalisation.yaml",
            "autodrive-b3-fixture.yaml",
            "autodrive-finalize.yaml",
            "autodrive-handoff.yaml",
        ] {
            fs::set_permissions(recipes.join(name), fs::Permissions::from_mode(0o644)).unwrap();
        }
        Self {
            _tmp: tmp,
            home,
            trusted,
            repo,
            state,
            recovery,
            recipe,
            bin,
            log,
        }
    }

    fn run(&self, runner: &Path, no_merge: &str) -> Output {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        crate::isolated_amplihack::isolated(amplihack_bin())
            .args(["recipe", "run"])
            .arg(&self.recipe)
            .args(["-c", &format!("pr_number={PR}")])
            .args(["-c", &format!("no_merge={no_merge}")])
            .args(["-c", "autodrive_dry_run=false"])
            .args([
                "-c",
                &format!("autodrive_state_dir={}", self.state.display()),
            ])
            .args([
                "-c",
                &format!("step_recovery_dir={}", self.recovery.display()),
            ])
            .args(["-c", &format!("fixture_repo={}", self.repo.display())])
            // The run starts in the PR checkout, as a drive does, so the run's
            // Git recording can attribute the merge step's git reads.
            .args(["--format", "json", "-w"])
            .arg(&self.repo)
            .current_dir(&self.repo)
            .env("HOME", &self.home)
            .env("AMPLIHACK_HOME", &self.trusted)
            .env("RECIPE_RUNNER_RS_PATH", runner)
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
}

fn super_root_bundle() -> PathBuf {
    root().join("amplifier-bundle")
}

/// The calling run: a DONE loop, a first finalisation that fails before its
/// merge step, and the merge loop's own step-03r, copied from the bundle.
fn parent_recipe() -> String {
    let mut doc: Yaml = serde_yaml::from_str(
        r#"name: autodrive-b3-fixture
description: a recovered finalisation merges in another process
version: 1.0.0
context:
  step_recovery_dir: ""
  pr_number: ""
  no_merge: "true"
  autodrive_dry_run: "false"
  autodrive_state_dir: ""
  autodrive_authorization: ""
  build_result: ""
  fixture_repo: ""
  merge_loop_preflight: ""
  merge_loop_result: ""
  merge_loop_outcome: ""
  autodrive_finalization: ""
recursion:
  max_total_steps: 20
steps:
  - id: preflight
    type: bash
    output: merge_loop_preflight
    command: |
      jq -cn --arg r "$RECIPE_VAR_fixture_repo" --arg s "$RECIPE_VAR_autodrive_state_dir" --arg pr "$RECIPE_VAR_pr_number" \
        '{should_run:"true",pr:$pr,repo:$r,review_dir:$r,state_dir:$s}'
  - id: loop
    type: bash
    output: merge_loop_result
    command: echo '{"loop":"merge-ready","loop_result":"DONE","round_label":"round-1"}'
  - id: outcome
    type: bash
    parse_json: true
    output: merge_loop_outcome
    command: echo '{"outcome":"done","finalise":"true"}'
  - id: step-03-finalize
    type: recipe
    recipe: b3-first-finalisation
    continue_on_error: true
    output: autodrive_finalization
"#,
    )
    .unwrap();
    let merge_loop = yaml(&root().join("amplifier-bundle/recipes/autodrive-merge-loop.yaml"));
    let recovery = merge_loop["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some("step-03r-finalize-recovery"))
        .expect("the merge loop's finalisation recovery step")
        .clone();
    steps_mut(&mut doc).push(recovery);
    serde_yaml::to_string(&doc).unwrap()
}

/// The runner at the pinned revision, built once and shared with the other
/// runner-backed tests (#2658). It is always available, so these tests no
/// longer look on the host or skip themselves.
fn runner() -> PathBuf {
    crate::issue_2539_step_recovery_fixture::real_runner()
}

fn show(o: &Output) -> String {
    format!(
        "status: {}\nstdout: {}\nstderr: {}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn outcome(o: &Output) -> Value {
    let result: Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("structured run result ({e}): {}", show(o)));
    result["autodrive_outcome"].clone()
}

#[test]
fn a_merge_made_by_recovered_finalisation_ends_the_run_completed() {
    let runner = runner();
    let f = Fixture::new();
    let o = f.run(&runner, "false");
    let v = outcome(&o);
    assert_eq!(v["stop_reason"], json!("completed"), "{}", show(&o));
    assert_eq!(v["merge"]["result"], json!("merged"), "{}", show(&o));
    assert_eq!(v["requested_action"], json!("merge"), "{}", show(&o));
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    // The child wrote the record for this run, not for its own invocation.
    let record: Value = serde_json::from_str(
        fs::read_to_string(f.state.join("finalize-merge.json"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    assert_eq!(record["run_id"], v["run_id"], "{record}");
    // The recovery driver retried the failed finalisation once.
    let gh = fs::read_to_string(f.log.join("gh.log")).unwrap_or_default();
    assert!(gh.contains("pr view"), "{gh}");
    assert!(!gh.contains("pr merge"), "already merged at the head: {gh}");
}

#[test]
fn a_recovered_finalisation_that_finds_the_base_behind_hands_the_pr_off() {
    let runner = runner();
    let f = Fixture::new();
    fs::write(f.log.join("behind"), "").unwrap();
    let o = f.run(&runner, "false");
    let v = outcome(&o);
    assert_eq!(v["merge"]["result"], json!("base-behind"), "{}", show(&o));
    assert_eq!(v["merge"]["handoff"], json!("true"), "{}", show(&o));
    assert_eq!(v["stop_reason"], json!("not_merged"), "{}", show(&o));
    let handoff = fs::read_to_string(f.log.join("handoff"))
        .unwrap_or_else(|_| panic!("the hand-off was not reached: {}", show(&o)));
    assert!(handoff.contains("base-behind"), "{handoff}");
    let gh = fs::read_to_string(f.log.join("gh.log")).unwrap_or_default();
    assert!(
        gh.contains("isDraft"),
        "the hand-off read the PR back: {gh}"
    );
    assert!(
        !gh.contains("pr merge"),
        "no merge request on a DIRTY branch: {gh}"
    );
}

#[test]
fn a_verify_only_run_that_reaches_done_ends_completed() {
    let runner = runner();
    let f = Fixture::new();
    let o = f.run(&runner, "true");
    let v = outcome(&o);
    assert_eq!(
        v["requested_action"],
        json!("verify_readiness"),
        "{}",
        show(&o)
    );
    assert_eq!(
        v["merge"]["result"],
        json!("not-authorised"),
        "{}",
        show(&o)
    );
    assert_eq!(v["stop_reason"], json!("completed"), "{}", show(&o));
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let gh = fs::read_to_string(f.log.join("gh.log")).unwrap_or_default();
    assert!(
        gh.is_empty(),
        "a verify-only run asks GitHub nothing here: {gh}"
    );
}
