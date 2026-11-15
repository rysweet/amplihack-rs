//! Issue #2658 (Crusty B1 on #2687): a shipped routing recipe's bash steps
//! replay after a usage-limit relaunch on the pinned runner.
//!
//! Runner 0.7.0 treats every context key a bash or recipe step can see as an
//! input of that step (runner #208), and replays the step only when the
//! recipe persists each such key. `smart-classify-route.yaml` sets
//! `base_branch` as a default, so the key is always present; unless the
//! recipe persists it, `preflight-validation` (a `replay: reuse` bash step)
//! reruns on relaunch and becomes the restart point.
//!
//! The test runs a copy of the shipped recipe, unchanged, on the pinned
//! runner. A fake agent hits a session limit at `classify-and-decompose`; the
//! relaunch must resume there, with `preflight-validation` restored. Removing
//! `base_branch` from the recipe's `recovery.persist.context` makes it fail
//! with the restart at `preflight-validation`.
//!
//! Everything runs inside the fixture's temporary directory: its `HOME`, a
//! private `bin` and a `PATH` without `/usr/local/bin` or any user directory,
//! so no host agent or host runner can run in place of the fakes.
#![cfg(unix)]

use crate::issue_1741_fixture::{Attempt, Fixture, describe, quote};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const AGENT_STEP: &str = "classify-and-decompose";
const BASH_STEP: &str = "preflight-validation";

fn shipped_recipe(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../amplifier-bundle/recipes")
        .join(name)
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

        let bin = f.root.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        // preflight-validation looks for recipe-runner-rs on PATH. Point it at
        // the fixture's capture wrapper around the pinned runner, never at a
        // PATH or `which` result.
        symlink(&f.runner, bin.join("recipe-runner-rs")).unwrap();
        // Later bash steps call `amplihack orch helper`; use the binary under
        // test, not a host install.
        symlink(
            crate::support::amplihack_bin::amplihack_bin(),
            bin.join("amplihack"),
        )
        .unwrap();

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
        let before = self.f.run_dirs();
        let output = self
            .f
            .command(
                &self.recipe,
                &self.f.repo,
                "2658",
                &["-c", "base_branch=main"],
            )
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("AMPLIHACK_LAUNCHER_BINARY", &self.launcher)
            .output()
            .expect("execute amplihack");
        let run_dirs = self.f.run_dirs().difference(&before).cloned().collect();
        self.f.attempt_from(output, run_dirs)
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
    let restored: Vec<&str> = recovery["restored_steps"]
        .as_array()
        .unwrap_or_else(|| panic!("restored_steps: {recovery:#}"))
        .iter()
        .filter_map(|s| s.as_str().or_else(|| s["step_id"].as_str()))
        .collect();
    assert!(
        restored.contains(&BASH_STEP),
        "{BASH_STEP} must be restored, not rerun: {recovery:#}"
    );
    assert_eq!(
        r.f.events(),
        [AGENT_STEP, AGENT_STEP],
        "the relaunch resumes at the agent step and calls the fake once more"
    );
}
