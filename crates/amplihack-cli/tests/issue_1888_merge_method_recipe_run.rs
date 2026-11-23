//! #1888, through `amplihack recipe run` and the real recipe runner: an
//! authorised auto-drive merge uses the merge method the run was given, or one
//! the repository allows, and a method that cannot be settled hands the PR off
//! with no merge request.
//!
//! What is real: the CLI, the recipe runner, the runner's authorisation
//! capture and `report()`, the merge loop's own `step-03-finalize` step (copied
//! unchanged into a small calling recipe, so the `merge_method` it forwards is
//! the shipped forwarding), the finalize `merge` step with the trusted
//! `autodrive_merge.sh`, and the hand-off recipe's bash steps. What is a
//! stand-in: `gh` (an open PR with green checks, the repository's allowed
//! methods, and a merge that records its argv), and the finalisation and
//! hand-off agent steps, which become one-line bash steps in the test's copy
//! of the bundle. Assertions are on the recorded `gh pr merge` argv, the merge
//! record the step leaves in the state folder, the run's outcome and the
//! hand-off reason; no recipe text is read for an assertion. Nothing reaches
//! GitHub or an agent.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};
use serde_yaml::Value as Yaml;

use crate::support::amplihack_bin::amplihack_bin;
use crate::support::exec_stub::write_executable;

const PR: &str = "1888";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Copies the checkout's bundle as an install stages it, without group or
/// other write, so the trusted-bundle mode check passes under any umask.
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

fn read_yaml(path: &Path) -> Yaml {
    serde_yaml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn write_yaml(path: &Path, doc: &Yaml) {
    fs::write(path, serde_yaml::to_string(doc).unwrap()).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}

/// Replaces the step `id` of the recipe at `path` with a bash step.
fn bash_in_place(path: &Path, id: &str, output: &str, command: &str) {
    let mut doc = read_yaml(path);
    let steps = doc["steps"].as_sequence_mut().unwrap();
    let step = steps
        .iter_mut()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{} has no step {id}", path.display()));
    *step = serde_yaml::from_str(&format!(
        "id: {id}\ntype: bash\noutput: {output}\ncommand: {}\n",
        serde_json::to_string(command).unwrap()
    ))
    .unwrap();
    write_yaml(path, &doc);
}

/// GitHub: an open, clean PR at the fixture head with a green check. The
/// repository read prints `$FAKE_LOG/allows`; a merge records its argv and
/// the PR then reads MERGED.
const FAKE_GH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$FAKE_LOG/gh.log"
head="$(cat "$FAKE_LOG/head")"
case "${1:-} ${2:-}" in
  "pr view")
    case "$*" in *isDraft*) echo false; exit 0 ;; esac
    state=OPEN; [ -e "$FAKE_LOG/merged" ] && state=MERGED
    jq -cn --arg h "$head" --arg s "$state" '{state:$s,title:"Fixture",body:"Fixture body",mergeStateStatus:"CLEAN",headRefOid:$h}' ;;
  "pr checks") printf 'build\tpass\t1m\thttps://example.invalid\n' ;;
  "api repos/{owner}/{repo}") cat "$FAKE_LOG/allows" ;;
  "pr merge") printf '%s\n' "$@" > "$FAKE_LOG/merge-argv"; cat > /dev/null; : > "$FAKE_LOG/merged" ;;
  *) echo "fixture gh: unexpected call: $*" >&2; exit 1 ;;
esac
"#;

