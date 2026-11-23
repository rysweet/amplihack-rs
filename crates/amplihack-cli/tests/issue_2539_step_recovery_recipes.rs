//! Issue #2539: the converted bundle recipes load, validate and route each
//! converted step through `step-recovery run`, and `workflow-finalize` ends a
//! handed-off run at its report step with exit 81, before `workflow-complete`.
//! The fixture recipe run through the real runner is in
//! `issue_2539_step_recovery_fixture.rs`.

#![cfg(unix)]

use std::process::Output;

use crate::support::auto_drive_light::{recipe, step_command};
use crate::support::step_recovery::{Fixture, HANDOFF_EXIT, bundle_recipes, show};

/// `(recipe, step whose command runs the driver, recorded step id)`. Bash
/// steps are wrapped in place; an agent or sub-recipe step keeps its type,
/// gets `continue_on_error`, and is recovered by the bash step after it.
///
/// #3129 replaced the Crusty loop's and the merge loop's converted steps
/// (their loop steps, the crusty-phase recovery and the finalisation
/// recovery) with one: the goal loop's `goal-loop` step, which runs the
/// driver through `step-recovery run`. The two old recipes are one-step
/// aliases now; [`ALIASES`] keeps them validated.
const CONVERTED: [(&str, &str, &str); 5] = [
    ("autodrive-goal-loop", "goal-loop", "goal-loop"),
    (
        "workflow-finalize",
        "step-20b-push-cleanup",
        "step-20b-push-cleanup",
    ),
    ("workflow-finalize", "step-21-pr-ready", "step-21-pr-ready"),
    (
        "workflow-finalize",
        "validate-agentic-finalization",
        "validate-agentic-finalization",
    ),
    // #2966: the re-entry into the CI repair, the assessment recipe's third
    // step, after the agent and the worktree reading. A bash step wrapped in
    // place.
    (
        "workflow-finalization-assessment",
        "continue-ci-repair",
        "continue-ci-repair",
    ),
];

/// The one-release aliases of the recipes whose steps were converted before
/// #3129. They still load and validate.
const ALIASES: [&str; 2] = ["autodrive-crusty-loop", "autodrive-merge-loop"];

fn validate(f: &Fixture, path: &std::path::Path) -> Output {
    f.amplihack()
        .args(["recipe", "validate"])
        .arg(path)
        .output()
        .unwrap()
}

