//! Issue #2658 (Crusty B1/B2 on #2687): a shipped recipe's `replay: reuse`
//! bash steps replay after a usage-limit relaunch on the pinned runner.
//!
//! Runner 0.7.0 treats every context key a bash or recipe step can see as an
//! input of that step (runner #208), and replays the step only when the
//! recipe persists each such key. A key the recipe receives but does not
//! list in `recovery.persist.context` therefore stops every one of its
//! `replay: reuse` bash steps from replaying, and the earliest of them
//! becomes the restart point of the relaunch.
//!
//! Two cases cover the two levels, each running shipped recipes unchanged on
//! the pinned runner.
//!
//! The routing case runs `smart-classify-route.yaml` with `base_branch=main`
//! and `forbidden_base_branches=release/*`. A fake agent hits a session limit
//! at `classify-and-decompose`; the relaunch must resume there, with
//! `preflight-validation` restored. `main` matches no pattern in that deny
//! list, so no shipped step changes what it does. Removing either key from
//! the recipe's `recovery.persist.context` makes the case fail with the
//! restart at `preflight-validation`.
//!
//! The phase case runs the shipped `workflow-prep.yaml` and
//! `workflow-design.yaml` as `replay: never` children of a parent built from
//! the shipped `default-workflow.yaml`, standing in for it. The parent keeps
//! that recipe's context, recovery policy and recursion limits, and adds a
//! bash step emitting the `worktree_setup` object the worktree phase would
//! produce, which is what lets `workflow-design`'s repository resources
//! resolve. The run's one added input is `step_recovery_dir`, which
//! `default-workflow` declares and hands down.
//!
//! Both children finish, the parent's trailing step then hits a usage limit,
//! and the relaunch must restore the five bash `replay: reuse` steps of the
//! two children rather than run them again. Removing `step_recovery_dir` from
//! either child's persist list makes the case fail with the restart at that
//! child's earliest bash reuse step: `step-02d-detect-host-type` for
//! `workflow-prep`, `step-06b-checkpoint-doc-review` for `workflow-design`.
//!
//! Everything runs inside the fixture's temporary directory: its `HOME`, a
//! private `bin` and a `PATH` without `/usr/local/bin` or any user directory,
//! so no host agent, host runner or host `gh` can run in place of the fakes.
//! No case compares persist key lists; each watches which steps a relaunch
//! restores.
#![cfg(unix)]

use crate::issue_1741_fixture::{Attempt, Fixture, describe, quote};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const AGENT_STEP: &str = "classify-and-decompose";
const BASH_STEP: &str = "preflight-validation";

/// `workflow-prep`'s bash `replay: reuse` steps, earliest first. The first is
/// the restart point when that recipe cannot save a key it received.
const PREP_BASH_STEPS: [&str; 3] = [
    "step-02d-detect-host-type",
    "step-03-create-issue",
    "step-03b-extract-issue-number",
];

/// `workflow-design`'s bash `replay: reuse` steps, earliest first. The goal
/// probe is the last step of its recipe, so only a stop after the recipe
/// finishes puts it under observation.
const DESIGN_BASH_STEPS: [&str; 2] = [
    "step-06b-checkpoint-doc-review",
    "step-06d-goal-already-met-probe",
];

/// The parent's trailing step, where the phase case stops and restarts.
const STOP_STEP: &str = "stop-after-design";

/// The parent's stand-in for the worktree phase, which `default-workflow`
/// runs between prep and design. It is first here rather than second because
/// invalidating a step invalidates every later step of the same recipe, and a
/// stand-in after `workflow-prep` would be re-executed and hold the restart
/// point away from the step under test. Nothing in `workflow-prep` reads the
/// value, so producing it earlier changes no shipped step's behaviour.
const WORKTREE_STEP: &str = "setup-worktree-standin";

fn shipped_recipe(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes")
        .join(name)
}

fn shipped_tools() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../amplifier-bundle/tools")
}

/// A private `bin` holding the fixture's runner and the binary under test.
/// `preflight-validation` looks for `recipe-runner-rs` on `PATH` and several
/// steps call `amplihack orch helper`; neither may find a host install.
fn fixture_bin(f: &Fixture) -> PathBuf {
    let bin = f.root.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&f.runner, bin.join("recipe-runner-rs")).unwrap();
    symlink(
        crate::support::amplihack_bin::amplihack_bin(),
        bin.join("amplihack"),
    )
    .unwrap();
    bin
}

