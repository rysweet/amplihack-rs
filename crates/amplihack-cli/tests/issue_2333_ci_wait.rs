//! Issue #2333, acceptance A3: a run that is waiting for CI keeps waiting.
//!
//! A round that produces no commit because it is waiting on an external
//! system — a platform allowance window, or a check still running — is not
//! an absence of progress. The no-progress rule of #1743 stays, and the
//! merge-ready loop stays exempt from it; what must hold is that an external
//! wait is never counted as a lack of progress and never ends the run as
//! `STUCK` with `stop_reason` `no_progress`.
//!
//! Contract: docs/reference/autodrive-rest-reads.md#waiting-is-not-a-failed-criterion.
//!
//! Nothing here sleeps, retries or polls. Each case runs `autodrive_loop.sh`
//! once with a stub round and a stub evaluator, and asserts on what the loop
//! recorded.

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;

use crate::support::auto_drive_light::{
    FAKE_GH, FAKE_GIT, jq_available, private_dir, repo_root, show, stderr, stdout,
};

/// The text `autodrive_progress.sh` prints when it stops a loop in code.
const DECIDED: &str = "no progress (decided in code)";

/// Plan line per round label, the last repeating:
/// `VERDICT|findings,…|ci_signal|rc`. The record shape is main's after
/// #2637: the verdict field, the head, the round label and the CI signal.
const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
set -u
S="$STUB_STATE"
case "${1:-} ${2:-}" in
  "recipe show") exit 0 ;;
  "orch helper")
    case "${3:-}" in autodrive-completion-*) exec "$REAL_AMPLIHACK" "$@" ;; esac
    in="$(cat)"
    if [ "${3:-}" = "extract-json" ]; then
      f=""; while [ $# -gt 0 ]; do [ "$1" = "--require-field" ] && f="${2:-}"; shift; done
      printf '%s\n' "$in" | jq -R -c --arg f "$f" 'fromjson? | select(type == "object" and has($f))' 2>/dev/null | tail -n 1
      exit 0
    fi
    if [ "${3:-}" = "extract-field" ]; then
      f=""; d=""
      while [ $# -gt 0 ]; do
        case "$1" in --field) f="${2:-}" ;; --default) d="${2:-}" ;; esac; shift
      done
      v=""
      [ -n "$in" ] && v="$(printf '%s' "$in" | jq -r --arg f "$f" 'if .[$f] == null then empty else .[$f] end | if type == "string" then . else tojson end' 2>/dev/null)"
      if [ -n "$v" ]; then printf '%s' "$v"; else printf '%s' "$d"; fi
      exit 0
    fi
    exit 0 ;;
  "recipe run") ;;
  *) exit 0 ;;
esac
n=$(cat "$S/calls" 2>/dev/null || echo 0); n=$((n + 1)); printf '%s' "$n" > "$S/calls"
name="$(basename -- "${3:-}" .yaml)"
printf '%s\0' "recipe" "run" "$name" "${@:4}" > "$STUB_LOG/call-$(printf '%05d' "$n")"
if [ "$name" = "loop-health-evaluator" ]; then
  label=""; prev=""; head_read=""
  for a in "$@"; do
    case "$prev:$a" in
      -c:loop_round_label=*) label="${a#loop_round_label=}" ;;
      --context-from-file:loop_round_label=*) label="$(cat -- "${a#*=}")" ;;
      --context-from-file:loop_head_read=*) head_read="$(cat -- "${a#*=}")" ;;
      -c:loop_head_read=*) head_read="${a#loop_head_read=}" ;;
    esac
    prev=$a
  done
  printf '%s\t%s\n' "$label" "$head_read" >> "$S/evaluator-calls"
  h=$(wc -l < "$S/evaluator-calls")
  if [ "$h" -gt "${STUB_EVALUATOR_ROUNDS:-4}" ]; then echo "LOOP_HEALTH: STUCK"; exit 1; fi
  v=CONTINUE
  d="${STUB_EVALUATOR_DONE_AFTER:-0}"
  [ "$d" != 0 ] && [ "$h" -ge "$d" ] && v=DONE
  printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: %s\n' "$v"
  exit 0
fi
record=""; label=""; wd=""; prev=""
for a in "$@"; do
  [ "$prev" = "--working-dir" ] && wd="$a"
  if [ "$prev" = "-c" ]; then
    case "$a" in
      autodrive_round_record=*) record="${a#autodrive_round_record=}" ;;
      autodrive_round_label=*) label="${a#autodrive_round_label=}" ;;
    esac
  fi
  prev=$a
