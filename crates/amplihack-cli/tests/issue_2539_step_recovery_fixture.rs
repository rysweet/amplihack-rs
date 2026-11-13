//! Issue #2539 (scenario 8): a small recipe that uses both opt-in forms (a
//! wrapped bash step, and a sub-recipe with `continue_on_error` plus an
//! `--already-failed` recovery step) runs through `amplihack recipe run
//! <dir>/fixture-opt-in.yaml` and the real runner. A recipe's own directory
//! is searched first for sub-recipes, so the `step-failure-recovery` the
//! driver starts with `--recipe-dir <dir>` is the fixture's: a bash stand-in
//! that replays a scripted agent reply.
//! The real runner is `RECIPE_RUNNER_QA_BINARY`, else `recipe-runner-rs` on
//! `PATH`; without one each test says so and returns, unless
//! `AMPLIHACK_2539_REQUIRE_RECIPE_RUNNER` is set, when it fails.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::json;

use crate::support::step_recovery::{Fixture, HANDOFF_EXIT, show, text};

// ── Fixture recipe through the real runner (scenario 8) ─────────────────────

const STAND_IN: &str = r#"name: step-failure-recovery
description: test stand-in that replays a scripted recovery reply
version: 1.0.0
context:
  step_recovery_evidence: ""
recursion:
  max_total_steps: 5
steps:
  - id: recover
    type: bash
    command: |
      d="$STEP_RECOVERY_AGENT_DIR"
      n=$(( $(cat "$d/calls" 2>/dev/null | wc -l) + 1 ))
      printf '%s\t%s\n' "$n" "${AMPLIHACK_STEP_RECOVERY_ACTIVE-}" >> "$d/calls"
      cp "$RECIPE_VAR_step_recovery_evidence" "$d/evidence-$n.json"
      if [ -x "$d/repair-$n.sh" ]; then "$d/repair-$n.sh"; fi
      cat "$d/reply-$n.txt"
"#;

const CHILD: &str = "name: fixture-child\ndescription: always fails\nversion: 1.0.0\ncontext: {}\nsteps:\n  - id: fail\n    type: bash\n    command: \"exit 1\"\n";

const PARENT: &str = r#"name: fixture-opt-in
description: both opt-in forms
version: 1.0.0
context:
  step_recovery_dir: ""
  tool_dir: ""
  fixture_dir: ""
recursion:
  max_total_steps: 20
steps:
  - id: step-01-wrapped
    type: bash
    command: |
      amplihack orch helper step-recovery run --step-id step-01-wrapped \
        --dir "$RECIPE_VAR_step_recovery_dir" --goal "fixture goal" \
        --state-file "$RECIPE_VAR_tool_dir/state.json" --recipe-dir "$RECIPE_VAR_fixture_dir" \
        -- "$RECIPE_VAR_tool_dir/find-top"
    output: wrapped_result
  - id: step-02-sub
    type: recipe
    recipe: fixture-child
    continue_on_error: true
    output: child_result
  - id: step-02r-sub-recovery
    type: bash
    condition: "not child_result"
    command: |
      amplihack orch helper step-recovery run --already-failed --essential --step-id step-02-sub \
        --dir "$RECIPE_VAR_step_recovery_dir" --goal "fixture goal" \
        --state-file "$RECIPE_VAR_tool_dir/state.json" --recipe-dir "$RECIPE_VAR_fixture_dir" \
        -- "$RECIPE_VAR_tool_dir/child-again"
    output: child_recovery
  - id: step-03-use
    type: bash
    condition: "not child_recovery or 'step_recovery' not in child_recovery"
    command: echo "USED-MARK"
  - id: step-04-report
    type: bash
    command: amplihack orch helper step-recovery report --dir "$RECIPE_VAR_step_recovery_dir"
"#;

fn real_runner() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("RECIPE_RUNNER_QA_BINARY") {
        return Some(PathBuf::from(explicit));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("recipe-runner-rs"))
        .find(|c| {
            c.is_file()
                && Command::new(c)
                    .arg("--version")
                    .output()
                    .is_ok_and(|o| o.status.success())
        })
}

