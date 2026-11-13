//! Issue #2539: an auto-drive loop stops after a round that left a recorded
//! handoff, so no later round or evaluator meets the same failure again.
//!
//! `autodrive_loop.sh` runs under `isolated("bash")` with a stub `amplihack`
//! first on `PATH`. The stub plays the round recipe and the loop-health
//! evaluator and passes every `orch helper step-recovery` call, and the
//! auto-drive completion helpers, to the real binary. A round listed in
//! `handoff-rounds` records a handoff with the real `step-recovery record`
//! in the `step_recovery_dir` the loop handed it. Each round commits, so the
//! no-progress rule never stops the loop first.

#![cfg(unix)]

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use crate::support::auto_drive_light::{jq_available, show, stdout};
use crate::support::step_recovery::{HANDOFF_EXIT, private_dir, repo_root};

const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
set -u
S="$STUB_STATE"
case "${1:-} ${2:-}" in
  "recipe show") exit 0 ;;
  "orch helper")
    case "${3:-}" in step-recovery|autodrive-completion-*) exec "$REAL_AMPLIHACK" "$@" ;; esac
    in="$(cat)"
    if [ "${3:-}" = "extract-json" ]; then
      f=""; while [ $# -gt 0 ]; do [ "$1" = "--require-field" ] && f="${2:-}"; shift; done
      printf '%s\n' "$in" | jq -R -c --arg f "$f" 'fromjson? | select(type == "object" and has($f))' 2>/dev/null | tail -n 1
      exit 0
    fi
    if [ "${3:-}" = "extract-field" ]; then
      f=""; d=""
      while [ $# -gt 0 ]; do case "$1" in --field) f="${2:-}" ;; --default) d="${2:-}" ;; esac; shift; done
      v=""
      [ -n "$in" ] && v="$(printf '%s' "$in" | jq -r --arg f "$f" 'if .[$f] == null then empty else .[$f] end | if type == "string" then . else tojson end' 2>/dev/null)"
      if [ -n "$v" ]; then printf '%s' "$v"; else printf '%s' "$d"; fi
      exit 0
    fi
    exit 0 ;;
  "recipe run") ;;
  *) exit 0 ;;
esac
name="$(basename -- "${3:-}" .yaml)"
if [ "$name" = "loop-health-evaluator" ]; then
  label=""; prev=""
  for a in "$@"; do case "$prev:$a" in -c:loop_round_label=*) label="${a#loop_round_label=}" ;; esac; prev=$a; done
  printf '%s\n' "$label" >> "$S/evaluator-calls"
  if [ "$(wc -l < "$S/evaluator-calls")" -ge 3 ]; then echo "LOOP_HEALTH: STUCK"; exit 1; fi
  printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: CONTINUE\n'
  exit 0
fi
record=""; label=""; wd=""; srd=""; prev=""
for a in "$@"; do
  [ "$prev" = "--working-dir" ] && wd="$a"
  if [ "$prev" = "-c" ]; then
    case "$a" in
      autodrive_round_record=*) record="${a#autodrive_round_record=}" ;;
      autodrive_round_label=*) label="${a#autodrive_round_label=}" ;;
      step_recovery_dir=*) srd="${a#step_recovery_dir=}" ;;
    esac
  fi
  prev=$a
done
printf '%s\t%s\n' "$label" "$srd" >> "$S/round-calls"
t="$(git -C "$wd" write-tree)"
c="$(git -C "$wd" -c user.name=t -c user.email=t@t commit-tree "$t" -p HEAD -m "stub $label")"
git -C "$wd" update-ref HEAD "$c"
printf '{"merge_ready_verdict":"CONCERNS","head_sha":"%s","reviewed_head_sha":"%s","round_label":"%s"}\n' "$c" "$c" "$label" > "$record"
printf 'item-%s\n' "$label" > "$record.findings"
if grep -qx "$label" "$S/handoff-rounds" 2>/dev/null; then
  "$REAL_AMPLIHACK" orch helper step-recovery record --dir "$srd" --step-id step-00-merge-ready-files \
    --reason "merge-ready files refused twice with the same error" --owner run-requester \
    --needs "check the merge-ready files in the state dir" || exit 2
fi
if grep -qx "$label" "$S/corrupt-rounds" 2>/dev/null; then
  printf '{not json\n' > "$srd/step-00-merge-ready-files.jsonl"; chmod 600 "$srd/step-00-merge-ready-files.jsonl"
fi
echo "round ran"
"#;

const GH_STUB: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$STUB_STATE/gh-calls\"\n[ \"$1 $2\" = 'pr view' ] && git -C \"$STUB_REPO\" rev-parse HEAD\nexit 0\n";

