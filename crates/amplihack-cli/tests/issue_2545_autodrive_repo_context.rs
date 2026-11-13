//! #2545: the auto-drive helper steps read the PR worktree, the state
//! directory and the round record from the recipe context
//! (`RECIPE_VAR_<key>`) only, never from bare variables that a step may not
//! have, or that a parent run may have left with a stale value (#1561).
//!
//! Each bash step is run as written in the recipe under a cleared
//! environment. A fake `amplihack` records its arguments and, like the real
//! helper's path parser, exits 2 when a path argument is empty. HOME and
//! AMPLIHACK_HOME point at private temporary directories.

use serde_yaml::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn step_command(recipe: &str, id: &str) -> String {
    let path = root()
        .join("amplifier-bundle/recipes")
        .join(format!("{recipe}.yaml"));
    let text = fs::read_to_string(&path).expect("recipe readable");
    let doc: Value = serde_yaml::from_str(&text).expect("valid recipe YAML");
    doc["steps"]
        .as_sequence()
        .expect("recipe steps")
        .iter()
        .find(|step| step["id"].as_str() == Some(id))
        .and_then(|step| step["command"].as_str())
        .unwrap_or_else(|| panic!("{recipe}: missing bash step {id}"))
        .to_string()
}

const FAKE: &str = r#"#!/bin/sh
for a in "$@"; do printf '%s\n' "$a"; done >> "$ARGS_RECORD"
case " $* " in *" extract-field "*) printf 'true\n'; exit 0 ;; esac
prev=""
for a in "$@"; do
  case "$prev" in
    --repo|--directory|--record)
      [ -n "$a" ] || { echo "error: a value is required for '$prev <PATH>' but none was supplied" >&2; exit 2; } ;;
  esac
  prev="$a"
done
printf '{}\n'
"#;

/// How the step's environment is built.
#[derive(Default, Clone, Copy)]
struct Setup {
    /// Export bare REPO_PATH, AUTODRIVE_STATE_DIR and AUTODRIVE_ROUND_RECORD
    /// pointing at a decoy directory, as a stale parent run might.
    decoy: bool,
    /// Leave RECIPE_VAR_repo_path unset.
    no_repo_context: bool,
    /// Leave RECIPE_VAR_autodrive_state_dir unset.
    no_state_context: bool,
}

struct Run {
    status: i32,
    stderr: String,
    args: Vec<String>,
    worktree: PathBuf,
    state: PathBuf,
    record: PathBuf,
    decoy: PathBuf,
    _tmp: tempfile::TempDir,
}

fn run_step(recipe: &str, id: &str, setup: Setup) -> Run {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).expect("chmod tempdir");
    let base = tmp.path().to_path_buf();
    let home = base.join("home");
    let amplihack_home = base.join("amplihack-home");
    let bin = base.join("bin");
    let worktree = base.join("pr-worktree");
    let state = base.join("state");
    let decoy = base.join("decoy");
    for dir in [&home, &amplihack_home, &bin, &worktree, &state, &decoy] {
        fs::create_dir_all(dir).expect("mkdir");
    }
    let fake = bin.join("amplihack");
    fs::write(&fake, FAKE).expect("write fake");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).expect("chmod");
    let record = state.join("round.json");
    let args_record = base.join("args");
    let invocation = state.join("invocation.json");
    fs::write(&invocation, "{}").expect("invocation");

    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(step_command(recipe, id))
        .current_dir(&base)
        .env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", &home)
        .env("AMPLIHACK_HOME", &amplihack_home)
        .env("ARGS_RECORD", &args_record)
        .env("AMPLIHACK_BIN", &fake)
        .env("RECIPE_VAR_autodrive_round_record", &record)
        .env("RECIPE_VAR_autodrive_invocation_file", &invocation)
        .env("RECIPE_VAR_finalization_candidate", "{}")
        .env("RECIPE_VAR_finalization_observations", "{}")
        .env("RECIPE_VAR_accepted_readiness", "{}")
        .env("RECIPE_VAR_merge_ready_review", "{}");
    if !setup.no_repo_context {
        cmd.env("RECIPE_VAR_repo_path", &worktree);
    }
    if !setup.no_state_context {
        cmd.env("RECIPE_VAR_autodrive_state_dir", &state);
    }
    if setup.decoy {
        cmd.env("REPO_PATH", &decoy)
            .env("AUTODRIVE_STATE_DIR", &decoy)
            .env("AUTODRIVE_ROUND_RECORD", decoy.join("round.json"));
    }
    let output = cmd.output().expect("run bash step");
    let args = fs::read_to_string(&args_record)
        .expect("fake amplihack was invoked")
        .lines()
        .map(str::to_string)
        .collect();
    Run {
        status: output.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        args,
        worktree,
        state,
        record,
        decoy,
        _tmp: tmp,
    }
}