fn step_ids(name: &str) -> Vec<String> {
    recipe(name)["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

fn step(name: &str, id: &str) -> serde_yaml::Value {
    recipe(name)["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap_or_else(|| panic!("{name} has no step {id}"))
        .clone()
}

#[test]
fn the_recovery_recipe_and_every_converted_recipe_load_and_validate() {
    let f = Fixture::new();
    let mut names: Vec<&str> = CONVERTED.iter().map(|(r, _, _)| *r).collect();
    names.extend(ALIASES);
    names.extend([
        "step-failure-recovery",
        "autodrive-build",
        "auto-drive-to-merge",
    ]);
    names.dedup();
    for name in names {
        let o = validate(&f, &bundle_recipes().join(format!("{name}.yaml")));
        assert_eq!(o.status.code(), Some(0), "{name}:\n{}", show(&o));
    }
    let r = recipe("step-failure-recovery");
    let steps = r["steps"].as_sequence().unwrap();
    assert_eq!(steps.len(), 1, "one agent step");
    assert_eq!(steps[0]["type"].as_str(), Some("agent"));
    assert_eq!(r["recursion"]["max_total_steps"].as_u64(), Some(5));
}

#[test]
fn each_converted_step_runs_its_command_through_the_driver_with_its_own_id() {
    for (name, step_id, id) in CONVERTED {
        let command = step_command(name, step_id);
        assert!(
            command.contains("step-recovery run"),
            "{name}/{id} is not wrapped:\n{command}"
        );
        assert!(
            command.contains(&format!("--step-id {id}")),
            "{name}/{id}: wrong id:\n{command}"
        );
        assert!(
            command.contains("\"$RECIPE_VAR_step_recovery_dir\""),
            "{name}/{id}:\n{command}"
        );
        let wrapped = command.split_once("step-recovery run").unwrap().1;
        assert!(
            !wrapped.contains("{{"),
            "{name}/{id}: values come from $RECIPE_VAR_*, never {{{{…}}}}"
        );
    }
    // A handoff before an outward action stops the recipe there.
    //
    // validate-agentic-finalization is not a guard, but it is `--no-repair`
    // (#2819). Since #2343 it reports the terminal state assess-finalization
    // decided, from values all recorded before the step first ran. Since
    // #2923 the recovery agent may push commits the run itself made, on the
    // failed step's branch, but this step has none of its own, and opening
    // the pull request a FAILED_IMPLEMENTATION is missing stays forbidden,
    // as does starting a recipe -- so it can neither open that pull request
    // nor produce an assessment, and a retry would read the same bytes and
    // fail identically. The step is
    // `--essential --stop-on-handoff --no-repair`: no continue_recorded, no
    // retry that cannot converge, and a handoff still exits 81. See
    // docs/reference/step-failure-recovery.md#when-a-retry-cannot-observe-a-repair.
    for (name, id, flag) in [
        // #3129: the goal loop's driver. The old loops stopped in code after
        // a round that recorded a handoff; the loop step now ends with exit
        // 81 when recovery hands the failed driver off.
        (
            "autodrive-goal-loop",
            "goal-loop",
            "--essential --stop-on-handoff",
        ),
        (
            "workflow-finalize",
            "validate-agentic-finalization",
            "--essential --stop-on-handoff --no-repair",
        ),
        (
            "workflow-finalize",
            "step-20b-push-cleanup",
            "--essential --stop-on-handoff",
        ),
        (
            "workflow-finalize",
            "step-21-pr-ready",
            "--essential --stop-on-handoff",
        ),
        // #2966, crusty round 7: a fresh session reads the pull request live,
        // so a pass that fails with the same lines as the last one is still
        // offered a retry.
        (
            "workflow-finalization-assessment",
            "continue-ci-repair",
            "--essential --stop-on-handoff --retry-on-recurrence",
        ),
    ] {
        let command = step_command(name, id);
        assert!(
            command.contains(flag),
            "{name}/{id} needs {flag}:\n{command}"
        );
        assert!(!command.contains("--state-file"), "{name}/{id}");
    }
    // #2966: unlike the validator, continue-ci-repair is offered a retry. What
    // it re-runs is the assessment, which reads the pull request live and may
    // repair it, so a retry can observe a repair.
    assert!(
        !step_command("workflow-finalization-assessment", "continue-ci-repair")
            .contains("--no-repair"),
        "continue-ci-repair must not be --no-repair"
    );
    for (name, step_id, id) in CONVERTED.iter().filter(|(_, s, i)| s != i) {
        let original = step(name, id);
        assert_eq!(
            original["continue_on_error"].as_bool(),
            Some(true),
            "{name}/{id}"
        );
        let command = step_command(name, step_id);
        // A sub-recipe's failure leaves no output to re-run; the crusty
        // phase check is read-only, so it runs first (an agent step can
        // succeed with empty output, which also runs this step).
        let already_failed = original["type"].as_str() == Some("recipe");
        assert_eq!(
            command.contains("--already-failed"),
            already_failed,
            "{name}/{step_id}"
        );
        let ids = step_ids(name);
        let pos = |s: &str| ids.iter().position(|x| x == s).unwrap();
        assert_eq!(pos(step_id), pos(id) + 1, "{name}: {step_id} follows {id}");
    }
}

#[test]
fn autodrive_build_recovers_its_sub_recipe_step_and_keeps_build_result_for_resolve_pr() {
    let s02 = step("autodrive-build", "step-02-default-workflow");
    assert_eq!(s02["continue_on_error"].as_bool(), Some(true));
    assert_eq!(s02["output"].as_str(), Some("workflow_result"));
    let s02r = step("autodrive-build", "step-02r-default-workflow-recovery");
    // step-02r runs only where step-02 ran and failed, never for a build
    // step-02 skipped.
    assert_eq!(
        s02r["condition"].as_str().unwrap(),
        format!(
            "{} and not workflow_result",
            s02["condition"].as_str().unwrap()
        )
    );
    assert_eq!(s02r["output"].as_str(), Some("build_recovery"));
    let cmd = s02r["command"].as_str().unwrap();
    for flag in [
        "--already-failed",
        "--essential",
        "--step-id step-02-default-workflow",
    ] {
        assert!(cmd.contains(flag), "{flag}:\n{cmd}");
    }
    let s03 = step("autodrive-build", "step-03-resolve-pr");
    assert_eq!(s03["output"].as_str(), Some("build_result"));
    let cond = s03["condition"].as_str().unwrap_or_default();
    assert!(
        cond.contains("build_recovery") && cond.contains("step_recovery"),
        "{cond}"
    );
    let ids = step_ids("autodrive-build");
    let pos = |id: &str| ids.iter().position(|s| s == id).unwrap();
    assert!(pos("step-02-default-workflow") < pos("step-02r-default-workflow-recovery"));
    assert!(pos("step-02r-default-workflow-recovery") < pos("step-03-resolve-pr"));
}

#[test]
fn workflow_finalize_reports_after_validation_and_before_completion() {
    let ids = step_ids("workflow-finalize");
    assert_eq!(ids[0], "step-recovery-init");
    let pos = |id: &str| {
        ids.iter()
            .position(|s| s == id)
            .unwrap_or_else(|| panic!("{id}: {ids:?}"))
    };
    assert!(pos("validate-agentic-finalization") < pos("step-recovery-report"));
    assert!(pos("step-recovery-report") < pos("workflow-complete"));

    // The report step's own command, run with the real binary: a recorded
    // handoff ends it with 81, an empty directory lets the run go on.
    let f = Fixture::new();
    std::os::unix::fs::symlink(
        crate::support::amplihack_bin::amplihack_bin(),
        f.tools.join("amplihack"),
    )
    .unwrap();
    let report = step_command("workflow-finalize", "step-recovery-report");
    let run_as = |f: &Fixture, dir: &std::path::Path| {
        crate::isolated_amplihack::isolated("bash")
            .args(["-c", &report])
            .current_dir(&f.work)
            .env("PATH", format!("{}:/usr/bin:/bin", f.tools.display()))
            .env("HOME", &f.home)
            .env("XDG_STATE_HOME", f.root.join("state"))
            .env("RECIPE_VAR_step_recovery_dir", dir)
            .output()
            .unwrap()
    };
    let run_report = |f: &Fixture| run_as(f, &f.dir);
    assert_eq!(
        run_report(&f).status.code(),
        Some(0),
        "no handoff: the run goes on"
    );
    assert!(
        f.dir.is_dir(),
        "a directory init did not create here is kept"
    );
    // Review S3: with recovery off, the report step says so in its output.
    let off = run_as(&f, std::path::Path::new(""));
    assert_eq!(off.status.code(), Some(0), "{}", show(&off));
    assert!(
        String::from_utf8_lossy(&off.stdout).contains(r#""step_recovery":"disabled""#),
        "{}",
        show(&off)
    );
    // Review S1: the directory workflow-finalize's own init created is
    // removed when it holds no record (the retry branch note included).
    let init = step_command("workflow-finalize", "step-recovery-init");
    let make_own = || {
        let made = crate::isolated_amplihack::isolated("bash")
            .args(["-c", &init])
            .env("PATH", format!("{}:/usr/bin:/bin", f.tools.display()))
            .env("HOME", &f.home)
            .env("XDG_STATE_HOME", f.root.join("state"))
            .output()
            .unwrap();
        let own = std::path::PathBuf::from(String::from_utf8_lossy(&made.stdout).trim());
        assert!(own.is_dir(), "{}", show(&made));
        own
    };
    let own = make_own();
    std::fs::write(own.join("build-branch.txt"), "feat/x\n").unwrap();
    let o = run_as(&f, &own);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(
        !own.exists(),
        "a clean run leaves no directory:\n{}",
        show(&o)
    );
    let own = make_own();
    let rec = f
        .amplihack()
        .args(["orch", "helper", "step-recovery", "record", "--dir"])
        .arg(&own)
        .args([
            "--step-id",
            "step-21-pr-ready",
            "--reason",
            "ready refused",
            "--owner",
            "run-requester",
            "--needs",
            "mark it ready",
        ])
        .output()
        .unwrap();
    assert_eq!(rec.status.code(), Some(0), "{}", show(&rec));
    // A handed-off run keeps the directory it created, for the operator.
    let o = run_as(&f, &own);
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    assert!(own.join("step-21-pr-ready.jsonl").is_file());
}

/// #2959 S3, re-pointed at the goal loop (#3129). Finalisation and its
/// recovery step are gone; the step that recovery now re-runs is the goal
/// loop's `goal-loop`, through `step-recovery run`. A re-run is the same
/// driver command: it reads merge authority only from the run's own captured
/// invocation (`--invocation`), and neither recovery nor the recipe hands it
/// an authorisation, a record or a permission flag of its own. Runs the
/// step's own command with a stand-in `amplihack` that records the recovery
/// call and runs the command after `--`, and a stand-in driver that records
/// its arguments.
#[test]
fn goal_loop_recovery_cannot_widen_permission() {
    let command = step_command("autodrive-goal-loop", "goal-loop");
    let run = |vars: &[(&str, &str)]| -> (Vec<String>, Vec<String>) {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let tools = dir.path().join("home/amplifier-bundle/tools");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        let recovery_log = dir.path().join("recovery.args");
        let driver_log = dir.path().join("driver.args");
        crate::support::exec_stub::write_executable(
            &bin.join("amplihack"),
            "#!/usr/bin/env bash\n\
             case \"$1 $2 $3\" in\n\
             \"orch helper step-recovery\") printf '%s\\n' \"$@\" >\"$RECORD_RECOVERY\"; while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\" ;;\n\
             *) echo \"unexpected: $*\" >&2; exit 3 ;;\n\
             esac\n",
            0o755,
        );
        crate::support::exec_stub::write_executable(
            &tools.join("autodrive_goal_loop.sh"),
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" >\"$RECORD_ARGS\"\n",
            0o755,
        );
        crate::support::exec_stub::copy_executable(
            &crate::support::step_recovery::repo_root()
                .join("amplifier-bundle/tools/recipe_context_value.sh"),
            &tools.join("recipe_context_value.sh"),
        );
        let scratch = dir.path().join("tmp");
        std::fs::create_dir(&scratch).unwrap();
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c")
            .arg(&command)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("TMPDIR", &scratch)
            .env("AMPLIHACK_HOME", dir.path().join("home"))
            .env("RECORD_RECOVERY", &recovery_log)
            .env("RECORD_ARGS", &driver_log)
            .env("RECIPE_VAR_step_recovery_dir", dir.path())
            .env("RECIPE_VAR_autodrive_state_dir", dir.path().join("state"))
            .current_dir(dir.path())
            .stdin(std::process::Stdio::null());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        let lines = |p: &std::path::Path| -> Vec<String> {
            std::fs::read_to_string(p)
                .unwrap_or_else(|e| panic!("{} was not written ({e}): {}", p.display(), show(&out)))
                .lines()
                .map(str::to_string)
                .collect()
        };
        let recovery = lines(&recovery_log);
        assert!(
            recovery.windows(2).any(|w| w == ["--step-id", "goal-loop"])
                && recovery
                    .windows(2)
                    .any(|w| w[0] == "--dir" && w[1] == dir.path().to_str().unwrap()),
            "the driver runs under step-recovery with the run's own directory: {recovery:?}"
        );
        (recovery, lines(&driver_log))
    };
    let invocation = "/run/state/invocation.json";
    for vars in [
        // No no_merge in the original inputs.
        vec![],
        // A verify-only original.
        vec![("RECIPE_VAR_no_merge", "true")],
        // A dry-run original.
        vec![
            ("RECIPE_VAR_no_merge", "false"),
            ("RECIPE_VAR_autodrive_dry_run", "true"),
        ],
        // An original that may merge, with an authorisation already in the
        // context: the re-run still reads only the captured invocation.
        vec![
            ("RECIPE_VAR_no_merge", "false"),
            ("RECIPE_VAR_autodrive_invocation_file", invocation),
            (
                "RECIPE_VAR_autodrive_authorization",
                r#"{"allows_merge":true,"pr_selected":true}"#,
            ),
        ],
    ] {
        let (recovery, driver) = run(&vars);
        let want = vars
            .iter()
            .find(|(k, _)| *k == "RECIPE_VAR_autodrive_invocation_file")
            .map_or("", |(_, v)| *v);
        let at = driver
            .iter()
            .position(|a| a == "--invocation")
            .unwrap_or_else(|| panic!("the driver was given no --invocation: {driver:?}"));
        assert_eq!(
            driver.get(at + 1).map(String::as_str),
            Some(want),
            "{vars:?}: merge authority comes only from the run's own captured invocation: {driver:?}"
        );
        assert_eq!(
            driver.iter().filter(|a| *a == "--invocation").count(),
            1,
            "{driver:?}"
        );
        for a in recovery.iter().chain(&driver) {
            for granted in [
                "allows_merge",
                "no_merge",
                "dry_run",
                "autodrive_authorization",
                "merge_allowed",
            ] {
                assert!(
                    !a.contains(granted),
                    "{vars:?}: the re-run is handed a permission of its own ({a}): {recovery:?} {driver:?}"
                );
            }
        }
    }
}

/// #2959, re-pointed at the goal loop's merge action (#3129). With the
/// authorisation helper failing (exit 2), nothing is merged and the action
/// says the authorisation could not be read, with the helper's own error.
/// Finalisation's exit 1, which gave its recovery step something to
/// recover, is gone with finalisation: the merge action reports a failure as
/// an outcome and the next turn reads it.
#[test]
fn the_merge_action_says_authorization_unreadable_when_the_record_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;
    let head = "2959295929592959295929592959295929592959";
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let stub = bin.join("amplihack");
    crate::support::exec_stub::write_executable(
        &stub,
        "#!/usr/bin/env bash\ncase \"$1 $2 $3 $4\" in\n  \"orch helper autodrive authorization\") echo 'ERROR: stub' >&2; exit 2 ;;\nesac\necho \"unexpected: $*\" >&2\nexit 3\n",
        0o755,
    );
    let gh_log = dir.path().join("gh.log");
    crate::support::exec_stub::write_executable(
        &bin.join("gh"),
        format!("#!/bin/sh\necho \"$*\" >> '{}'\nexit 0\n", gh_log.display()),
        0o755,
    );
    let state = dir.path().join("state");
    let td = state.join("turns").join("001");
    std::fs::create_dir_all(&td).unwrap();
    for d in [&state, &state.join("turns"), &td] {
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::write(
        td.join("decision.json"),
        serde_json::json!({"schema_version": 1, "valid": true, "action": "merge",
            "args": {"head": head}, "why": "fixture", "problem": null})
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        td.join("goal.json"),
        serde_json::json!({"schema_version": 1, "summary": "fixture", "pr": {"head": head}})
            .to_string(),
    )
    .unwrap();
    let out = std::process::Command::new("bash")
        .arg(
            crate::support::step_recovery::repo_root()
                .join("amplifier-bundle/tools/autodrive_pr_goal.sh"),
        )
        .args(["act", "merge"])
        .arg(&td)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AUTODRIVE_GOAL_CONTRACT", "1")
        .env("AUTODRIVE_STATE_DIR", &state)
        .env("AUTODRIVE_REPO", dir.path())
        .env("AUTODRIVE_WORKTREE", dir.path())
        .env("AMPLIHACK_BIN", &stub)
        .env("RECIPE_VAR_pr_number", "2959")
        .env(
            "AUTODRIVE_INVOCATION_FILE",
            state.join("gone/invocation.json"),
        )
        .env("RECIPE_VAR_no_merge", "false")
        .env("RECIPE_VAR_autodrive_dry_run", "false")
        .current_dir(dir.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(td.join("result.json"))
            .unwrap_or_else(|e| panic!("no result.json ({e}): {}", show(&out))),
    )
    .unwrap();
    assert_eq!(
        result["outcome"],
        "authorization-unreadable",
        "{result}: {}",
        show(&out)
    );
    assert!(
        result["summary"]
            .as_str()
            .is_some_and(|s| s.contains("ERROR: stub")),
        "the summary carries the helper's error: {result}"
    );
    assert!(
        !std::fs::read_to_string(&gh_log)
            .unwrap_or_default()
            .contains("pr merge"),
        "nothing was merged: {}",
        show(&out)
    );
}