done
printf '%s\n' "$label" >> "$S/round-calls"
r="${label#round-}"
line="$(sed -n "${r}p" "$S/plan")"; [ -n "$line" ] || line="$(tail -n 1 "$S/plan")"
IFS='|' read -r verdict items ci rc <<<"$line"
head="$(git -C "$wd" rev-parse HEAD)"
field=crusty_verdict; [ "$name" = "autodrive-merge-round" ] && field=merge_ready_verdict
( umask 077
  printf '{"%s":"%s","head_sha":"%s","reviewed_head_sha":"%s","round_label":"%s","ci_signal":"%s"}\n' \
    "$field" "$verdict" "$head" "$head" "$label" "${ci:-}" > "$record"
  : > "$record.findings"
  if [ -n "${items:-}" ]; then printf '%s\n' "$items" | tr ',' '\n' > "$record.findings"; fi )
if [ -n "${AUTODRIVE_COMPLETION_STATE:-}" ] && [ -f "$record" ]; then
  "$REAL_AMPLIHACK" orch helper autodrive-completion-seal --private-state "$AUTODRIVE_COMPLETION_STATE" || exit 2
  action=evaluate
  case "$verdict" in CLEAN|MINOR_ONLY) action=complete ;; esac
  "$REAL_AMPLIHACK" orch helper autodrive-completion-write --private-state "$AUTODRIVE_COMPLETION_STATE" --action "$action" --reason 'Round evidence determines the next action' || exit 2
fi
echo "round ran"
exit "${rc:-0}"
"#;

struct Loop {
    tmp: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
    stub: PathBuf,
    state: PathBuf,
    repo: PathBuf,
    cwd: PathBuf,
    task: PathBuf,
}

fn git(dir: &Path, home: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", show(&out));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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
        let (bin, log, stub, state, repo, cwd, task) = (
            p("bin"),
            p("log"),
            p("stub-state"),
            p("state"),
            p("repo"),
            p("cwd"),
            p("task-in"),
        );
        for d in [&bin, &log, &stub, &cwd] {
            std::fs::create_dir_all(d).unwrap();
        }
        private_dir(&state);
        std::fs::write(&task, "issue 2333 fixture task\n").unwrap();
        git(&cwd, &root, &["init", "-q"]);
        let tree = git(&cwd, &root, &["write-tree"]);
        let commit = git(&cwd, &root, &["commit-tree", &tree, "-m", "init"]);
        git(&cwd, &root, &["update-ref", "HEAD", &commit]);
        git(
            &cwd,
            &root,
            &["worktree", "add", "-q", "--detach", repo.to_str().unwrap()],
        );
        crate::support::exec_stub::write_executable(&bin.join("amplihack"), AMPLIHACK_STUB, 0o755);
        crate::support::exec_stub::write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        std::fs::write(log.join("gh.log"), "").unwrap();
        Some(Loop {
            tmp,
            bin,
            log,
            stub,
            state,
            repo,
            cwd,
            task,
        })
    }

    /// Install the fake `git`, for the cases that drive the head read.
    fn with_fake_git(&self) -> &Self {
        crate::support::exec_stub::write_executable(&self.bin.join("git"), FAKE_GIT, 0o755);
        self
    }

    fn plan(&self, lines: &[&str]) {
        std::fs::write(self.stub.join("plan"), lines.join("\n") + "\n").unwrap();
    }

    fn head(&self) -> String {
        git(&self.repo, self.tmp.path(), &["rev-parse", "HEAD"])
    }

    fn run(&self, loop_name: &str, env: &[(&str, &str)]) -> Output {
        let (recipe, token, field) = match loop_name {
            "crusty" => ("autodrive-crusty-round", "CLEAN", "crusty_verdict"),
            _ => (
                "autodrive-merge-round",
                "MERGE_READY",
                "merge_ready_verdict",
            ),
        };
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        );
        let mut c = crate::isolated_amplihack::isolated("bash");
        c.arg(repo_root().join("amplifier-bundle/tools/autodrive_loop.sh"))
            .args(["--loop-name", loop_name, "--round-recipe", recipe])
            .args(["--clean-token", token, "--verdict-field", field])
            .arg("--repo")
            .arg(&self.repo)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--task-file")
            .arg(&self.task)
            .args(["--context", "repo_path=.", "--context", "pr_number=7"])
            .env("PATH", path)
            .env("STUB_LOG", &self.log)
            .env("STUB_STATE", &self.stub)
            .env("FAKE_LOG", &self.log)
            .env("FAKE_BIN", &self.bin)
            .env("FAKE_GH_HEAD", self.head())
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env(
                "REAL_AMPLIHACK",
                crate::support::amplihack_bin::amplihack_bin(),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&self.cwd);
        for (k, v) in env {
            c.env(k, v);
        }
        c.output().unwrap()
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.stub.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn state_file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.state.join(name)).ok()
    }
}