struct Loop {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    stub: PathBuf,
    recovery: PathBuf,
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let o = amplihack_git::command_in(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

impl Loop {
    fn new() -> Option<Self> {
        if !jq_available() {
            eprintln!("SKIP: jq is not installed; the stub orch helper needs it");
            return None;
        }
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let p = |n: &str| root.join(n);
        for d in [p("bin"), p("stub"), p("cwd"), p("home")] {
            fs::create_dir_all(d).unwrap();
        }
        private_dir(&p("state"));
        private_dir(&p("recovery"));
        fs::write(p("task"), "issue 2539 loop fixture\n").unwrap();
        git(&p("cwd"), &["init", "-q"]);
        let tree = git(&p("cwd"), &["write-tree"]);
        let commit = git(&p("cwd"), &["commit-tree", &tree, "-m", "init"]);
        git(&p("cwd"), &["update-ref", "HEAD", &commit]);
        git(
            &p("cwd"),
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                p("repo").to_str().unwrap(),
            ],
        );
        crate::support::exec_stub::write_executable(
            &p("bin").join("amplihack"),
            AMPLIHACK_STUB,
            0o755,
        );
        crate::support::exec_stub::write_executable(&p("bin").join("gh"), GH_STUB, 0o755);
        Some(Self {
            stub: p("stub"),
            recovery: p("recovery"),
            root,
            _tmp: tmp,
        })
    }

    fn run(&self) -> Output {
        let path = format!(
            "{}:/usr/local/bin:/usr/bin:/bin",
            self.root.join("bin").display()
        );
        crate::isolated_amplihack::isolated("bash")
            .arg(repo_root().join("amplifier-bundle/tools/autodrive_loop.sh"))
            .args([
                "--loop-name",
                "merge-ready",
                "--round-recipe",
                "autodrive-merge-round",
            ])
            .args([
                "--clean-token",
                "MERGE_READY",
                "--verdict-field",
                "merge_ready_verdict",
            ])
            .arg("--repo")
            .arg(self.root.join("repo"))
            .arg("--state-dir")
            .arg(self.root.join("state"))
            .arg("--task-file")
            .arg(self.root.join("task"))
            .args(["--context", "repo_path=.", "--context", "pr_number=2539"])
            .arg("--context")
            .arg(format!("step_recovery_dir={}", self.recovery.display()))
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("STUB_STATE", &self.stub)
            .env("STUB_REPO", self.root.join("repo"))
            .env("AMPLIHACK_BIN", self.root.join("bin/amplihack"))
            .env(
                "REAL_AMPLIHACK",
                crate::support::amplihack_bin::amplihack_bin(),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(self.root.join("cwd"))
            .output()
            .expect("spawn autodrive_loop.sh")
    }

    fn lines(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.stub.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

fn loop_result(o: &Output) -> serde_json::Value {
    stdout(o)
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v.get("loop_result").is_some())
        .unwrap_or_else(|| panic!("no loop_result line\n{}", show(o)))
}

#[test]
fn a_round_that_records_a_handoff_stops_the_loop_with_exit_81() {
    let Some(l) = Loop::new() else { return };
    fs::write(l.stub.join("handoff-rounds"), "round-2\n").unwrap();
    let o = l.run();
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    let r = loop_result(&o);
    assert_eq!(r["loop_result"], "STEP_RECOVERY_HANDOFF", "{}", show(&o));
    let all = format!("{}{}", stdout(&o), String::from_utf8_lossy(&o.stderr));
    assert!(
        all.contains("step-00-merge-ready-files.jsonl"),
        "names the record:\n{all}"
    );

    let rounds = l.lines("round-calls");
    let labels: Vec<&str> = rounds
        .iter()
        .map(|r| r.split('\t').next().unwrap())
        .collect();
    assert_eq!(labels, ["round-1", "round-2"], "no round after the handoff");
    for row in &rounds {
        assert!(
            row.ends_with(&format!("\t{}", l.recovery.display())),
            "every round gets the run's step_recovery_dir: {row}"
        );
    }
    assert_eq!(
        l.lines("evaluator-calls"),
        ["round-1"],
        "no evaluator after the handoff round"
    );
}

#[test]
fn a_handoff_in_the_first_round_stops_before_any_evaluator() {
    let Some(l) = Loop::new() else { return };
    fs::write(l.stub.join("handoff-rounds"), "round-1\n").unwrap();
    let o = l.run();
    assert_eq!(o.status.code(), Some(HANDOFF_EXIT), "{}", show(&o));
    assert_eq!(l.lines("round-calls").len(), 1);
    assert!(l.lines("evaluator-calls").is_empty());
}

#[test]
fn invalid_recovery_records_stop_the_loop_with_exit_2() {
    let Some(l) = Loop::new() else { return };
    fs::write(l.stub.join("corrupt-rounds"), "round-1\n").unwrap();
    let o = l.run();
    assert_eq!(o.status.code(), Some(2), "{}", show(&o));
    assert_eq!(
        loop_result(&o)["loop_result"],
        "INVALID_RECOVERY_RECORDS",
        "{}",
        show(&o)
    );
    assert!(l.lines("evaluator-calls").is_empty());
}

#[test]
fn without_a_handoff_the_loop_runs_on_to_the_evaluators_stop() {
    let Some(l) = Loop::new() else { return };
    let o = l.run();
    assert_eq!(
        o.status.code(),
        Some(1),
        "evaluator STUCK ends the loop as before:\n{}",
        show(&o)
    );
    assert_eq!(l.lines("evaluator-calls").len(), 3);
    assert_ne!(loop_result(&o)["loop_result"], "STEP_RECOVERY_HANDOFF");
}
