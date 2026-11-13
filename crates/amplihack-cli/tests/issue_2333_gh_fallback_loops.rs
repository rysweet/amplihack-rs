//! T3, T6 and the WAITING loop result (D8, D9, D11), end to end through
//! `autodrive_loop.sh` with a stub round, a stub evaluator and the #2333
//! fake `gh`. Same harness shape as issue_1743_autodrive_budget.rs.
//!
//! The stub round writes the record fields `round_record` will write
//! (`waits`, `wait_target`, `round_waiting`, `merge_state`); the real
//! classifier that fills them is covered in the `waits` and `steps` modules.

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;

use crate::support::auto_drive_light::{
    jq_available, private_dir, repo_root, show, stderr, stdout,
};
use crate::support::autodrive_gh_fake::{FAKE_GH, write_fixtures};

const DECIDED: &str = "no progress (decided in code)";
const EPOCH: &str = "1794528000";

/// Plan line per round label (the last repeats):
/// `VERDICT|findings,…|waits-json|wait_target|round_waiting|rc|sidecar_reason`.
/// `@HEAD` in the target becomes the head SHA. A non-empty sidecar reason
/// writes `<record>.wait`, as crusty step-01 does when the head is
/// unavailable, and no record.
const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
set -u
S="$STUB_STATE"
case "${1:-} ${2:-}" in
  "recipe show") exit 0 ;;
  "orch helper")
    case "${3:-}" in autodrive-completion-*|autodrive-gh-*|autodrive-round-waits) exec "$REAL_AMPLIHACK" "$@" ;; esac
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
  label=""; prev=""
  for a in "$@"; do
    case "$prev:$a" in
      -c:loop_round_label=*) label="${a#loop_round_label=}" ;;
      --context-from-file:loop_round_label=*) label="$(cat -- "${a#*=}")" ;;
    esac
    prev=$a
  done
  printf '%s\n' "$label" >> "$S/evaluator-calls"
  h=$(wc -l < "$S/evaluator-calls")
  if [ "$h" -gt 4 ]; then echo "LOOP_HEALTH: STUCK"; exit 1; fi
  printf '  \342\234\223 step-04-enforce-loop-verdict: completed\n    Output: LOOP_HEALTH: CONTINUE\n'
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
IFS='|' read -r verdict items waits target waiting rc sidecar <<<"$line"
head="$(git -C "$wd" rev-parse HEAD)"
target="${target//@HEAD/@$head}"
if [ -n "${sidecar:-}" ]; then
  ( umask 077; t="$(mktemp "${record}.wait.XXXXXX")"
    printf '{"round_waiting":true,"wait_reason":"%s","wait_target":"%s"}\n' "$sidecar" "$target" > "$t"
    mv -f -- "$t" "${record}.wait" )
  echo "crusty-pr-head-unreadable: every head read is unavailable" >&2
  exit "${rc:-1}"
fi
field=crusty_verdict; [ "$name" = "autodrive-merge-round" ] && field=merge_ready_verdict
ci=PASS; case "${waits:-}" in *ci-pending*) ci=PENDING ;; esac
case "${items:-}" in *ci-failing*) ci="RED:1/1" ;; esac
( umask 077
  printf '{"%s":"%s","head_sha":"%s","reviewed_head_sha":"%s","round_label":"%s","ci_signal":"%s","merge_state":"BLOCKED","waits":%s,"wait_target":"%s","round_waiting":%s}\n' \
    "$field" "$verdict" "$head" "$head" "$label" "$ci" "${waits:-[]}" "${target:-}" "${waiting:-false}" > "$record"
  : > "$record.findings"
  if [ -n "${items:-}" ]; then printf '%s\n' "$items" | tr ',' '\n' > "$record.findings"; fi )
if [ -n "${AUTODRIVE_COMPLETION_STATE:-}" ] && [ -f "$record" ]; then
  "$REAL_AMPLIHACK" orch helper autodrive-completion-seal --private-state "$AUTODRIVE_COMPLETION_STATE" || exit 2
  action=evaluate
  case "$verdict" in CLEAN|MINOR_ONLY) action=complete ;; esac
  "$REAL_AMPLIHACK" orch helper autodrive-completion-write --private-state "$AUTODRIVE_COMPLETION_STATE" --action "$action" --reason 'Wait fixture judgement' || exit 2
fi
echo "round ran"
exit "${rc:-0}"
"#;

struct Loop {
    tmp: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
    stub: PathBuf,
    fix: PathBuf,
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
        let (bin, log, stub, fix, state, repo, cwd, task) = (
            p("bin"),
            p("log"),
            p("stub-state"),
            p("fix"),
            p("state"),
            p("repo"),
            p("cwd"),
            p("task-in"),
        );
        for d in [&bin, &log, &stub, &fix, &cwd] {
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
        let l = Loop {
            tmp,
            bin,
            log,
            stub,
            fix,
            state,
            repo,
            cwd,
            task,
        };
        write_fixtures(&l.fix, &l.head());
        Some(l)
    }

