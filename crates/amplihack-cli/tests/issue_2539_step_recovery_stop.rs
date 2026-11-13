//! Issue #2539: a handoff on a guard or outward-action step stops the recipe
//! at that step with exit 81 (`--stop-on-handoff`, `--guard`), a guard step
//! cannot be repaired, and a failed `init` disables recovery without
//! stopping the run.
//!
//! The recipe tests run the real converted step commands, read from the
//! bundle's YAML, in a small recipe through the real runner, followed by a
//! marker step that must never run. The driver's own recovery recipe goes
//! through the fixture's stub runner, which plays the agent.

#![cfg(unix)]

use std::fs;
use std::process::Output;

use serde_json::json;
use serde_yaml::{Mapping, Value};

use crate::issue_2539_step_recovery_fixture::real_runner;
use crate::support::step_recovery::{
    Fixture, HANDOFF_EXIT, bundle_recipes, outcome, private_dir, show, text,
};

/// The `command` of step `id` in bundle recipe `recipe`.
fn bundle_command(recipe: &str, id: &str) -> String {
    let path = bundle_recipes().join(format!("{recipe}.yaml"));
    let doc: Value = serde_yaml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    doc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .and_then(|s| s["command"].as_str())
        .unwrap_or_else(|| panic!("{recipe} has no step {id} with a command"))
        .to_string()
}

fn bash_step(id: &str, command: &str) -> Value {
    let mut m = Mapping::new();
    m.insert("id".into(), id.into());
    m.insert("type".into(), "bash".into());
    m.insert("command".into(), command.into());
    Value::Mapping(m)
}

/// Runs `steps` as one recipe through the real runner with the fixture's
/// recovery dir. The driver's agent call goes to the fixture's stub runner.
fn run_recipe(f: &Fixture, steps: Vec<Value>, env: &[(&str, &str)]) -> Option<Output> {
    run_recipe_in(f, steps, f.dir.to_str().unwrap(), env)
}

/// As [`run_recipe`] with `step_recovery_dir=<srd>`. `None` when no real
/// runner exists (a failure under `AMPLIHACK_2539_REQUIRE_RECIPE_RUNNER`).
fn run_recipe_in(
    f: &Fixture,
    steps: Vec<Value>,
    srd: &str,
    env: &[(&str, &str)],
) -> Option<Output> {
    let Some(runner) = real_runner() else {
        assert!(
            std::env::var_os("AMPLIHACK_2539_REQUIRE_RECIPE_RUNNER").is_none(),
            "no recipe-runner-rs found"
        );
        eprintln!("recipe-runner-rs unavailable; skipping the recipe run");
        return None;
    };
    let recipes = f.root.join("recipes");
    private_dir(&recipes);
    let recipe = serde_yaml::to_string(&json!({
        "name": "stop-fixture", "description": "stop on handoff", "version": "1.0.0",
        "context": {"step_recovery_dir": "", "pr_number": "2539"},
        "recursion": {"max_total_steps": 20},
    }))
    .unwrap();
    let mut doc: Value = serde_yaml::from_str(&recipe).unwrap();
    doc["steps"] = Value::Sequence(steps);
    let path = recipes.join("stop-fixture.yaml");
    fs::write(&path, serde_yaml::to_string(&doc).unwrap()).unwrap();
    // The real wrappers start `step-failure-recovery` by name: give the
    // working tree the bundle's recipes (and no tools, so steps still fail).
    let bundle = f.work.join("amplifier-bundle");
    if !bundle.exists() {
        fs::create_dir_all(&bundle).unwrap();
        std::os::unix::fs::symlink(bundle_recipes(), bundle.join("recipes")).unwrap();
    }
    let link = f.tools.join("amplihack");
    if !link.exists() {
        std::os::unix::fs::symlink(crate::support::amplihack_bin::amplihack_bin(), link).unwrap();
    }
    let mut cmd = f.amplihack();
    cmd.args(["recipe", "run"])
        .arg(&path)
        .arg("-c")
        .arg(format!("step_recovery_dir={srd}"))
        .env("RECIPE_RUNNER_RS_PATH", runner)
        .env("STUB_RUNNER", f.tools.join("recipe-runner-rs"));
    for (k, v) in env {
        cmd.env(k, v);
    }
    Some(cmd.output().unwrap())
}

/// The real command, with the driver's agent call sent to the stub runner.
fn via_stub(command: &str) -> String {
    format!("export RECIPE_RUNNER_RS_PATH=\"$STUB_RUNNER\"\n{command}")
}

fn marker_step(f: &Fixture, id: &str) -> Value {
    bash_step(id, &format!("touch '{}'", f.root.join(id).display()))
}

fn handoff() -> serde_json::Value {
    json!({"choice": "handoff", "reason": "the PR cannot be marked ready",
        "owner": "run-requester", "needs": "mark the PR ready by hand"})
}

