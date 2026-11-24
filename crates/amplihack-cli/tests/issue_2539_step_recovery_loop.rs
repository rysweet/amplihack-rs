//! Issue #2539 and #2814, on the shared goal loop (#3129): every Crusty round
//! the loop's `review` action runs gets the run's `step_recovery_dir`, and a
//! round's `pre-review.*` baseline is reclaimed once the round has returned
//! and the loop goes on.
//!
//! The old auto-drive loops stopped after a round that left a recorded
//! handoff, and their loop-health evaluator stopped them as STUCK. #3129
//! removed both stops: a Crusty round that fails goes into the next turn's
//! reading and never ends the run (docs/reference/autodrive-goal-loop.md,
//! "Failures inside a turn"). The tests of those stops went with them.
//!
//! `autodrive_pr_goal.sh act review <turn_dir>` runs under `isolated("bash")`
//! with a stub `amplihack` as `AMPLIHACK_BIN`. The stub plays the Crusty round
//! recipe: it writes the round record and a `pre-review.<label>` baseline
//! beside it, and a round listed in `handoff-rounds` records a handoff with
//! the real `step-recovery record` in the `step_recovery_dir` it was handed,
//! then ends with the handoff exit. One action runs one round; nothing here
//! starts the loop driver.

#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support::auto_drive_light::{jq_available, show};
use crate::support::step_recovery::{private_dir, repo_root};

const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
set -u
S="$STUB_STATE"
[ "${1:-} ${2:-}" = "recipe run" ] || { echo "unexpected amplihack call: $*" >&2; exit 3; }
name="$(basename -- "${3:-}" .yaml)"
[ "$name" = "autodrive-crusty-round" ] || { echo "unexpected recipe: $name" >&2; exit 3; }
record=""; label=""; srd=""; head=""; prev=""
for a in "$@"; do
  if [ "$prev" = "-c" ]; then
    case "$a" in
      autodrive_round_record=*) record="${a#autodrive_round_record=}" ;;
      autodrive_round_label=*) label="${a#autodrive_round_label=}" ;;
      step_recovery_dir=*) srd="${a#step_recovery_dir=}" ;;
      reviewed_head_sha=*) head="${a#reviewed_head_sha=}" ;;
    esac
  fi
  prev=$a
done
printf '%s\t%s\n' "$label" "$srd" >> "$S/round-calls"
printf '{"crusty_verdict":"CONCERNS","head_sha":"%s","reviewed_head_sha":"%s","concern_count":1}\n' "$head" "$head" > "$record"
# #2808 SR-10 / #2814: the round's pre-review baseline, beside the round
# record, so a test can see which rounds have one reclaimed.
mkdir -p "$(dirname -- "$record")/pre-review.$label" || exit 2
if grep -qx "$label" "$S/handoff-rounds" 2>/dev/null; then
  "$REAL_AMPLIHACK" orch helper step-recovery record --dir "$srd" --step-id step-02-crusty-review \
    --reason "the review failed twice with the same error" --owner run-requester \
    --needs "read the round log in the state dir" || exit 2
  echo "round stopped at the handoff"; exit 81
fi
if grep -qx "$label" "$S/failed-rounds" 2>/dev/null; then echo "round failed"; exit 1; fi
echo "round ran"
"#;

const GH_STUB: &str = "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$STUB_STATE/gh-calls\"\nexit 0\n";

struct Goal {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    stub: PathBuf,
    state: PathBuf,
    recovery: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
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

impl Goal {
    fn new() -> Self {
        assert!(
            jq_available(),
            "jq is not installed; the goal tool needs it"
        );
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let p = |n: &str| root.join(n);
        for d in [p("bin"), p("stub"), p("repo"), p("home")] {
            fs::create_dir_all(d).unwrap();
        }
        private_dir(&p("state"));
        private_dir(&p("state").join("turns"));
        private_dir(&p("recovery"));
        fs::write(p("task"), "issue 2539 review fixture\n").unwrap();
        git(&p("repo"), &["init", "-q"]);
        let tree = git(&p("repo"), &["write-tree"]);
        let commit = git(&p("repo"), &["commit-tree", &tree, "-m", "init"]);
        git(&p("repo"), &["update-ref", "HEAD", &commit]);
        crate::support::exec_stub::write_executable(
            &p("bin").join("amplihack"),
            AMPLIHACK_STUB,
            0o755,
        );
        crate::support::exec_stub::write_executable(&p("bin").join("gh"), GH_STUB, 0o755);
        Self {
            stub: p("stub"),
            state: p("state"),
            recovery: p("recovery"),
            root,
            _tmp: tmp,
        }
    }