/// Copy a directory tree whole. The shipped helpers source one another by
/// relative path, so a step that finds one must find the rest beside it.
fn copy_tree(src: &Path, dest: &Path) {
    let status = std::process::Command::new("/bin/cp")
        .arg("-a")
        .arg(src)
        .arg(dest)
        .status()
        .expect("copy a shipped bundle directory");
    assert!(
        status.success(),
        "cp -a {} {}: {status}",
        src.display(),
        dest.display()
    );
}

/// Run one attempt with a `PATH` and launcher that reach only the fixture's
/// own fakes, and pair the output with the run directory it created.
fn attempt_with(
    f: &Fixture,
    recipe: &Path,
    bin: &Path,
    launcher: &Path,
    extra: &[&str],
) -> Attempt {
    let before = f.run_dirs();
    let output = f
        .command(recipe, &f.repo, "2658", extra)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("AMPLIHACK_LAUNCHER_BINARY", launcher)
        .output()
        .expect("execute amplihack");
    let run_dirs = f.run_dirs().difference(&before).cloned().collect();
    f.attempt_from(output, run_dirs)
}

/// The step ids a relaunch restored rather than re-executed.
///
/// These cases assert `mode` and `restart` directly rather than through
/// `Attempt::assert_resumed`, which resolves its source run through
/// `Attempt::dir()` and so requires a `smart-orchestrator` root scope. Both
/// cases run a shipped recipe that is not that one.
fn restored_steps(recovery: &Value) -> BTreeSet<&str> {
    recovery["restored_steps"]
        .as_array()
        .unwrap_or_else(|| panic!("restored_steps: {recovery:#}"))
        .iter()
        .filter_map(|s| s["step_id"].as_str())
        .collect()
}

struct Routing {
    f: Fixture,
    recipe: PathBuf,
    bin: PathBuf,
    launcher: PathBuf,
    marker: PathBuf,
}

impl Routing {
    fn new() -> Self {
        let f = Fixture::new();
        let recipe = f.root.join("smart-classify-route.yaml");
        fs::copy(shipped_recipe("smart-classify-route.yaml"), &recipe)
            .expect("copy the shipped smart-classify-route recipe");

        let bin = fixture_bin(&f);

        // The runner starts agent steps as `<launcher> claude -- <args>`, with
        // the task on stdin and the system prompt in --system-prompt-file
        // (runner #205). The fake drains stdin, records the call, and reports
        // a session limit until the marker exists.
        let marker = f.root.join("agent-may-answer");
        let launcher = bin.join("fake-agent");
        crate::support::exec_stub::write_executable(
            &launcher,
            format!(
                r#"#!/bin/sh
cat > /dev/null
printf '%s\n' {step} >> {events}
if [ ! -f {marker} ]; then
  printf '%s\n' "You've hit your session limit · resets 8:30am (America/Los_Angeles)" >&2
  exit 1
fi
printf '%s\n' '{{"task_type":"Development","goal":"g","success_criteria":["c"],"no_merge":false,"workstreams":[{{"name":"ws","description":"recover issue","recipe":"default-workflow"}}],"recommended_workflow":"default-workflow","workflow_reason":"r"}}'
"#,
                step = AGENT_STEP,
                events = quote(&f.root.join("events")),
                marker = quote(&marker),
            ),
            0o700,
        );
        Self {
            f,
            recipe,
            bin,
            launcher,
            marker,
        }
    }

    fn attempt(&self) -> Attempt {
        attempt_with(
            &self.f,
            &self.recipe,
            &self.bin,
            &self.launcher,
            &[
                "-c",
                "base_branch=main",
                // A key the recipe never declares, so the run sets it to a
                // value no default supplies. A key that is never set behaves
                // identically whether or not a persist list names it, and
                // would observe nothing.
                "-c",
                "forbidden_base_branches=release/*",
            ],
        )
    }
}