fn result(o: &Output) -> Value {
    stdout(o)
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("loop_result").is_some())
        .unwrap_or_else(|| panic!("no loop_result line\n{}", show(o)))
}

/// The run was never stopped by the rule in code. Termination stays the
/// agentic evaluation's to decide, so an evaluator that says `STUCK` is not
/// what this asserts on: what must not happen is a stop decided in code
/// because the head did not move while an external system was being waited
/// on. Such a stop writes `<loop>-stuck-reason.txt`, prints [`DECIDED`], and
/// reaches the report as `stop_reason` `no_progress`.
fn assert_never_stopped_in_code(l: &Loop, o: &Output, loop_name: &str) {
    assert!(
        !stderr(o).contains(DECIDED) && !stdout(o).contains(DECIDED),
        "a wait is not an absence of progress: {}",
        show(o)
    );
    assert!(
        l.state_file(&format!("{loop_name}-stuck-reason.txt"))
            .is_none(),
        "the rule in code wrote a stuck reason for a wait: {}",
        show(o)
    );
    let saved = l
        .state_file(&format!("{loop_name}-result.json"))
        .expect("the loop writes its result");
    let saved: Value = serde_json::from_str(saved.trim()).unwrap();
    assert!(
        !saved
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .contains(DECIDED),
        "{saved}"
    );
}

// ── A3: a merge-ready run waiting for CI keeps waiting ──────────────────────

/// The 11 Nov 2026 shape: three rounds at the same head because CI is still
/// running, then the checks go green. The head did not move and no commit
/// was made in those three rounds, and that is not a reason to stop. The run
/// keeps waiting and merges when the checks settle.
#[test]
fn a_merge_ready_run_waiting_for_ci_at_an_unchanged_head_keeps_waiting() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[
        "NOT_MERGE_READY|ci-pending|PENDING|0",
        "NOT_MERGE_READY|ci-pending|PENDING|0",
        "NOT_MERGE_READY|ci-pending|PENDING|0",
        "MERGE_READY||PASS|0",
    ]);
    // The evaluator keeps the run going while the checks run, and ends it
    // once the fourth round reports the checks green.
    let o = l.run(
        "merge-ready",
        &[
            ("STUB_EVALUATOR_ROUNDS", "99"),
            ("STUB_EVALUATOR_DONE_AFTER", "4"),
        ],
    );
    assert_never_stopped_in_code(&l, &o, "merge-ready");
    assert_eq!(
        l.lines("round-calls").len(),
        4,
        "the run reads the checks again rather than giving up: {}",
        show(&o)
    );
    let r = result(&o);
    assert_eq!(
        r["loop_result"],
        "DONE",
        "a run that waited for CI merges when CI goes green: {}",
        show(&o)
    );
    assert_ne!(r["loop_result"], "STUCK", "{}", show(&o));
}

/// Termination stays the evaluator's to decide. When it does stop the run,
/// the reason recorded is its own, never the rule in code — so the report
/// reads `handoff`, not `no_progress`.
#[test]
fn a_stop_during_a_ci_wait_is_the_evaluators_and_says_so() {
    let Some(l) = Loop::new() else { return };
    l.plan(&["NOT_MERGE_READY|ci-pending|PENDING|0"]);
    let o = l.run("merge-ready", &[("STUB_EVALUATOR_ROUNDS", "2")]);
    assert_never_stopped_in_code(&l, &o, "merge-ready");
    let r = result(&o);
    assert_eq!(
        r["loop_result"],
        "STUCK",
        "the stub evaluator stops this run: {}",
        show(&o)
    );
    assert!(
        r["reason"]
            .as_str()
            .is_some_and(|s| s.contains("loop-health-evaluator")),
        "the stop names the evaluator as its source: {r}"
    );
    assert!(
        l.lines("evaluator-calls").len() > 1,
        "every round was judged by the agentic evaluation: {}",
        show(&o)
    );
}