    fn plan(&self, lines: &[&str]) {
        std::fs::write(self.stub.join("plan"), lines.join("\n") + "\n").unwrap();
    }

    fn mode(&self, file: &str, v: &str) {
        std::fs::write(self.fix.join(file), v).unwrap();
    }

    fn head(&self) -> String {
        git(&self.repo, self.tmp.path(), &["rev-parse", "HEAD"])
    }

    fn run(&self, loop_name: &str) -> Output {
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
        crate::isolated_amplihack::isolated("bash")
            .arg(repo_root().join("amplifier-bundle/tools/autodrive_loop.sh"))
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
            .env("FAKE_FIX", &self.fix)
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env(
                "REAL_AMPLIHACK",
                crate::support::amplihack_bin::amplihack_bin(),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&self.cwd)
            .output()
            .unwrap()
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.stub.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn evaluator_argvs(&self) -> Vec<Vec<String>> {
        let mut names: Vec<_> = std::fs::read_dir(&self.log)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("call-"))
            .collect();
        names.sort();
        names
            .iter()
            .map(|n| {
                std::fs::read(self.log.join(n))
                    .unwrap()
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect::<Vec<_>>()
            })
            .filter(|a| a.get(2).map(String::as_str) == Some("loop-health-evaluator"))
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

fn assert_waiting(l: &Loop, o: &Output, loop_name: &str, target: &str) -> Value {
    assert_eq!(
        o.status.code(),
        Some(0),
        "WAITING is not a failure: {}",
        show(o)
    );
    let r = result(o);
    assert_eq!(r["loop_result"], "WAITING", "{}", show(o));
    assert_eq!(r["wait_target"], target, "{r}");
    assert!(
        r["wait_reason"].as_str().is_some_and(|s| !s.is_empty()),
        "{r}"
    );
    assert!(
        !stderr(o).contains(DECIDED),
        "a wait is not lack of progress: {}",
        show(o)
    );
    assert!(!stdout(o).contains("\"STUCK\""), "{}", show(o));
    let saved = l
        .state_file(&format!("{loop_name}-result.json"))
        .expect("the result file is written");
    let saved: Value = serde_json::from_str(saved.trim()).unwrap();
    assert_eq!(saved["loop_result"], "WAITING", "{saved}");
    assert_eq!(saved["wait_target"], target, "{saved}");
    r
}

// ── T3: wait-only rounds end WAITING, never STUCK ───────────────────────────

#[test]
fn a_pending_ci_only_round_ends_waiting_keyed_to_the_head() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[r#"NOT_MERGE_READY||["ci-pending"]|ci@HEAD|true|0|"#]);
    let o = l.run("merge-ready");
    assert_waiting(&l, &o, "merge-ready", &format!("ci@{}", l.head()));
    assert!(
        l.lines("evaluator-calls").is_empty(),
        "a wait-only round never runs the evaluator"
    );
    assert_eq!(
        l.lines("round-calls"),
        ["round-1"],
        "the loop ends at the wait"
    );
}

#[test]
fn a_rate_limited_read_only_round_ends_waiting_for_the_reset() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[&format!(
        r#"NOT_MERGE_READY|approval-facts-unreadable|["approval-facts-unreadable"]|ratelimit-reset@{EPOCH}|true|0|"#
    )]);
    let o = l.run("merge-ready");
    let r = assert_waiting(&l, &o, "merge-ready", &format!("ratelimit-reset@{EPOCH}"));
    assert!(
        stderr(&o).contains(&format!("ratelimit-reset@{EPOCH}")),
        "the stop names the reset: {}",
        show(&o)
    );
    assert!(l.lines("evaluator-calls").is_empty(), "{r}");
}

#[test]
fn repeated_wait_only_rounds_across_resumes_never_become_stuck() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[r#"NOT_MERGE_READY||["ci-pending"]|ci@HEAD|true|0|"#]);
    let target = format!("ci@{}", l.head());
    for _ in 0..3 {
        let o = l.run("merge-ready");
        assert_waiting(&l, &o, "merge-ready", &target);
    }
    assert!(
        l.state_file("merge-ready-stuck-reason.txt").is_none(),
        "no STUCK reason is written"
    );
}

#[test]
fn an_unknown_merge_state_round_ends_waiting_on_mergeability() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[r#"NOT_MERGE_READY||["merge-state-unknown"]|mergeability@HEAD|true|0|"#]);
    let o = l.run("merge-ready");
    assert_waiting(&l, &o, "merge-ready", &format!("mergeability@{}", l.head()));
}