fn value_after(args: &[String], flag: &str) -> Option<String> {
    let at = args.iter().rposition(|a| a == flag)?;
    args.get(at + 1).cloned()
}

fn assert_context_paths(recipe: &str, id: &str, expect_record: bool, setup: Setup) {
    let run = run_step(recipe, id, setup);
    assert_eq!(run.status, 0, "{recipe}/{id} failed: {}", run.stderr);
    assert_eq!(
        value_after(&run.args, "--repo").as_deref(),
        Some(run.worktree.to_str().unwrap()),
        "{recipe}/{id} must pass the context repo_path as --repo"
    );
    assert_eq!(
        value_after(&run.args, "--directory").as_deref(),
        Some(run.state.to_str().unwrap()),
        "{recipe}/{id} must pass the context autodrive_state_dir as --directory"
    );
    if expect_record {
        assert_eq!(
            value_after(&run.args, "--record").as_deref(),
            Some(run.record.to_str().unwrap()),
            "{recipe}/{id} must pass the context autodrive_round_record as --record"
        );
    }
    assert_no_decoy(&run, recipe, id);
}

fn assert_no_decoy(run: &Run, recipe: &str, id: &str) {
    let decoy = run.decoy.to_str().unwrap();
    assert!(
        run.args.iter().all(|a| !a.contains(decoy)),
        "{recipe}/{id} used a bare variable instead of the recipe context: {:?}",
        run.args
    );
}

const FINALIZE: &str = "autodrive-finalize";
const ASSESS: &str = "accept-refreshed-readiness";
const REDUCE: &str = "reduce-observed-outcome";
const MERGE_ROUND: &str = "autodrive-merge-round";
const ROUND: &str = "step-03-extract-merge-ready-verdict";

#[test]
fn finalize_assess_uses_context_repo_path_without_bare_repo_path() {
    assert_context_paths(FINALIZE, ASSESS, false, Setup::default());
}

#[test]
fn finalize_reduce_uses_context_repo_path_without_bare_repo_path() {
    assert_context_paths(FINALIZE, REDUCE, false, Setup::default());
}

#[test]
fn merge_round_verdict_uses_context_paths_without_bare_variables() {
    assert_context_paths(MERGE_ROUND, ROUND, true, Setup::default());
}

/// A stale bare value from a parent run must never win over the context.
#[test]
fn bare_aliases_do_not_override_context() {
    let decoy = Setup {
        decoy: true,
        ..Setup::default()
    };
    assert_context_paths(MERGE_ROUND, ROUND, true, decoy);
    assert_context_paths(FINALIZE, ASSESS, false, decoy);
    assert_context_paths(FINALIZE, REDUCE, false, decoy);
}

/// A missing repo_path falls back to the recipe default `.`, never to a bare
/// REPO_PATH left in the environment.
#[test]
fn missing_repo_context_defaults_to_dot_not_bare_alias() {
    for (recipe, id) in [(FINALIZE, ASSESS), (FINALIZE, REDUCE), (MERGE_ROUND, ROUND)] {
        let run = run_step(
            recipe,
            id,
            Setup {
                decoy: true,
                no_repo_context: true,
                ..Setup::default()
            },
        );
        assert_eq!(run.status, 0, "{recipe}/{id} failed: {}", run.stderr);
        assert_eq!(
            value_after(&run.args, "--repo").as_deref(),
            Some("."),
            "{recipe}/{id} must default --repo to '.' when repo_path is missing"
        );
        assert_no_decoy(&run, recipe, id);
    }
}

/// A missing state directory is a caller error: the helper must see an empty
/// --directory and exit 2, not a value filled in from a bare alias.
#[test]
fn missing_state_dir_context_fails_closed_instead_of_using_bare_alias() {
    for (recipe, id) in [(FINALIZE, ASSESS), (FINALIZE, REDUCE), (MERGE_ROUND, ROUND)] {
        let run = run_step(
            recipe,
            id,
            Setup {
                decoy: true,
                no_state_context: true,
                ..Setup::default()
            },
        );
        assert_eq!(
            run.status, 2,
            "{recipe}/{id} must fail closed when autodrive_state_dir is missing; stderr: {}",
            run.stderr
        );
        assert_eq!(
            value_after(&run.args, "--directory").as_deref(),
            Some(""),
            "{recipe}/{id} must pass an empty --directory when the context has none"
        );
        assert_no_decoy(&run, recipe, id);
    }
}