/// A merge loop that reached DONE and asks for finalisation, followed by the
/// merge loop's own `step-03-finalize`.
fn calling_recipe() -> String {
    let mut doc: Yaml = serde_yaml::from_str(
        r#"name: autodrive-1888-fixture
description: a DONE merge loop that finalises
version: 1.0.0
context:
  pr_number: ""
  no_merge: "true"
  autodrive_dry_run: "false"
  autodrive_state_dir: ""
  autodrive_authorization: ""
  build_result: ""
  merge_method: ""
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
        '{should_run:"true",pr:$pr,repo:$r,state_dir:$s}'
  - id: loop
    type: bash
    output: merge_loop_result
    command: echo '{"loop":"merge-ready","loop_result":"DONE","round_label":"round-1"}'
  - id: outcome
    type: bash
    parse_json: true
    output: merge_loop_outcome
    command: echo '{"outcome":"done","finalise":"true"}'
"#,
    )
    .unwrap();
    let merge_loop = read_yaml(&root().join("amplifier-bundle/recipes/autodrive-merge-loop.yaml"));
    let finalize = merge_loop["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some("step-03-finalize"))
        .expect("the merge loop's finalisation step")
        .clone();
    doc["steps"].as_sequence_mut().unwrap().push(finalize);
    serde_yaml::to_string(&doc).unwrap()
}

struct Drive {
    _tmp: tempfile::TempDir,
    out: Output,
    head: String,
    log: PathBuf,
    state: PathBuf,
}

impl Drive {
    fn merge_argv(&self) -> Option<Vec<String>> {
        fs::read_to_string(self.log.join("merge-argv"))
            .ok()
            .map(|s| s.lines().map(str::to_owned).collect())
    }
    fn record(&self) -> Value {
        let raw = fs::read_to_string(self.state.join("finalize-merge.json"))
            .unwrap_or_else(|e| panic!("no merge record ({e}): {}", self.show()));
        serde_json::from_str(raw.trim()).unwrap()
    }
    fn outcome(&self) -> Value {
        let v: Value = serde_json::from_slice(&self.out.stdout)
            .unwrap_or_else(|e| panic!("structured run result ({e}): {}", self.show()));
        v["autodrive_outcome"].clone()
    }
    fn show(&self) -> String {
        format!(
            "status: {}\nstdout: {}\nstderr: {}",
            self.out.status,
            String::from_utf8_lossy(&self.out.stdout),
            String::from_utf8_lossy(&self.out.stderr)
        )
    }
}

/// One authorised drive's finalisation against a repository that allows the
/// given methods, with `merge_method` passed as `-c` when it is `Some`.
fn drive(allow_merge: bool, allow_squash: bool, allow_rebase: bool, method: Option<&str>) -> Drive {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().canonicalize().unwrap();
    let home = base.join("home");
    let trusted = base.join("trusted");
    let bundle = trusted.join("amplifier-bundle");
    let repo = base.join("pr");
    let state = base.join("state");
    let bin = base.join("bin");
    let log = base.join("log");
    for d in [&home, &repo, &bin, &log] {
        fs::create_dir_all(d).unwrap();
    }
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    copy_bundle(&root().join("amplifier-bundle"), &bundle);
    let recipes = bundle.join("recipes");
    let finalize = recipes.join("autodrive-finalize.yaml");
    bash_in_place(
        &finalize,
        "observe-finalization-entry",
        "finalization_entry",
        "echo 'PR open, not a draft'",
    );
    bash_in_place(
        &finalize,
        "state-outcome",
        "autodrive_finalization",
        "echo 'finalisation ended'",
    );
    bash_in_place(
        &recipes.join("autodrive-handoff.yaml"),
        "hand-off",
        "handoff_action",
        "printf '%s\\n' \"$RECIPE_VAR_handoff_reason\" > \"$FAKE_LOG/handoff\"",
    );
    let recipe = recipes.join("autodrive-1888-fixture.yaml");
    fs::write(&recipe, calling_recipe()).unwrap();
    fs::set_permissions(&recipe, fs::Permissions::from_mode(0o644)).unwrap();

    let git = |args: &[&str]| -> String {
        let o = amplihack_git::command_in(&repo)
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
    };
    git(&["init", "-q", "-b", "feat/issue-1888"]);
    fs::write(repo.join("README.md"), "fixture\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-qm", "fixture"]);
    let head = git(&["rev-parse", "HEAD"]);
    let head_file = state.join("merge-ready-latest.head");
    fs::write(&head_file, format!("{head}\n")).unwrap();
    fs::set_permissions(&head_file, fs::Permissions::from_mode(0o600)).unwrap();

    fs::write(log.join("head"), &head).unwrap();
    fs::write(
        log.join("allows"),
        format!("{allow_merge}\t{allow_squash}\t{allow_rebase}\n"),
    )
    .unwrap();
    write_executable(&bin.join("gh"), FAKE_GH, 0o755);
    write_executable(
        &bin.join("codex"),
        "#!/bin/sh\necho 'fixture codex: no agent runs in this test' >&2\nexit 1\n",
        0o755,
    );
    std::os::unix::fs::symlink(amplihack_bin(), bin.join("amplihack")).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut c = crate::isolated_amplihack::isolated(amplihack_bin());
    c.args(["recipe", "run"])
        .arg(&recipe)
        .args(["-c", &format!("pr_number={PR}")])
        .args(["-c", "no_merge=false", "-c", "autodrive_dry_run=false"])
        .args(["-c", &format!("autodrive_state_dir={}", state.display())])
        .args(["-c", &format!("fixture_repo={}", repo.display())]);
    if let Some(m) = method {
        c.args(["-c", &format!("merge_method={m}")]);
    }
    let out = c
        .args(["--format", "json", "-w"])
        .arg(&repo)
        .current_dir(&repo)
        .env("HOME", &home)
        .env("AMPLIHACK_HOME", &trusted)
        .env(
            "RECIPE_RUNNER_RS_PATH",
            crate::issue_2539_step_recovery_fixture::real_runner(),
        )
        .env("PATH", path)
        .env("FAKE_LOG", &log)
        .env("AMPLIHACK_AGENT_BINARY", "codex")
        .env("AMPLIHACK_CLEANUP_ON_EXIT", "0")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .env("AMPLIHACK_DISABLE_BLARIFY", "1")
        .output()
        .expect("amplihack recipe run");
    Drive {
        _tmp: tmp,
        out,
        head,
        log,
        state,
    }
}

fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
    let i = argv.iter().position(|a| a == flag)?;
    argv.get(i + 1).map(String::as_str)
}

#[test]
fn a_run_given_merge_merges_with_a_merge_commit_at_the_assessed_head() {
    let d = drive(true, true, true, Some("merge"));
    assert_eq!(d.out.status.code(), Some(0), "{}", d.show());
    let argv = d
        .merge_argv()
        .unwrap_or_else(|| panic!("no merge: {}", d.show()));
    assert!(argv.iter().any(|a| a == "--merge"), "{argv:?}");
    for absent in ["--squash", "--rebase", "--admin", "--auto"] {
        assert!(!argv.iter().any(|a| a == absent), "{absent}: {argv:?}");
    }
    assert_eq!(
        flag_value(&argv, "--match-head-commit"),
        Some(d.head.as_str())
    );
    let r = d.record();
    assert_eq!(r["result"], json!("merged"), "{r}");
    assert_eq!(r["merge_method"], json!("merge"), "{r}");
    assert!(
        r["merge_method_reason"]
            .as_str()
            .unwrap_or("")
            .contains("given"),
        "{r}"
    );
    let o = d.outcome();
    assert_eq!(o["stop_reason"], json!("completed"), "{}", d.show());
}

#[test]
fn a_repository_that_allows_only_merge_commits_is_merged_with_one() {
    let d = drive(true, false, false, None);
    assert_eq!(d.out.status.code(), Some(0), "{}", d.show());
    let argv = d
        .merge_argv()
        .unwrap_or_else(|| panic!("no merge: {}", d.show()));
    assert!(argv.iter().any(|a| a == "--merge"), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "--squash"), "{argv:?}");
    assert_eq!(
        flag_value(&argv, "--match-head-commit"),
        Some(d.head.as_str())
    );
    let r = d.record();
    assert_eq!(r["merge_method"], json!("merge"), "{r}");
    assert!(
        r["merge_method_reason"]
            .as_str()
            .unwrap_or("")
            .contains("squash false"),
        "the reason names what the repository allows: {r}"
    );
}

#[test]
fn a_run_without_a_method_still_squashes_where_squash_is_allowed() {
    let d = drive(true, true, false, None);
    assert_eq!(d.out.status.code(), Some(0), "{}", d.show());
    let argv = d
        .merge_argv()
        .unwrap_or_else(|| panic!("no merge: {}", d.show()));
    assert!(argv.iter().any(|a| a == "--squash"), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "--merge"), "{argv:?}");
    assert_eq!(d.record()["merge_method"], json!("squash"));
}

#[test]
fn a_method_the_repository_disallows_is_handed_off_without_a_merge() {
    let d = drive(true, false, false, Some("squash"));
    assert!(d.merge_argv().is_none(), "no merge request: {}", d.show());
    let r = d.record();
    assert_eq!(r["result"], json!("refused"), "{r}");
    assert_eq!(r["handoff"], json!("true"), "{r}");
    let why = r["reason"].as_str().unwrap_or("");
    assert!(why.contains("'squash'"), "names the given method: {r}");
    assert!(why.contains("squash false"), "names what is allowed: {r}");
    assert_eq!(d.outcome()["stop_reason"], json!("handoff"), "{}", d.show());
    let handoff = fs::read_to_string(d.log.join("handoff"))
        .unwrap_or_else(|_| panic!("the hand-off was not reached: {}", d.show()));
    assert!(handoff.contains("refused"), "{handoff}");
}