#[test]
fn a_forged_wait_target_never_reaches_the_result() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[r#"NOT_MERGE_READY||["ci-pending"]|ci@$(touch pwned)|true|0|"#]);
    let o = l.run("merge-ready");
    assert!(
        !stdout(&o).contains("pwned") && !l.state.join("pwned").exists(),
        "wait_target is checked against closed patterns: {}",
        show(&o)
    );
    if let Some(saved) = l.state_file("merge-ready-result.json") {
        assert!(!saved.contains("pwned"), "{saved}");
    }
}

// ── D11: the evaluator is told about waits on mixed rounds ──────────────────

#[test]
fn a_mixed_round_runs_the_evaluator_with_the_waits_labelled() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[
        r#"NOT_MERGE_READY|qa-failed|["ci-pending"]|ci@HEAD|false|0|"#,
        r#"NOT_MERGE_READY|qa-failed,lint-error|["ci-pending"]|ci@HEAD|false|0|"#,
        r#"NOT_MERGE_READY|lint-error|[]||false|0|"#,
    ]);
    let o = l.run("merge-ready");
    let argvs = l.evaluator_argvs();
    assert!(
        !argvs.is_empty(),
        "a mixed round is still judged: {}",
        show(&o)
    );
    let first = &argvs[0];
    assert!(
        first
            .iter()
            .any(|a| a == r#"loop_round_waits=["ci-pending"]"#),
        "evaluator gets loop_round_waits: {first:?}"
    );
    assert!(
        first.iter().any(|a| a == "loop_round_wait_only=false"),
        "evaluator gets loop_round_wait_only: {first:?}"
    );
    let last = argvs.last().unwrap();
    assert!(last.iter().any(|a| a == "loop_round_waits=[]"), "{last:?}");
}

// ── T6 and mixed rounds: fixable findings still give STUCK ──────────────────

#[test]
fn an_unchanged_failing_check_still_stops_as_stuck_in_code() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[r#"NOT_MERGE_READY|ci-failing|[]||false|0|"#]);
    let o = l.run("merge-ready");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let r = result(&o);
    assert_eq!(r["loop_result"], "STUCK", "{}", show(&o));
    assert!(r["reason"].as_str().unwrap().contains(DECIDED), "{r}");
    assert_eq!(r["round_label"], "round-2");
    assert_eq!(l.lines("evaluator-calls"), ["round-1"]);
}

#[test]
fn a_mixed_round_counts_toward_stuck_on_its_fixable_findings_only() {
    // Round 2 has the same fixable finding at the same head; its waits are
    // different. The waits neither block nor excuse the STUCK stop.
    let Some(l) = Loop::new() else { return };
    l.plan(&[
        r#"NOT_MERGE_READY|qa-failed|["ci-pending"]|ci@HEAD|false|0|"#,
        &format!(r#"NOT_MERGE_READY|qa-failed|["approval-facts-unreadable"]|ratelimit-reset@{EPOCH}|false|0|"#),
    ]);
    let o = l.run("merge-ready");
    let r = result(&o);
    assert_eq!(r["loop_result"], "STUCK", "{}", show(&o));
    assert!(r["reason"].as_str().unwrap().contains(DECIDED), "{r}");
}

// ── Crusty: the step-01 sidecar and the dispatch's exit 76 ──────────────────

#[test]
fn a_crusty_round_whose_head_is_unavailable_ends_waiting_not_invalid_decision() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[&format!(
        "-|||ratelimit-reset@{EPOCH}|true|1|crusty-pr-head-unreadable"
    )]);
    let o = l.run("crusty");
    let r = assert_waiting(&l, &o, "crusty", &format!("ratelimit-reset@{EPOCH}"));
    assert_eq!(r["wait_reason"], "crusty-pr-head-unreadable", "{r}");
    assert!(!stdout(&o).contains("INVALID_DECISION"), "{}", show(&o));
    assert!(l.lines("evaluator-calls").is_empty());
}