/// The narrow rule, on its own: two rounds alike in every way except that CI
/// is still running are not a stop. `autodrive_progress.sh` is unchanged by
/// #2333; this holds it to that.
#[test]
fn the_no_progress_rule_does_not_stop_on_a_running_check() {
    let Some(l) = Loop::new() else { return };
    let tool = repo_root().join("amplifier-bundle/tools/autodrive_progress.sh");
    let findings = l.state.join("f");
    std::fs::write(&findings, "ci-pending\n").unwrap();
    let head = l.head();
    let record = |label: &str, ci: &str| {
        let o = std::process::Command::new("bash")
            .arg(&tool)
            .args(["record"])
            .arg(&l.state)
            .args(["merge-ready", label])
            .arg(&head)
            .args(["0"])
            .arg(&findings)
            .arg(ci)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", show(&o));
    };
    let check = |label: &str, ci: &str| {
        std::process::Command::new("bash")
            .arg(&tool)
            .args(["check"])
            .arg(&l.state)
            .args(["merge-ready", label])
            .arg(&head)
            .arg(&findings)
            .arg(ci)
            .output()
            .unwrap()
    };
    record("round-1", "PENDING");
    let o = check("round-2", "PENDING");
    assert_eq!(
        o.status.code(),
        Some(0),
        "a check still running is not a stop: {}",
        show(&o)
    );
    assert!(!stdout(&o).contains(DECIDED), "{}", show(&o));

    // The rule itself is intact: the same two rounds with CI settled do stop.
    record("round-2", "RED:1/1");
    let o = check("round-3", "RED:1/1");
    assert_eq!(
        o.status.code(),
        Some(10),
        "#1743's rule is not removed: {}",
        show(&o)
    );
}

// ── A2, at the loop: an unavailable head read is not INVALID_DECISION ───────

/// The crusty loop reads the completion dispatch's `evaluator_invoked`. An
/// unavailable head read gives `true`, so the loop goes on to the evaluator
/// rather than escalating `INVALID_DECISION` — which is what stopped the
/// merge loop with `crusty-rereview-not-done` on 12 Nov 2026.
#[test]
fn an_unavailable_head_read_does_not_escalate_invalid_decision() {
    let Some(l) = Loop::new() else { return };
    l.with_fake_git();
    git(
        &l.repo,
        l.tmp.path(),
        &["remote", "add", "origin", "https://github.com/o/r"],
    );
    l.plan(&["CLEAN||PASS|0"]);
    let o = l.run(
        "crusty",
        &[
            ("FAKE_GH_GRAPHQL_UNAVAILABLE", "1"),
            ("FAKE_GIT_LS_REMOTE", "fail"),
            ("FAKE_GH_FAIL", "repos/o/r/pulls/7"),
            ("STUB_EVALUATOR_ROUNDS", "2"),
        ],
    );
    let out = format!("{}{}", stdout(&o), stderr(&o));
    assert!(
        !out.contains("INVALID_DECISION"),
        "an unreadable head is not an invalid decision: {}",
        show(&o)
    );
    assert!(
        !out.contains("no evaluator fallback"),
        "the dispatch hands the round to the evaluator: {}",
        show(&o)
    );
    let calls = l.lines("evaluator-calls");
    assert!(!calls.is_empty(), "the evaluator ran: {}", show(&o));
    assert!(
        calls[0].ends_with("\tread-failed"),
        "with the fault in its context: {calls:?}"
    );
    assert_never_stopped_in_code(&l, &o, "crusty");
}

/// And when the head reads cleanly, the clean round still completes without
/// an evaluator call: the unavailable path adds an outcome, it does not
/// reroute the ordinary one.
#[test]
fn a_readable_matching_head_still_completes_the_crusty_loop() {
    let Some(l) = Loop::new() else { return };
    l.with_fake_git();
    git(
        &l.repo,
        l.tmp.path(),
        &["remote", "add", "origin", "https://github.com/o/r"],
    );
    l.plan(&["CLEAN||PASS|0"]);
    let o = l.run(
        "crusty",
        &[
            ("FAKE_GH_GRAPHQL_UNAVAILABLE", "1"),
            ("FAKE_GIT_LS_REMOTE", "answer"),
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let r = result(&o);
    assert_eq!(r["loop_result"], "DONE", "{}", show(&o));
    assert_eq!(r["evaluator_invoked"], false, "{}", show(&o));
    assert!(
        l.lines("evaluator-calls").is_empty(),
        "objective completion does not consult the evaluator"
    );
}

// ── Nothing sleeps, retries or polls ────────────────────────────────────────

/// The whole point of #2333 is that waiting is recognised, not waited out.
/// No tool on this path may sleep or loop to wait.
#[test]
fn no_tool_on_the_wait_path_sleeps_or_retries() {
    let tools = repo_root().join("amplifier-bundle/tools");
    for name in [
        "autodrive_progress.sh",
        "autodrive_escalate.sh",
        "autodrive_merge.sh",
        "autodrive_rest.sh",
    ] {
        let p = tools.join(name);
        let Ok(body) = std::fs::read_to_string(&p) else {
            panic!("{name} must exist on the merged tree");
        };
        for line in body.lines() {
            let code = line.split('#').next().unwrap_or_default();
            for banned in ["sleep ", "sleep\t", "read -t", "until ", "while true"] {
                assert!(
                    !code.contains(banned),
                    "{name}: `{banned}` is not a fix for a wait: {line}"
                );
            }
        }
    }
}
