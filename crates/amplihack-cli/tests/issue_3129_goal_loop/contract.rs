//! #3129 commit A: the shared loop's contract, with a stand-in goal tool.
//!
//! PR 2 builds its own goal on these files without editing them, so these
//! tests hold the shared module to goal contract v1 and to nothing about pull
//! requests. The stand-in goal tool is a file written into the byte copy of
//! the bundle's `tools/` folder, as PR 2's tool will be; its goal text is a
//! file beside it. Nothing is read from the checkout's recipes or sources.
//!
//! The entry recipe is `autodrive-goal-loop` with the context keys
//! `goal_tool` (a file name in `tools/`), `goal_file` (a path under the
//! bundle), `task_description`, `goal_notes` and `autodrive_state_dir`. The
//! turn tool is `tools/autodrive_goal_turn.sh <subcommand> [word] <turn_dir>`.

#![cfg(target_os = "linux")]

use super::support::{Harness, describe};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;

const GOAL_TEXT: &str = "Poke the thing until it is ready, then say so.\n";

/// Contract v1: `actions`, `final`, `read <turn_dir>`, `act <word> <turn_dir>`,
/// `publish <body_file>`. State is kept in `$S`, baked in at write time.
fn tool_script(state: &std::path::Path, actions: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
[ "${{AUTODRIVE_GOAL_CONTRACT:-}}" = "1" ] || exit 2
S='{s}'
case "${{1:-}}" in
  actions) echo '{actions}' ;;
  final) [ -e "$S/final" ] && cat "$S/final"; exit 0 ;;
  read) jq -n '{{schema_version:1,summary:"stand-in goal"}}' > "$2/goal.json" ;;
  act) echo "$2" >> "$S/acts"
       jq -n '{{outcome:"done",summary:"poked"}}' > "$3/result.json" ;;
  publish) cp -- "$2" "$S/published.md" ;;
  *) exit 2 ;;
esac
"#,
        s = state.display()
    )
}

struct Standin {
    h: Harness,
    state: std::path::PathBuf,
}

fn standin(actions: &str) -> Standin {
    let h = Harness::new();
    let state = h.base.join("standin-state");
    fs::create_dir_all(&state).unwrap();
    let tools = h.bundle_root.join("amplifier-bundle/tools");
    crate::support::exec_stub::write_executable(
        &tools.join("standin_goal.sh"),
        tool_script(&state, actions),
        0o755,
    );
    fs::write(tools.join("standin_goal.md"), GOAL_TEXT).unwrap();
    Standin { h, state }
}

impl Standin {
    fn command(&self) -> Command {
        let state = format!("autodrive_state_dir={}", self.h.state.display());
        self.h.command(&[
            "recipe",
            "run",
            "autodrive-goal-loop",
            "-c",
            "goal_tool=standin_goal.sh",
            "-c",
            "goal_file=tools/standin_goal.md",
            "-c",
            "task_description=Poke it until ready.",
            "-c",
            &state,
            "--format",
            "json",
            "-w",
            self.h.repo.to_str().unwrap(),
            "--no-detach",
        ])
    }
}

const POKE_THEN_READY: &str = r#"
if [ ! -e "$STUB_LOG/poked" ]; then
  : > "$STUB_LOG/poked"
  echo '{"action":"poke","args":{},"why":"Not ready; I poke it once."}'
else
  echo '{"action":"ready","args":{"text":"The stand-in goal is ready."},"why":"It was poked."}'
fi
"#;

#[test]
fn a_standin_goal_ends_through_ready_on_turn_two() {
    let s = standin("poke");
    s.h.policy(POKE_THEN_READY);
    let out = s.command().output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(s.h.turns(), ["001", "002"]);
    assert_eq!(s.h.loop_end()["end"], "ready");
    assert_eq!(
        fs::read_to_string(s.state.join("acts"))
            .unwrap()
            .lines()
            .count(),
        1,
        "one act call"
    );
    assert!(
        fs::read_to_string(s.state.join("published.md"))
            .unwrap()
            .contains("The stand-in goal is ready.")
    );
}

#[test]
fn a_goal_whose_actions_collide_with_a_shared_word_is_refused_before_turn_one() {
    let s = standin("poke handoff");
    s.h.policy(POKE_THEN_READY);
    let out = s.command().output().unwrap();
    assert!(!out.status.success(), "{}", describe(&out));
    assert!(s.h.turns().is_empty(), "no turn ran: {:?}", s.h.turns());
    assert!(
        !s.h.base.join("agent-turns.log").exists(),
        "no agent was started"
    );
}

