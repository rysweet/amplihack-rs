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
const CONVERTED: [(&str, &str, &str); 8] = [
    (
        "autodrive-crusty-loop",
        "step-02-crusty-loop",
        "step-02-crusty-loop",
    ),
    (
        "autodrive-crusty-loop",
        "step-03r-record-crusty-phase-recovery",
        "step-03-record-crusty-phase",
    ),
    (
        "autodrive-merge-loop",
        "step-02-merge-ready-loop",
        "step-02-merge-ready-loop",
    ),
    (
        "autodrive-merge-loop",
        "step-03r-finalize-recovery",
        "step-03-finalize",
    ),
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
    // #2966: the re-entry into the CI repair, the assessment recipe's second
    // step. A bash step wrapped in place.
    (
        "workflow-finalization-assessment",
        "continue-ci-repair",
        "continue-ci-repair",
    ),
];

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
        (
            "workflow-finalization-assessment",
            "continue-ci-repair",
            "--essential --stop-on-handoff",
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
fn merge_loop_recovers_finalisation_only_where_the_loop_asked_for_it() {
    // #2575: step-03 runs only when the loop outcome asks to finalise, so its
    // recovery never runs after a hand-off, where nothing was finalised.
    let s03 = step("autodrive-merge-loop", "step-03-finalize");
    assert_eq!(s03["continue_on_error"].as_bool(), Some(true));
    assert_eq!(s03["output"].as_str(), Some("autodrive_finalization"));
    let s03r = step("autodrive-merge-loop", "step-03r-finalize-recovery");
    assert_eq!(
        s03r["condition"].as_str().unwrap(),
        format!(
            "{} and not autodrive_finalization",
            s03["condition"].as_str().unwrap()
        )
    );
    let ids = step_ids("autodrive-merge-loop");
    let pos = |id: &str| ids.iter().position(|s| s == id).unwrap();
    assert_eq!(
        pos("step-03r-finalize-recovery"),
        pos("step-03-finalize") + 1
    );
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

/// #2959 S3: when the finalize merge step fails and the recovery step re-runs
/// finalisation, the re-run is captured from the operator's own inputs and
/// cannot widen permission. Runs step-03r's own command with a stand-in
/// `amplihack` that records the `recipe run` arguments it is given.
#[test]
fn finalisation_recovery_cannot_widen_permission() {
    use std::os::unix::fs::PermissionsExt;
    let command = step_command("autodrive-merge-loop", "step-03r-finalize-recovery");
    let run = |vars: &[(&str, &str)]| -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = dir.path().join("recipe-run.args");
        std::fs::write(
            bin.join("amplihack"),
            "#!/usr/bin/env bash\n\
             case \"$1 $2 $3\" in\n\
             \"orch helper step-recovery\") while [ \"$1\" != -- ]; do shift; done; shift; exec \"$@\" ;;\n\
             \"orch helper extract-json\") cat ;;\n\
             \"orch helper extract-field\") cat >/dev/null ;;\n\
             \"recipe run autodrive-finalize\") printf '%s\\n' \"$@\" >\"$RECORD_ARGS\" ;;\n\
             *) echo \"unexpected: $*\" >&2; exit 3 ;;\n\
             esac\n",
        )
        .unwrap();
        std::fs::set_permissions(
            bin.join("amplihack"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c")
            .arg(&command)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("RECORD_ARGS", &log)
            .env("RECIPE_VAR_step_recovery_dir", dir.path())
            .current_dir(dir.path())
            .stdin(std::process::Stdio::null());
        for (k, v) in vars {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        std::fs::read_to_string(&log)
            .unwrap_or_else(|e| panic!("recipe run was not invoked ({e}): {}", show(&out)))
            .lines()
            .map(str::to_string)
            .collect()
    };
    // No no_merge in the original inputs: the re-run does not merge.
    let args = run(&[]);
    assert!(args.contains(&"no_merge=true".to_string()), "{args:?}");
    // A verify-only original stays verify-only.
    let args = run(&[("RECIPE_VAR_no_merge", "true")]);
    assert!(args.contains(&"no_merge=true".to_string()), "{args:?}");
    // A dry-run original stays a dry run.
    let args = run(&[
        ("RECIPE_VAR_no_merge", "false"),
        ("RECIPE_VAR_autodrive_dry_run", "true"),
    ]);
    assert!(
        args.contains(&"autodrive_dry_run=true".to_string()),
        "{args:?}"
    );
    // Recovery hands the re-run no authorization or record of its own: the
    // re-run captures its invocation afresh from these inputs.
    for args in [run(&[]), run(&[("RECIPE_VAR_no_merge", "false")])] {
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("autodrive_invocation_file=") || a.contains("allows_merge")),
            "{args:?}"
        );
    }
}

/// #2959: with the helper failing (exit 2), the finalize merge step exits
/// non-zero so the loop's recovery step has something to recover.
#[test]
fn the_finalize_merge_step_fails_loudly_when_the_record_cannot_be_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let stub = bin.join("amplihack");
    std::fs::write(
        &stub,
        "#!/usr/bin/env bash\ncase \"$1 $2 $3 $4\" in\n  \"orch helper autodrive authorization\") echo 'ERROR: stub' >&2; exit 2 ;;\nesac\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let out = std::process::Command::new("bash")
        .arg("-c")
        .arg(step_command("autodrive-finalize", "merge"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AMPLIHACK_HOME", dir.path())
        .env("AMPLIHACK_BIN", &stub)
        .env("RECIPE_VAR_pr_number", "2959")
        .env("RECIPE_VAR_repo_path", dir.path())
        .env("REPO_PATH", dir.path())
        .env("RECIPE_VAR_autodrive_state_dir", &state)
        .env(
            "RECIPE_VAR_autodrive_invocation_file",
            state.join("gone/invocation.json"),
        )
        .env("RECIPE_VAR_no_merge", "false")
        .env("RECIPE_VAR_autodrive_dry_run", "false")
        .current_dir(dir.path())
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", show(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("authorization-unreadable"),
        "{}",
        show(&out)
    );
}