#[test]
fn shipped_classify_route_bash_step_replays_after_a_usage_limit_relaunch() {
    let r = Routing::new();

    let first = r.attempt();
    assert!(
        !first.output.status.success(),
        "{}",
        describe(&first.output)
    );
    assert!(
        String::from_utf8_lossy(&first.output.stderr).contains("usage_limit"),
        "the fake agent must stop the run at a recognized usage limit: {}",
        describe(&first.output)
    );
    assert_eq!(
        r.f.events(),
        [AGENT_STEP],
        "the fake agent, not a host agent, ran the agent step"
    );

    fs::write(&r.marker, "").unwrap();
    let second = r.attempt();
    let recovery = second.recovery();
    assert_eq!(recovery["mode"], "resumed", "{recovery:#}");
    assert_eq!(
        recovery["restart"]["step_id"],
        AGENT_STEP,
        "a persisted-input routing bash step must not become the restart point: {recovery:#}\n{}",
        describe(&second.output)
    );
    assert!(
        restored_steps(&recovery).contains(BASH_STEP),
        "{BASH_STEP} must be restored, not rerun: {recovery:#}\n{}",
        describe(&second.output)
    );
    assert_eq!(
        r.f.events(),
        [AGENT_STEP, AGENT_STEP],
        "the relaunch resumes at the agent step and calls the fake once more"
    );
}

struct Phases {
    f: Fixture,
    recipe: PathBuf,
    bin: PathBuf,
    launcher: PathBuf,
    agent_log: PathBuf,
    recovery_dir: PathBuf,
}

impl Phases {
    fn new() -> Self {
        let f = Fixture::new();

        // A local origin keeps every step off the network and makes
        // step-02d-detect-host-type report "other", which is the local
        // tracking path through step-03-create-issue. `create_issue` and
        // `strict_delivery` keep their defaults, so no step reaches a remote.
        let origin = f.root.join("origin.git");
        let origin_arg = origin.to_str().unwrap();
        f.git(&["init", "--bare", "--initial-branch=main", origin_arg]);
        f.git(&["remote", "set-url", "origin", origin_arg]);

        for name in ["workflow-prep.yaml", "workflow-design.yaml"] {
            fs::copy(shipped_recipe(name), f.root.join(name))
                .unwrap_or_else(|e| panic!("copy the shipped {name}: {e}"));
        }

        // step-03-create-issue fails rather than guess when its helper is
        // missing, and the doc-review checkpoint sources two more. Stage the
        // shipped tree under the fixture's AMPLIHACK_HOME, which the steps
        // search first, so none of them reaches a host install.
        let bundle = f.home.join(".amplihack/amplifier-bundle");
        fs::create_dir_all(&bundle).unwrap();
        copy_tree(&shipped_tools(), &bundle.join("tools"));

        let recovery_dir = f.root.join("step-recovery");
        fs::create_dir(&recovery_dir).unwrap();
        fs::set_permissions(&recovery_dir, fs::Permissions::from_mode(0o700)).unwrap();

        let bin = fixture_bin(&f);

        // Every agent step of both phases is answered from one superset
        // object: the parsed fields the recipes read are `requires_api_design`
        // and `requires_database_design` (step-05b and step-05c conditions)
        // and `status` (the doc-review checkpoint). Ordinary values, so the
        // shipped steps take their ordinary branches.
        let agent_log = f.root.join("agent-calls");
        let launcher = bin.join("fake-agent");
        crate::support::exec_stub::write_executable(
            &launcher,
            format!(
                r#"#!/bin/sh
cat > /dev/null
printf 'agent\n' >> {log}
printf '%s\n' '{{"status":"OK","requires_api_design":"true","requires_database_design":"true","summary":"fixture","components":[],"files_to_change":[],"risks":[],"findings":[]}}'
"#,
                log = quote(&agent_log),
            ),
            0o700,
        );

        let recipe = f.root.join("default-workflow-phases.yaml");
        let parent = parent_recipe(&f);
        f.write_recipe(&recipe, &parent);

        Self {
            f,
            recipe,
            bin,
            launcher,
            agent_log,
            recovery_dir,
        }
    }

    fn attempt(&self) -> Attempt {
        let recovery_dir = format!("step_recovery_dir={}", self.recovery_dir.display());
        attempt_with(
            &self.f,
            &self.recipe,
            &self.bin,
            &self.launcher,
            &["-c", &recovery_dir],
        )
    }