    /// A private turn folder `<state>/turns/<n>`, as the driver makes it.
    fn turn(&self, n: u32) -> PathBuf {
        let td = self.state.join("turns").join(format!("{n:03}"));
        private_dir(&td);
        td
    }

    /// `autodrive_pr_goal.sh act review <turn_dir>` under goal contract v1.
    fn review(&self, td: &Path) -> Output {
        let path = format!(
            "{}:/usr/local/bin:/usr/bin:/bin",
            self.root.join("bin").display()
        );
        crate::isolated_amplihack::isolated("bash")
            .arg(repo_root().join("amplifier-bundle/tools/autodrive_pr_goal.sh"))
            .args(["act", "review"])
            .arg(td)
            .env("PATH", path)
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("home"))
            .env("AUTODRIVE_GOAL_CONTRACT", "1")
            .env("AUTODRIVE_STATE_DIR", &self.state)
            .env("AUTODRIVE_REPO", self.root.join("repo"))
            .env("AUTODRIVE_WORKTREE", self.root.join("repo"))
            .env("AUTODRIVE_TASK_FILE", self.root.join("task"))
            .env("RECIPE_VAR_pr_number", "2539")
            .env("RECIPE_VAR_step_recovery_dir", &self.recovery)
            .env("STUB_STATE", &self.stub)
            .env("AMPLIHACK_BIN", self.root.join("bin/amplihack"))
            .env(
                "REAL_AMPLIHACK",
                crate::support::amplihack_bin::amplihack_bin(),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(self.root.join("repo"))
            .output()
            .expect("spawn autodrive_pr_goal.sh")
    }

    /// The `pre-review.*` baselines still in the state directory, sorted.
    fn baselines(&self) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(&self.state)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("pre-review."))
            .collect();
        found.sort();
        found
    }

    fn lines(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.stub.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// The action's `result.json` outcome word.
fn outcome(td: &Path, o: &Output) -> String {
    let text = fs::read_to_string(td.join("result.json"))
        .unwrap_or_else(|e| panic!("no result.json ({e})\n{}", show(o)));
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    v["outcome"].as_str().unwrap_or_default().to_string()
}

/// #2539: every round the loop starts gets the run's `step_recovery_dir`, so
/// a failure repaired in one round is a recurrence in a later one.
#[test]
fn every_review_round_gets_the_runs_step_recovery_dir() {
    let g = Goal::new();
    for n in [1, 2] {
        let td = g.turn(n);
        let o = g.review(&td);
        assert_eq!(o.status.code(), Some(0), "{}", show(&o));
        assert_eq!(outcome(&td, &o), "reviewed", "{}", show(&o));
    }
    let rounds = g.lines("round-calls");
    assert_eq!(rounds.len(), 2, "one round per review action: {rounds:?}");
    for row in &rounds {
        assert!(
            row.ends_with(&format!("\t{}", g.recovery.display())),
            "every round gets the run's step_recovery_dir: {row}"
        );
    }
}

/// #2808 SR-10, #2814: a round the loop goes on from leaves no baseline
/// behind, whatever it returned. Since #3129 the loop goes on from every
/// round that does not end the run with exit 79 or 75, including one that
/// recorded a handoff: that round's failure is the next turn's reading.
#[test]
fn a_review_round_the_loop_goes_on_from_leaves_no_pre_review_baseline() {
    let g = Goal::new();
    fs::write(g.stub.join("failed-rounds"), "turn-002\n").unwrap();
    fs::write(g.stub.join("handoff-rounds"), "turn-003\n").unwrap();
    for (n, want) in [(1, "reviewed"), (2, "review-failed"), (3, "review-failed")] {
        let td = g.turn(n);
        let o = g.review(&td);
        assert_eq!(o.status.code(), Some(0), "turn {n}: {}", show(&o));
        assert_eq!(outcome(&td, &o), want, "turn {n}: {}", show(&o));
        assert!(
            g.baselines().is_empty(),
            "turn {n}: the round's baseline must be reclaimed: {:?} {}",
            g.baselines(),
            show(&o)
        );
    }
    assert_eq!(
        g.lines("round-calls").len(),
        3,
        "the fixture must run rounds"
    );
    // The handoff round really recorded one.
    assert!(
        g.recovery.join("step-02-crusty-review.jsonl").is_file(),
        "the third round recorded its handoff"
    );
}