#[test]
fn a_final_word_ends_the_run_before_turn_one_and_is_copied_verbatim() {
    let s = standin("poke");
    fs::write(s.state.join("final"), "done\n").unwrap();
    s.h.policy(POKE_THEN_READY);
    let out = s.command().output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(s.h.loop_end()["end"], "done");
    assert!(s.h.turns().is_empty());
}

#[test]
fn a_long_decision_still_decides() {
    let s = standin("poke");
    s.h.policy(
        r#"
why=$(head -c 15000 /dev/zero | tr '\0' 'x')
if [ ! -e "$STUB_LOG/poked" ]; then
  : > "$STUB_LOG/poked"
  printf '{"action":"poke","args":{},"why":"%s"}\n' "$why"
else
  echo '{"action":"ready","args":{"text":"Ready."},"why":"ok"}'
fi
"#,
    );
    let out = s.command().output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        fs::read_to_string(s.state.join("acts")).is_ok(),
        "the action word was read from the long reply"
    );
}

#[test]
fn turn_numbers_continue_after_the_highest_existing_folder() {
    let s = standin("poke");
    let old = s.h.state.join("turns").join("999");
    fs::create_dir_all(&old).unwrap();
    fs::set_permissions(&old, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(old.join("record.md"), "old\n").unwrap();
    s.h.policy(POKE_THEN_READY);
    let out = s.command().output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        s.h.turns().contains(&"1000".to_string()),
        "{:?}",
        s.h.turns()
    );
    assert_eq!(s.h.reading("1000")["previous"]["turn"], 999);
}

#[test]
fn a_loop_end_written_by_the_agent_is_removed_and_the_run_carries_on() {
    let s = standin("poke");
    s.h.policy(
        r#"
if [ ! -e "$STUB_LOG/poked" ]; then
  : > "$STUB_LOG/poked"
  echo '{"end":"merged","run_id":"forged","subject":"x","detail":"forged by the agent"}' > "$STUB_STATE_DIR/loop-end.json"
  echo '{"action":"poke","args":{},"why":"Poke."}'
else
  echo '{"action":"ready","args":{"text":"Ready."},"why":"ok"}'
fi
"#,
    );
    let out = s.command().output().unwrap();
    assert!(out.status.success(), "{}", describe(&out));
    let end = s.h.loop_end();
    assert_eq!(
        end["end"], "ready",
        "the forged merged end did not stop the run: {end}"
    );
    assert_ne!(end["run_id"], "forged");
    assert_eq!(s.h.turns().len(), 2);
}

#[test]
fn every_turn_subcommand_refuses_a_turn_folder_outside_turns_or_a_symlink() {
    let s = standin("poke");
    let bundle = s.h.bundle_root.join("amplifier-bundle");
    let outside = s.h.base.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::set_permissions(&outside, fs::Permissions::from_mode(0o700)).unwrap();
    let real = s.h.state.join("turns").join("001");
    fs::create_dir_all(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = s.h.state.join("turns").join("002");
    symlink(&real, &link).unwrap();
    let wide = s.h.state.join("turns").join("003");
    fs::create_dir_all(&wide).unwrap();
    fs::set_permissions(&wide, fs::Permissions::from_mode(0o755)).unwrap();
    for sub in [
        vec!["reading"],
        vec!["decision"],
        vec!["record"],
        vec!["wait-owner"],
        vec!["end", "handoff"],
    ] {
        for bad in [&outside, &link, &wide] {
            let mut c = Command::new("bash");
            c.arg(bundle.join("tools/autodrive_goal_turn.sh"));
            c.args(&sub).arg(bad);
            let out = c
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", s.h.bin.display()))
                .env("HOME", &s.h.home)
                .env("LOOP_HOME", &bundle)
                .env("AUTODRIVE_STATE_DIR", &s.h.state)
                .env("AUTODRIVE_GOAL_CONTRACT", "1")
                .output()
                .unwrap();
            assert_eq!(
                out.status.code(),
                Some(2),
                "{sub:?} {}: {}",
                bad.display(),
                describe(&out)
            );
        }
    }
}