    /// How many agent steps the fake has answered across every attempt.
    fn agent_calls(&self) -> usize {
        match fs::read_to_string(&self.agent_log) {
            Ok(s) => s.lines().count(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
            Err(e) => panic!("read the fake agent's call log: {e}"),
        }
    }
}

/// The parent the two phase recipes run under: the shipped
/// `default-workflow.yaml` with its context, recovery policy and recursion
/// limits intact, carrying only the two phases under test, a stand-in for the
/// worktree phase, and a trailing step to stop at.
///
/// The stand-in emits the object the worktree phase would, which is what lets
/// `workflow-design`'s repository resources resolve in the selected checkout.
///
/// The two keys the fixture's own additions need are added to the persist
/// list when the shipped recipe does not already carry them, so that what
/// this case observes is the two children's lists and not the parent's.
fn parent_recipe(f: &Fixture) -> Value {
    let text = fs::read_to_string(shipped_recipe("default-workflow.yaml"))
        .expect("read the shipped default-workflow recipe");
    let mut parent: Value =
        serde_yaml::from_str(&text).expect("parse the shipped default-workflow recipe");
    parent["name"] = json!("default-workflow-phases");
    parent["description"] =
        json!("Issue 2658 fixture: the prep and design phases under a default-workflow parent");
    let worktree = json!({
        "id": WORKTREE_STEP,
        "type": "bash",
        "auto_stage": false,
        "command": "printf '{\"worktree_path\":\"%s\"}\\n' \"$REPO_PATH\"",
        "parse_json": true,
        "parse_json_required": true,
        "output": "worktree_setup",
        "recovery": {"replay": "reuse"},
    });
    parent["steps"] = json!([
        worktree,
        {"id": "workflow-prep", "recovery": {"replay": "never"}, "type": "recipe", "recipe": "workflow-prep"},
        {"id": "workflow-design", "recovery": {"replay": "never"}, "type": "recipe", "recipe": "workflow-design"},
        f.step(STOP_STEP),
    ]);
    parent["recovery"]["persist"]["outputs"] =
        json!([WORKTREE_STEP, "workflow-prep", "workflow-design", STOP_STEP]);
    let context = parent["recovery"]["persist"]["context"]
        .as_array_mut()
        .expect("the shipped default-workflow persists context keys");
    for key in [STOP_STEP, "step_recovery_dir"] {
        if !context.iter().any(|k| k == key) {
            context.push(json!(key));
        }
    }
    parent
}

#[test]
fn shipped_phase_recipes_replay_their_bash_steps_after_a_usage_limit_relaunch() {
    let p = Phases::new();

    p.f.mode(STOP_STEP, "limit");
    let first = p.attempt();
    assert!(
        !first.output.status.success(),
        "{}",
        describe(&first.output)
    );
    assert!(
        String::from_utf8_lossy(&first.output.stderr).contains("usage_limit"),
        "the first attempt must stop at a recognized usage limit, after both phases finish: {}",
        describe(&first.output)
    );
    assert_eq!(
        p.f.events(),
        [STOP_STEP],
        "both phase recipes completed and the run stopped at the parent's trailing step: {}",
        describe(&first.output)
    );
    let answered = p.agent_calls();
    assert!(
        answered > 0,
        "the fake agent, not a host agent, answered the phase agent steps: {}",
        describe(&first.output)
    );

    p.f.mode(STOP_STEP, "success");
    let second = p.attempt();
    assert!(
        second.output.status.success(),
        "{}",
        describe(&second.output)
    );
    let recovery = second.recovery();
    assert_eq!(recovery["mode"], "resumed", "{recovery:#}");
    assert_eq!(
        recovery["restart"]["step_id"],
        STOP_STEP,
        "a persisted-input phase bash step must not become the restart point: {recovery:#}\n{}",
        describe(&second.output)
    );
    let restored = restored_steps(&recovery);
    for step in PREP_BASH_STEPS.iter().chain(DESIGN_BASH_STEPS.iter()) {
        assert!(
            restored.contains(step),
            "{step} must be restored, not rerun: {recovery:#}\n{}",
            describe(&second.output)
        );
    }
    assert_eq!(
        p.f.events(),
        [STOP_STEP, STOP_STEP],
        "the relaunch resumes at the parent's trailing step and runs it once more"
    );
    assert_eq!(
        p.agent_calls(),
        answered,
        "the relaunch restores every phase agent step and calls the fake agent no further times"
    );
}