fn run_fixture(second_reply: serde_json::Value) -> Option<(Fixture, Output)> {
    let Some(runner) = real_runner() else {
        assert!(
            std::env::var_os("AMPLIHACK_2539_REQUIRE_RECIPE_RUNNER").is_none(),
            "no recipe-runner-rs found"
        );
        eprintln!("recipe-runner-rs unavailable; skipping the fixture recipe run");
        return None;
    };
    let f = Fixture::new();
    let recipes = f.root.join("recipes");
    crate::support::step_recovery::private_dir(&recipes);
    for (n, body) in [
        ("step-failure-recovery", STAND_IN),
        ("fixture-child", CHILD),
        ("fixture-opt-in", PARENT),
    ] {
        fs::write(recipes.join(format!("{n}.yaml")), body).unwrap();
    }
    std::os::unix::fs::symlink(
        crate::support::amplihack_bin::amplihack_bin(),
        f.tools.join("amplihack"),
    )
    .unwrap();
    fs::write(f.tools.join("state.json"), "{}").unwrap();
    let top = f.tool("find-top", "#!/bin/bash\nset -u\necho \"top is $TOP\"\n");
    f.tool("child-again", "#!/bin/sh\necho child-ok\n");
    let line = |v: serde_json::Value| json!({ "step_recovery": v }).to_string();
    fs::write(
        f.agent.join("reply-1.txt"),
        line(json!({"choice": "retry_after_repair",
        "reason": "TOP unset under set -u", "repair": "set TOP in the tool"})),
    )
    .unwrap();
    f.repair(1, &format!("#!/bin/sh\nprintf '#!/bin/bash\\nTOP=/t\\necho \"top is $TOP\"\\n' > '{0}.n'\nchmod 755 '{0}.n'\nmv -f '{0}.n' '{0}'\n", top.display()));
    fs::write(f.agent.join("reply-2.txt"), line(second_reply)).unwrap();

    let o = f
        .amplihack()
        .args(["recipe", "run"])
        .arg(recipes.join("fixture-opt-in.yaml"))
        .arg("-c")
        .arg(format!("step_recovery_dir={}", f.dir.display()))
        .arg("-c")
        .arg(format!("tool_dir={}", f.tools.display()))
        .arg("-c")
        .arg(format!("fixture_dir={}", recipes.display()))
        .env("RECIPE_RUNNER_RS_PATH", runner)
        .output()
        .unwrap();
    Some((f, o))
}

#[test]
fn a_fixture_recipe_recovers_both_opt_in_forms_and_finishes() {
    let retry = json!({"choice": "retry_after_repair", "reason": "the child stopped early", "repair": "none needed"});
    let Some((f, o)) = run_fixture(retry) else {
        return;
    };
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let out = text(&o.stdout);
    assert!(
        out.contains("top is /t") && out.contains("child-ok"),
        "{}",
        show(&o)
    );
    assert!(
        out.contains("USED-MARK"),
        "the dependent step runs on a recovered result"
    );
    assert_eq!(
        f.records("step-01-wrapped")[0]["choice"],
        "retry_after_repair"
    );
    assert!(f.records("step-02-sub")[0]["failure"]["exit_code"].is_null());
    assert!(
        f.agent_calls().iter().all(|c| c.ends_with("\t1")),
        "{:?}",
        f.agent_calls()
    );
    assert_eq!(f.report().status.code(), Some(0));
}

#[test]
fn a_fixture_handoff_skips_dependent_steps_and_ends_at_the_report() {
    let handoff = json!({"choice": "handoff", "reason": "the child keeps failing",
        "owner": "run-requester", "needs": "look at the child recipe"});
    let Some((f, o)) = run_fixture(handoff) else {
        return;
    };
    assert_ne!(
        o.status.code(),
        Some(0),
        "a handed-off run never ends green:\n{}",
        show(&o)
    );
    let all = format!("{}{}", text(&o.stdout), text(&o.stderr));
    assert!(
        !all.contains("USED-MARK"),
        "the dependent step is skipped:\n{all}"
    );
    assert!(
        all.contains("step-02-sub.jsonl"),
        "the report names the record:\n{all}"
    );
    assert_eq!(f.records("step-02-sub")[0]["choice"], "handoff");
    assert_eq!(f.report().status.code(), Some(HANDOFF_EXIT));
}