#[test]
fn a_stale_wait_sidecar_does_not_stop_the_next_round() {
    let Some(l) = Loop::new() else { return };
    // Left by an earlier run's round-1, which a fresh run labels again.
    let stale = l.state.join("crusty-round-1.json.wait");
    std::fs::write(
        &stale,
        format!(r#"{{"round_waiting":true,"wait_reason":"crusty-pr-head-unreadable","wait_target":"ratelimit-reset@{EPOCH}"}}"#),
    )
    .unwrap();
    std::fs::set_permissions(&stale, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
    l.plan(&["CONCERNS|docs-typo|[]||false|0|"]);
    let o = l.run("crusty");
    let r = result(&o);
    assert_ne!(
        r["loop_result"],
        "WAITING",
        "an old sidecar never stops a later round: {}",
        show(&o)
    );
    assert!(
        !l.lines("evaluator-calls").is_empty(),
        "round-1 was judged: {}",
        show(&o)
    );
}

#[test]
fn a_crusty_completion_with_the_head_unavailable_ends_the_loop_waiting() {
    let Some(l) = Loop::new() else { return };
    l.mode("gql", "refuse");
    l.mode("rest", "refuse");
    l.plan(&["CLEAN||[]||false|0|"]);
    let o = l.run("crusty");
    assert_eq!(
        o.status.code(),
        Some(0),
        "dispatch 76 maps to WAITING, exit 0: {}",
        show(&o)
    );
    let r = result(&o);
    assert_eq!(r["loop_result"], "WAITING", "{}", show(&o));
    assert_eq!(r["wait_reason"], "head-unavailable", "{r}");
    assert!(!stdout(&o).contains("INVALID_DECISION"), "{}", show(&o));
    assert!(l.lines("evaluator-calls").is_empty());
    let saved = l.state_file("crusty-result.json").unwrap();
    assert!(saved.contains("\"WAITING\""), "{saved}");
}

#[test]
fn a_crusty_completion_with_graphql_refused_but_rest_answering_is_done() {
    let Some(l) = Loop::new() else { return };
    l.mode("gql", "refuse");
    l.plan(&["CLEAN||[]||false|0|"]);
    let o = l.run("crusty");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(result(&o)["loop_result"], "DONE", "{}", show(&o));
}

// ── IV3: autodrive_prior_wait names the previous round's sidecar ────────────

impl Loop {
    /// The `-c autodrive_prior_wait=` value of each round-recipe call, in call
    /// order. `None` when a call did not pass it at all.
    fn prior_wait_values(&self, recipe: &str) -> Vec<Option<String>> {
        let mut names: Vec<_> = std::fs::read_dir(&self.log)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("call-"))
            .collect();
        names.sort();
        names
            .iter()
            .map(|n| {
                std::fs::read(self.log.join(n))
                    .unwrap()
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect::<Vec<_>>()
            })
            .filter(|a| a.get(2).map(String::as_str) == Some(recipe))
            .map(|a| {
                a.windows(2).find_map(|w| {
                    (w[0] == "-c")
                        .then(|| w[1].strip_prefix("autodrive_prior_wait="))
                        .flatten()
                        .map(str::to_owned)
                })
            })
            .collect()
    }
}

#[test]
fn each_round_is_given_the_previous_rounds_sidecar_path() {
    let Some(l) = Loop::new() else { return };
    // Round 1 has a fixable finding, so the loop goes on to round 2.
    l.plan(&[
        "NOT_MERGE_READY|docs-typo|[]||false|0|",
        r#"NOT_MERGE_READY||["ci-pending"]|ci@HEAD|true|0|"#,
    ]);
    let o = l.run("merge-ready");
    assert_eq!(
        l.lines("round-calls"),
        ["round-1", "round-2"],
        "{}",
        show(&o)
    );
    let prev = l.state.join("merge-ready-round-1.json.wait");
    assert_eq!(
        l.prior_wait_values("autodrive-merge-round"),
        [Some(String::new()), Some(prev.to_str().unwrap().to_owned())],
        "round 1 has no previous round; round 2 names round 1's sidecar: {}",
        show(&o)
    );
}

#[test]
fn a_resumed_drive_names_the_last_round_of_the_earlier_drive_and_keeps_its_sidecar() {
    let Some(l) = Loop::new() else { return };
    l.plan(&[
        r#"NOT_MERGE_READY||["merge-state-unknown"]|mergeability@HEAD|true|0|"#,
        r#"NOT_MERGE_READY||["merge-state-unknown"]|mergeability@HEAD|true|0|"#,
    ]);
    let target = format!("mergeability@{}", l.head());
    let o = l.run("merge-ready");
    assert_waiting(&l, &o, "merge-ready", &target);
    // What merge-evidence step-01 leaves behind on a first UNKNOWN read.
    let prev = l.state.join("merge-ready-round-1.json.wait");
    let body = crate::support::autodrive_gh_fake::unknown_sidecar(&l.head());
    std::fs::write(&prev, &body).unwrap();

    let o = l.run("merge-ready");
    assert_waiting(&l, &o, "merge-ready", &target);
    assert_eq!(
        l.lines("round-calls"),
        ["round-1", "round-2"],
        "{}",
        show(&o)
    );
    assert_eq!(
        l.prior_wait_values("autodrive-merge-round").last().cloned(),
        Some(Some(prev.to_str().unwrap().to_owned())),
        "the resumed round-2 names round-1's sidecar: {}",
        show(&o)
    );
    assert_eq!(
        std::fs::read_to_string(&prev).ok().as_deref(),
        Some(body.as_str()),
        "the round-start rm removes only the current round's sidecar"
    );
}