#[test]
fn a_handoff_on_pr_ready_ends_the_recipe_with_81_and_the_ci_step_never_runs() {
    let f = Fixture::new();
    f.choose(1, handoff());
    let step21 = via_stub(&bundle_command("workflow-finalize", "step-21-pr-ready"));
    let steps = vec![
        bash_step("step-21-pr-ready", &step21),
        marker_step(&f, "step-22"),
    ];
    // On a GitHub host with no PR-ready helper installed, step-21 fails.
    let Some(o) = run_recipe(&f, steps, &[("REMOTE_HOST_TYPE", "github")]) else {
        return;
    };
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    assert!(
        !f.root.join("step-22").exists(),
        "step-22 ran:\n{}",
        show(&o)
    );
    let records = f.records("step-21-pr-ready");
    assert_eq!(records[0]["choice"], "handoff");
    assert_eq!(
        records[0]["source"],
        "agent",
        "{}\n{}",
        records[0],
        show(&o)
    );
}

#[test]
fn a_handoff_on_the_crusty_review_dir_guard_ends_the_round_before_step_01() {
    let f = Fixture::new();
    f.choose(1, handoff());
    let empty = f.root.join("no-tools");
    private_dir(&empty);
    let guard = via_stub(&bundle_command(
        "autodrive-crusty-round",
        "step-00-review-dir-guard",
    ));
    let steps = vec![
        bash_step("step-00-review-dir-guard", &guard),
        marker_step(&f, "step-01-round-context"),
        marker_step(&f, "step-02-crusty-review"),
    ];
    let tools = empty.to_str().unwrap();
    let Some(o) = run_recipe(&f, steps, &[("AUTODRIVE_LAUNCH_TOOLS", tools)]) else {
        return;
    };
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    for later in ["step-01-round-context", "step-02-crusty-review"] {
        assert!(!f.root.join(later).exists(), "{later} ran:\n{}", show(&o));
    }
    let evidence = f.evidence(1);
    assert_eq!(evidence["guard"], true);
    assert_eq!(evidence["allowed_choices"], json!(["handoff"]));
}

#[test]
fn a_guard_step_refuses_a_repair_and_hands_off_with_81() {
    let f = Fixture::new();
    let tool = f.tool(
        "guard",
        "#!/bin/sh\necho 'STOP: review dir changed' >&2\nexit 1\n",
    );
    f.choose(
        1,
        json!({"choice": "retry_after_repair", "reason": "restore the review dir",
        "repair": "git checkout the review dir"}),
    );
    let o = f.run(
        "step-02b-review-dir-unchanged",
        &["--guard"],
        &[tool.to_str().unwrap()],
    );
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    let line = outcome(&o);
    assert_eq!(line["choice"], "handoff");
    assert_eq!(line["source"], "validator");
    let reason = f.records("step-02b-review-dir-unchanged")[0]["reason"].clone();
    assert!(reason.as_str().unwrap().contains("check 3"), "{reason}");
    assert_eq!(f.agent_calls().len(), 1, "the guard step never ran again");
}

#[test]
fn stop_on_handoff_still_exits_0_on_continue_and_without_the_flag_a_handoff_exits_0() {
    let f = Fixture::new();
    let tool = f.tool("fails", "#!/bin/sh\nexit 1\n");
    let tool = tool.to_str().unwrap();
    f.choose(
        1,
        json!({"choice": "continue_recorded", "reason": "optional item"}),
    );
    let o = f.run("step-a", &["--stop-on-handoff"], &[tool]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let g = Fixture::new();
    g.choose(1, handoff());
    let tool = g.tool("fails", "#!/bin/sh\nexit 1\n");
    let o = g.run("step-b", &[], &[tool.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(outcome(&o)["choice"], "handoff");
}

#[test]
fn init_under_a_home_holding_a_symlink_disables_recovery_and_steps_run_unwrapped() {
    let f = Fixture::new();
    // HOME itself is real; its `.local` is a symlink, which init refuses.
    let elsewhere = f.root.join("tmp").join("elsewhere");
    private_dir(&elsewhere);
    std::os::unix::fs::symlink(&elsewhere, f.home.join(".local")).unwrap();
    let o = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "init"])
        .env("XDG_STATE_HOME", "relative-state")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(text(&o.stdout), "\n", "{}", show(&o));
    assert!(text(&o.stderr).contains("[step-recovery] WARNING: recovery disabled:"));

    let init = bundle_command("workflow-finalize", "step-recovery-init");
    let mut init_step = bash_step("step-recovery-init", &init);
    init_step["output"] = "step_recovery_dir".into();
    let step21 = via_stub(&bundle_command("workflow-finalize", "step-21-pr-ready"));
    let steps = vec![
        init_step,
        bash_step("step-21-pr-ready", &step21),
        marker_step(&f, "after"),
    ];
    // A relative XDG_STATE_HOME is ignored, so init uses HOME/.local.
    let env = [
        ("XDG_STATE_HOME", "relative-state"),
        ("REMOTE_HOST_TYPE", "github"),
    ];
    let Some(o) = run_recipe_in(&f, steps, "", &env) else {
        return;
    };
    let all = format!("{}{}", text(&o.stdout), text(&o.stderr));
    assert_eq!(o.status.code(), Some(1), "fails as before recovery:\n{all}");
    // The runner does not show a successful step's stderr; the warning is
    // checked on the direct `init` call above.
    assert!(all.contains("workflow PR-ready helper not found"), "{all}");
    assert!(
        f.agent_calls().is_empty(),
        "no recovery agent: {:?}",
        f.agent_calls()
    );
    assert!(!f.root.join("after").exists());
}
