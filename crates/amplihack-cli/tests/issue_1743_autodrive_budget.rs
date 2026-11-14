//! Issue #1743: auto-drive stops a loop in code when a round changes nothing,
//! and each run has a usage budget.
//!
//! Contract (docs/reference/auto-drive-to-merge.md, "No-progress stop and
//! usage budget"):
//!
//! - Crusty completes a verified clean objective without an evaluator call.
//!   Unresolved Crusty rounds require a completion decision and sealed evidence.
//! - After a round that is not clean, and before the evaluator, the driver
//!   compares the PR head SHA and the normalised open items with the last row
//!   of `<loop>-progress.tsv`. When the previous row is not clean and both
//!   match, the loop stops as `STUCK` (decided in code), exit 1, and the
//!   evaluator does not run for that round.
//! - Three limits (`autodrive_budget_loop_rounds`, `_total_rounds`,
//!   `_wall_seconds`), cumulative across runs on the same state directory,
//!   stop the loop as `BUDGET_EXHAUSTED`, exit 1. Clean rounds are never
//!   stopped after they ran. A relaunch with a higher limit resumes from the
//!   recorded round; one with the same limit starts no agent.
//! - Every stop writes `<loop>-result.json`; `STUCK` writes
//!   `<loop>-stuck-reason.txt`, `BUDGET_EXHAUSTED` writes
//!   `autodrive-budget-summary.md`, and both post a sanitised PR comment,
//!   with secrets masked, through `gh pr comment --body-file`.
//!
//! The driver runs end to end under `isolated("bash")` with a stub
//! `amplihack` (rounds, evaluator, jq-based `orch helper`) and a stub `gh`
//! first on `PATH`, a fixture main checkout with a linked worktree as the PR
//! worktree, and a fixture state directory. Nothing reaches GitHub or an
//! agent, and nothing under the real `~/.claude` or `~/.amplihack` is named.
//!
//! The stub evaluator answers CONTINUE, or DONE in `done` mode, and turns to
//! STUCK after `eval_stuck_after` calls (default 8), so a missing
//! implementation fails these tests instead of looping for ever.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support::auto_drive_light::{
    jq_available, private_dir, private_file, read, recipe, repo_root, show, stderr, stdout,
};

const BUDGET_KEYS: [&str; 3] = [
    "autodrive_budget_loop_rounds",
    "autodrive_budget_total_rounds",
    "autodrive_budget_wall_seconds",
];

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
exec 9>"$S/calls.lock"; flock 9
n=$(cat "$S/calls" 2>/dev/null || echo 0); n=$((n + 1)); printf '%s' "$n" > "$S/calls"
flock -u 9; exec 9>&-
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
  [ -f "$S/eval_rc" ] && { echo "orphaned: parent recipe run 4242 exited; stopping" >&2; exit "$(cat "$S/eval_rc")"; }
  h=$(wc -l < "$S/evaluator-calls")
  if [ "$h" -gt "$(cat "$S/eval_stuck_after" 2>/dev/null || echo 8)" ]; then echo "LOOP_HEALTH: STUCK"; exit 1; fi
  v=CONTINUE; [ "$(cat "$S/eval" 2>/dev/null)" = "done" ] && v=DONE
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
IFS='|' read -r verdict commit sleep_s light items wait_s rc_s <<<"$line"
[ "${sleep_s:-0}" -gt 0 ] && sleep "$sleep_s"
# A usage-limit wait, as the real wait reports it: log lines, then its
# length on the ledger the loop names (profile-command.md#usage-limit-wait).
if [ "${wait_s:-0}" -gt 0 ]; then
  echo "amplihack.recipe.usage_limit_wait start reason=usage_limit wait=capacity harness=claude reset=none until=2026-10-18 14:00:00 -07:00 bound=max_wait" >&2
  sleep "$wait_s"
  printf '%s\n' "$wait_s" >> "$AMPLIHACK_USAGE_WAIT_LEDGER"
  echo "amplihack.recipe.usage_limit_wait end reason=rotator capacity" >&2
  echo "amplihack.recipe.usage_limit_wait resume attempt=2" >&2
fi
[ -f "$S/wait_claim" ] && cat -- "$S/wait_claim" >> "$AMPLIHACK_USAGE_WAIT_LEDGER"
if [ "$commit" = "1" ]; then
  t="$(git -C "$wd" write-tree)"
  c="$(git -C "$wd" -c user.name=t -c user.email=t@t commit-tree "$t" -p HEAD -m "stub $label")"
  git -C "$wd" update-ref HEAD "$c"
fi
field=crusty_verdict; [ "$name" = "autodrive-merge-round" ] && field=merge_ready_verdict
if [ "$verdict" != "-" ]; then
  head="$(git -C "$wd" rev-parse HEAD)"
  printf '{"%s":"%s","head_sha":"%s","reviewed_head_sha":"%s","round_label":"%s"}\n' "$field" "$verdict" "$head" "$head" "$label" > "$record"
  : > "$record.findings"
  if [ -n "$items" ]; then printf '%s\n' "$items" | tr ',' '\n' > "$record.findings"; fi
  [ "$light" = "1" ] && printf '{"verdict_source":"light-minor-rule"}\n' > "$record.light.json"
fi
if [ -n "${AUTODRIVE_COMPLETION_STATE:-}" ] && [ -f "$record" ]; then
  chmod 0600 "$record" "$record.findings"
  "$REAL_AMPLIHACK" orch helper autodrive-completion-seal --private-state "$AUTODRIVE_COMPLETION_STATE" || exit 2
  action=evaluate
  case "$verdict" in CLEAN|MINOR_ONLY) action=complete ;; esac
  "$REAL_AMPLIHACK" orch helper autodrive-completion-write --private-state "$AUTODRIVE_COMPLETION_STATE" --action "$action" --reason 'Budget fixture judgement' || exit 2
fi
echo "round ran"
exit "${rc_s:-0}"
"#;

const GH_STUB: &str = r#"#!/usr/bin/env bash
set -u
# Head verification is read-only; comment assertions count only comment calls.
if [ "${1:-} ${2:-}" = 'pr view' ]; then git -C "$STUB_REPO" rev-parse HEAD; exit $?; fi
exec 9>"$STUB_STATE/gh.lock"; flock 9
n=$(cat "$STUB_STATE/gh-calls" 2>/dev/null || echo 0); n=$((n + 1)); printf '%s' "$n" > "$STUB_STATE/gh-calls"
flock -u 9; exec 9>&-
printf '%s\0' "$@" > "$STUB_LOG/gh-$n"
printf '%s' "${GH_PROMPT_DISABLED:-}" > "$STUB_LOG/gh-env-$n"
prev=""
for a in "$@"; do [ "$prev" = "--body-file" ] && cp -- "$a" "$STUB_LOG/gh-body-$n"; prev=$a; done
exit "$(cat "$STUB_STATE/gh_rc" 2>/dev/null || echo 0)"
"#;

fn tool(name: &str) -> PathBuf {
    repo_root().join("amplifier-bundle/tools").join(name)
}

fn git(dir: &Path, home: &Path, args: &[&str]) -> String {
    let out = amplihack_git::command_in(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct Fixture {
    tmp: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
    stub: PathBuf,
    state: PathBuf,
    repo: PathBuf,
    cwd: PathBuf,
    task: PathBuf,
}

impl Fixture {
    fn new() -> Option<Self> {
        if !jq_available() {
            eprintln!("SKIP: jq is not installed; the stub orch helper needs it");
            return None;
        }
        // Outside the real `~/.claude` and `~/.amplihack`: `assert_no_real_home`
        // fails on any file naming them, and every fixture path appears in the
        // stub logs (tests/support/scratch_base.rs).
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        // Completion helpers require physical paths on macOS (/var is a symlink).
        let physical = tmp.path().canonicalize().unwrap();
        let p = |n: &str| physical.join(n);
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
        std::fs::write(&task, "issue 1743 fixture task\n").unwrap();
        git(&cwd, tmp.path(), &["init", "-q"]);
        let tree = git(&cwd, tmp.path(), &["write-tree"]);
        let commit = git(&cwd, tmp.path(), &["commit-tree", &tree, "-m", "init"]);
        git(&cwd, tmp.path(), &["update-ref", "HEAD", &commit]);
        let wt = repo.display().to_string();
        git(
            &cwd,
            tmp.path(),
            &["worktree", "add", "-q", "--detach", &wt],
        );
        crate::support::exec_stub::write_executable(&bin.join("amplihack"), AMPLIHACK_STUB, 0o755);
        crate::support::exec_stub::write_executable(&bin.join("gh"), GH_STUB, 0o755);
        let fx = Fixture {
            tmp,
            bin,
            log,
            stub,
            state,
            repo,
            cwd,
            task,
        };
        fx.plan(&["CONCERNS|0|0|0|a"]);
        Some(fx)
    }

    /// One line per round label: `VERDICT|COMMIT|SLEEP|LIGHT|item,item[|WAIT]`,
    /// where WAIT is the seconds of a usage-limit wait the round reports.
    /// The last line repeats; `-` as the verdict writes no record.
    fn plan(&self, lines: &[&str]) {
        std::fs::write(self.stub.join("plan"), lines.join("\n") + "\n").unwrap();
    }

    fn set(&self, name: &str, value: &str) {
        std::fs::write(self.stub.join(name), value).unwrap();
    }

    fn run(&self, loop_name: &str, extra: &[&str]) -> Output {
        self.run_pr(loop_name, Some("1743"), extra)
    }

    fn run_pr(&self, loop_name: &str, pr: Option<&str>, extra: &[&str]) -> Output {
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
        c.arg(tool("autodrive_loop.sh"))
            .args(["--loop-name", loop_name, "--round-recipe", recipe])
            .args(["--clean-token", token, "--verdict-field", field])
            .arg("--repo")
            .arg(&self.repo)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--task-file")
            .arg(&self.task)
            .args(["--context", "repo_path=."]);
        if let Some(pr) = pr {
            c.args(["--context", &format!("pr_number={pr}")]);
        }
        c.args(extra)
            .env("PATH", path)
            .env("STUB_LOG", &self.log)
            .env("STUB_STATE", &self.stub)
            .env("STUB_REPO", &self.repo)
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env(
                "REAL_AMPLIHACK",
                crate::support::amplihack_bin::amplihack_bin(),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&self.cwd)
            .output()
            .expect("spawn autodrive_loop.sh")
    }

    fn lines(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.stub.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn evaluator_calls(&self) -> Vec<String> {
        self.lines("evaluator-calls")
    }

    fn round_calls(&self) -> Vec<String> {
        self.lines("round-calls")
    }

    fn recipe_argvs(&self) -> Vec<Vec<String>> {
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
                    .collect()
            })
            .collect()
    }

    /// Each `gh` call: its argv, the body file it named, and GH_PROMPT_DISABLED.
    fn gh_calls(&self) -> Vec<(Vec<String>, String, String)> {
        let n: usize = std::fs::read_to_string(self.stub.join("gh-calls"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        (1..=n)
            .map(|i| {
                let argv = std::fs::read(self.log.join(format!("gh-{i}")))
                    .unwrap()
                    .split(|b| *b == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| String::from_utf8_lossy(s).into_owned())
                    .collect();
                let body = std::fs::read_to_string(self.log.join(format!("gh-body-{i}")))
                    .unwrap_or_default();
                let env = std::fs::read_to_string(self.log.join(format!("gh-env-{i}")))
                    .unwrap_or_default();
                (argv, body, env)
            })
            .collect()
    }

    fn head(&self) -> String {
        git(&self.repo, self.tmp.path(), &["rev-parse", "HEAD"])
    }

    fn state_file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.state.join(name)).ok()
    }

    fn mode(&self, name: &str) -> u32 {
        std::fs::symlink_metadata(self.state.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .permissions()
            .mode()
            & 0o777
    }

    /// No stub log or state file names the real `~/.claude` or `~/.amplihack`.
    ///
    /// The fixture's own paths are removed from the text first. A caller whose
    /// temp directory sits under `~/.amplihack` (a recipe step's per-run
    /// `TMPDIR`) would otherwise match on every fixture path, though none of
    /// them is the real home (PR #1885).
    fn assert_no_real_home(&self) {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let home = PathBuf::from(home);
        let forbidden = [home.join(".claude"), home.join(".amplihack")];
        let fixture_roots: Vec<String> = [self.tmp.path().to_path_buf()]
            .into_iter()
            .chain(self.tmp.path().canonicalize().ok())
            .map(|p| p.display().to_string())
            .collect();
        for dir in [&self.log, &self.state, &self.stub] {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if !p.is_file() {
                    continue;
                }
                let mut text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
                for root in &fixture_roots {
                    text = text.replace(root.as_str(), "<fixture>");
                }
                for f in &forbidden {
                    let f = f.display().to_string();
                    assert!(!text.contains(&f), "{} names the real {f}", p.display());
                }
            }
        }
    }
}

fn ctx(key: &str, value: &str) -> [String; 2] {
    ["--context".to_string(), format!("{key}={value}")]
}

fn args(pairs: &[(&str, &str)]) -> Vec<String> {
    pairs.iter().flat_map(|(k, v)| ctx(k, v)).collect()
}

fn run_with(fx: &Fixture, loop_name: &str, pairs: &[(&str, &str)]) -> Output {
    let a = args(pairs);
    let a: Vec<&str> = a.iter().map(String::as_str).collect();
    fx.run(loop_name, &a)
}

/// The last stdout line carrying `loop_result`.
fn result(o: &Output) -> serde_json::Value {
    stdout(o)
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find(|v| v.get("loop_result").is_some())
        .unwrap_or_else(|| panic!("no loop_result line\n{}", show(o)))
}

const DECIDED: &str = "no progress (decided in code)";

// ── No-progress stop ─────────────────────────────────────────────────────────

#[test]
fn unchanged_head_and_items_stop_as_stuck_in_code_before_the_evaluator() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|0|b,a"]);
    let o = fx.run("crusty", &[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let r = result(&o);
    assert_eq!(r["loop_result"], "STUCK", "{}", show(&o));
    assert_eq!(r["round_label"], "round-2");
    let want = format!(
        "{DECIDED}: head {} and open items unchanged since round-1: a,b",
        fx.head()
    );
    assert_eq!(r["reason"], want.as_str());
    assert_eq!(
        fx.evaluator_calls(),
        ["round-1"],
        "round-2 never reaches the evaluator"
    );
    assert_eq!(fx.round_calls(), ["round-1", "round-2"]);
    let reason = fx
        .state_file("crusty-stuck-reason.txt")
        .expect("stuck reason file");
    assert_eq!(reason.trim_end(), want);
    let file: serde_json::Value = serde_json::from_str(
        fx.state_file("crusty-result.json")
            .expect("result file")
            .trim(),
    )
    .unwrap();
    assert_eq!(file, r, "the result file holds the escalate line");
    let err = stderr(&o);
    assert!(err.contains("AUTO_DRIVE_LOOP: STUCK"), "{err}");
    assert!(
        err.contains("absence of progress, not an attempt count"),
        "{err}"
    );
    for f in [
        "crusty-stuck-reason.txt",
        "crusty-result.json",
        "crusty-progress.tsv",
    ] {
        assert_eq!(fx.mode(f), 0o600, "{f}");
    }
    fx.assert_no_real_home();
}

#[test]
fn a_new_commit_every_round_is_never_stopped_by_the_rule() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    fx.set("eval_stuck_after", "3");
    let o = fx.run("crusty", &[]);
    let r = result(&o);
    assert_eq!(r["loop_result"], "STUCK");
    assert!(
        !r["reason"].as_str().unwrap().contains(DECIDED),
        "{}",
        show(&o)
    );
    assert_eq!(
        fx.evaluator_calls().len(),
        4,
        "every round reached the evaluator"
    );
    assert!(fx.state_file("crusty-stuck-reason.txt").is_none());
    let rows = fx.state_file("crusty-progress.tsv").expect("progress file");
    assert_eq!(rows.lines().count(), 4, "one row per round:\n{rows}");
}

#[test]
fn changed_open_items_count_as_progress_and_the_next_repeat_stops() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|0|a", "CONCERNS|0|0|0|b"]);
    let o = fx.run("merge-ready", &[]);
    let r = result(&o);
    assert_eq!(r["loop_result"], "STUCK", "{}", show(&o));
    assert_eq!(r["round_label"], "round-3");
    assert!(
        r["reason"].as_str().unwrap().ends_with("since round-2: b"),
        "{r}"
    );
    assert_eq!(fx.evaluator_calls(), ["round-1", "round-2"]);
    assert!(fx.state_file("merge-ready-stuck-reason.txt").is_some());
}

#[test]
fn open_items_are_compared_after_normalisation() {
    let Some(fx) = Fixture::new() else { return };
    // Same set: order, duplicates and surrounding space differ.
    fx.plan(&["CONCERNS|0|0|0|  b ,a,,a", "CONCERNS|0|0|0|a,b"]);
    let o = fx.run("crusty", &[]);
    let r = result(&o);
    assert!(
        r["reason"].as_str().unwrap().contains(DECIDED),
        "{}",
        show(&o)
    );
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .ends_with("since round-1: a,b"),
        "{r}"
    );
    assert_eq!(fx.evaluator_calls(), ["round-1"]);
}

#[test]
fn a_clean_previous_round_never_matches_and_an_empty_set_reads_none() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["MERGE_READY|0|0|0|", "CONCERNS|0|0|0|"]);
    let o = fx.run("merge-ready", &[]);
    let r = result(&o);
    assert_eq!(r["round_label"], "round-3", "{}", show(&o));
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .ends_with("since round-2: none"),
        "{r}"
    );
    assert_eq!(fx.evaluator_calls(), ["round-1", "round-2"]);
    let rows = fx.state_file("merge-ready-progress.tsv").unwrap();
    let first: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert_eq!(
        first,
        ["round-1", fx.head().as_str(), "1", "", "", h("").as_str()],
        "{rows}"
    );
}

#[test]
fn a_converged_round_keeps_done_and_records_its_row() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CLEAN|0|0|0|"]);
    fx.set("eval", "done");
    let o = fx.run("crusty", &[]);
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert_eq!(result(&o)["loop_result"], "DONE");
    let file = fx
        .state_file("crusty-result.json")
        .expect("a DONE writes the result file too");
    assert!(file.contains(r#""loop_result":"DONE""#), "{file}");
    assert_eq!(
        fx.state_file("crusty-progress.tsv")
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn a_leftover_light_json_does_not_exempt_a_round() {
    // Issue #1982: nothing writes `<record>.light.json` any more, and a file
    // that an earlier auto-drive-light left beside a record is not read. A
    // round on an unchanged head with unchanged open items stops as STUCK in
    // code, whether the leftover file sits beside the current record, the
    // previous one, or both.
    for plan in [
        &["CONCERNS|0|0|1|a,b"][..],
        &["CONCERNS|0|0|1|", "CONCERNS|0|0|0|"][..],
        &["CONCERNS|0|0|0|a", "CONCERNS|0|0|1|a"][..],
    ] {
        let Some(fx) = Fixture::new() else { return };
        fx.plan(plan);
        let o = fx.run("crusty", &[]);
        assert_eq!(o.status.code(), Some(1), "{plan:?}: {}", show(&o));
        let r = result(&o);
        assert_eq!(r["loop_result"], "STUCK", "{plan:?}: {}", show(&o));
        assert_eq!(r["round_label"], "round-2", "{plan:?}: {}", show(&o));
        assert!(
            r["reason"].as_str().unwrap().contains(DECIDED),
            "{plan:?}: a leftover .light.json must not exempt the round: {}",
            show(&o)
        );
        assert_eq!(
            fx.evaluator_calls(),
            ["round-1"],
            "{plan:?}: round-2 never reaches the evaluator"
        );
    }
}

#[test]
fn the_progress_rule_reads_no_light_file() {
    let body =
        crate::support::auto_drive_light::read("amplifier-bundle/tools/autodrive_progress.sh");
    assert!(
        !body.contains(".light.json"),
        "autodrive_progress.sh must not read <record>.light.json (#1982)"
    );
    assert!(
        !body.contains("#1802"),
        "the #1802 light exemption and its header clause are removed"
    );
}

#[test]
fn failed_rounds_on_one_head_do_not_stop_and_the_evaluator_decides() {
    // Plan fields: verdict|commit|sleep|light|items|wait|exit code.
    // Two rounds that exit non-zero with no findings, then two rounds that
    // leave no record at all (verdict MISSING), all on one head.
    for plan in ["CONCERNS|0|0|0||0|1", "-|0|0|0||0|0"] {
        let Some(fx) = Fixture::new() else { return };
        fx.plan(&[plan]);
        fx.set("eval_stuck_after", "3");
        let o = fx.run("merge-ready", &[]);
        let r = result(&o);
        assert!(
            !r["reason"].as_str().unwrap().contains(DECIDED),
            "{plan}: {}",
            show(&o)
        );
        assert_eq!(
            fx.evaluator_calls().len(),
            4,
            "{plan}: every round reached the evaluator"
        );
        let rows = fx
            .state_file("merge-ready-progress.tsv")
            .expect("progress file");
        assert_eq!(rows.lines().count(), 4, "{plan}:\n{rows}");
        for row in rows.lines() {
            assert_eq!(
                row.split('\t').nth(2),
                Some("2"),
                "{plan}: a failed round is flagged 2: {row}"
            );
        }
    }
}

// ── Orphaned children (#1976, crusty review of #2112) ─────────────────────────

/// A round that exits 75 lost the recipe run above the loop. The loop stops
/// as ORPHANED with exit 75 before the progress row and the evaluator, which
/// would inherit the same dead parent and leave the crash reported as STUCK.
#[test]
fn a_round_that_exits_75_stops_the_loop_as_orphaned_before_the_evaluator() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["-|0|0|0||0|75"]);
    let o = fx.run("crusty", &[]);
    assert_eq!(o.status.code(), Some(75), "{}", show(&o));
    let r = result(&o);
    assert_eq!(r["loop_result"], "ORPHANED", "{}", show(&o));
    assert_eq!(r["round_label"], "round-1");
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .contains("round round-1 exited 75"),
        "{}",
        show(&o)
    );
    assert_eq!(fx.round_calls(), ["round-1"]);
    assert!(fx.evaluator_calls().is_empty(), "the evaluator never runs");
    assert!(
        fx.state_file("crusty-progress.tsv").is_none(),
        "no failed progress row is written for an orphaned round"
    );
    let saved = fx.state_file("crusty-result.json").expect("result file");
    assert!(saved.contains("\"loop_result\":\"ORPHANED\""), "{saved}");
    assert!(stderr(&o).contains("This is a lost driver"), "{}", show(&o));
}

/// An evaluator that exits 75 stops the loop the same way, not as STUCK.
#[test]
fn an_evaluator_that_exits_75_stops_the_loop_as_orphaned_not_stuck() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    fx.set("eval_rc", "75");
    let o = fx.run("crusty", &[]);
    assert_eq!(o.status.code(), Some(75), "{}", show(&o));
    let r = result(&o);
    assert_eq!(r["loop_result"], "ORPHANED", "{}", show(&o));
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .contains("the loop-health evaluator for round-1 exited 75"),
        "{}",
        show(&o)
    );
    assert_eq!(fx.evaluator_calls(), ["round-1"]);
    assert_eq!(fx.round_calls(), ["round-1"]);
}

/// Crusty requires sealed evidence even when its child exits with an ordinary failure.
#[test]
fn a_crusty_round_without_completion_evidence_stops_before_the_evaluator() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["-|0|0|0||0|1"]);
    let o = fx.run("crusty", &[]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert_eq!(result(&o)["loop_result"], "INVALID_DECISION");
    assert!(fx.evaluator_calls().is_empty(), "{}", show(&o));
    assert_eq!(fx.round_calls(), ["round-1"]);
}

// The generic loop can still evaluate an ordinary failed round without a record.
#[test]
fn only_exit_75_is_an_orphan_stop() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["-|0|0|0||0|1"]);
    fx.set("eval_stuck_after", "1");
    let o = fx.run("merge-ready", &[]);
    let r = result(&o);
    assert_ne!(r["loop_result"], "ORPHANED", "{}", show(&o));
    assert!(!fx.evaluator_calls().is_empty(), "{}", show(&o));
}

#[test]
fn the_round_after_a_failed_round_is_exempt_too() {
    let Some(fx) = Fixture::new() else { return };
    // Round 1 crashes; round 2 is a normal non-clean round with no findings on
    // the same head; round 3 repeats round 2 and is stopped.
    fx.plan(&["CONCERNS|0|0|0||0|1", "CONCERNS|0|0|0|"]);
    let o = fx.run("crusty", &[]);
    let r = result(&o);
    assert_eq!(r["round_label"], "round-3", "{}", show(&o));
    assert!(r["reason"].as_str().unwrap().contains(DECIDED));
    assert_eq!(fx.evaluator_calls(), ["round-1", "round-2"]);
}

#[test]
fn failed_rounds_are_still_bounded_by_the_budget() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|0||0|1"]);
    let o = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "2")]);
    let r = result(&o);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED", "{}", show(&o));
    assert_eq!(r["round_label"], "round-2");
    assert_eq!(fx.evaluator_calls(), ["round-1"]);
}

#[test]
fn a_relaunch_compares_its_first_round_with_the_earlier_run() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|0|a"]);
    fx.set("eval_stuck_after", "0");
    let first = fx.run("crusty", &[]);
    assert!(!result(&first)["reason"].as_str().unwrap().contains(DECIDED));
    fx.set("eval_stuck_after", "8");
    let second = fx.run("crusty", &[]);
    let r = result(&second);
    assert_eq!(r["round_label"], "round-2", "{}", show(&second));
    assert!(
        r["reason"].as_str().unwrap().ends_with("since round-1: a"),
        "{r}"
    );
    assert_eq!(fx.evaluator_calls(), ["round-1"]);
}

#[test]
fn an_untrusted_progress_file_fails_open_to_the_evaluator() {
    let Some(fx) = Fixture::new() else { return };
    let outside = fx.tmp.path().join("outside.tsv");
    std::fs::write(&outside, "round-0\tdeadbeef\t0\ta\n").unwrap();
    symlink(&outside, fx.state.join("crusty-progress.tsv")).unwrap();
    fx.plan(&["CONCERNS|0|0|0|a"]);
    fx.set("eval_stuck_after", "2");
    let o = fx.run("crusty", &[]);
    assert!(stderr(&o).contains("WARNING"), "{}", show(&o));
    assert!(!result(&o)["reason"].as_str().unwrap().contains(DECIDED));
    assert_eq!(
        std::fs::read_to_string(&outside).unwrap(),
        "round-0\tdeadbeef\t0\ta\n",
        "nothing is written through the symlink"
    );
}

// ── Budget ───────────────────────────────────────────────────────────────────

#[test]
fn loop_rounds_stops_after_the_round_that_reached_it_and_resumes_higher() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    let o = run_with(&fx, "merge-ready", &[(BUDGET_KEYS[0], "2")]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let r = result(&o);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED");
    assert_eq!(r["round_label"], "round-2");
    assert_eq!(
        fx.evaluator_calls(),
        ["round-1"],
        "the post-round check precedes the evaluator"
    );
    let err = stderr(&o);
    assert!(
        err.contains("This is a usage budget set by the operator"),
        "{err}"
    );
    assert!(!err.contains("absence of progress"), "{err}");
    let summary = fx
        .state_file("autodrive-budget-summary.md")
        .expect("summary");
    assert!(
        summary.starts_with("# Auto-drive usage budget reached"),
        "{summary}"
    );
    assert!(summary.contains("(post-round check)"), "{summary}");
    assert!(
        summary.contains("Limits reached: autodrive_budget_loop_rounds"),
        "{summary}"
    );
    assert!(
        summary.contains("| merge-ready rounds | 2 | 2 |"),
        "{summary}"
    );
    assert!(summary.contains("| tokens | unknown | none |"), "{summary}");
    assert!(
        summary.contains("-c autodrive_budget_loop_rounds=4"),
        "{summary}"
    );
    assert_eq!(fx.mode("autodrive-budget-summary.md"), 0o600);

    // The same limit again: no agent starts.
    let again = run_with(&fx, "merge-ready", &[(BUDGET_KEYS[0], "2")]);
    assert_eq!(
        result(&again)["loop_result"],
        "BUDGET_EXHAUSTED",
        "{}",
        show(&again)
    );
    assert_eq!(result(&again)["round_label"], "round-2");
    assert_eq!(fx.round_calls().len(), 2, "a pre-round stop runs no round");
    assert!(
        fx.state_file("autodrive-budget-summary.md")
            .unwrap()
            .contains("(pre-round check)")
    );

    // A higher limit resumes at the recorded round.
    let more = run_with(&fx, "merge-ready", &[(BUDGET_KEYS[0], "3")]);
    assert_eq!(result(&more)["round_label"], "round-3", "{}", show(&more));
    assert_eq!(fx.round_calls(), ["round-1", "round-2", "round-3"]);
    fx.assert_no_real_home();
}

/// The keys each loop step reads with `write_context_value` must all be set by
/// the printed resume command, or the resumed loop refuses to start (#1862).
#[test]
fn the_resume_command_sets_every_key_the_loop_steps_read() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    let o = run_with(&fx, "merge-ready", &[(BUDGET_KEYS[0], "1")]);
    assert_eq!(
        result(&o)["loop_result"],
        "BUDGET_EXHAUSTED",
        "{}",
        show(&o)
    );
    let summary = fx
        .state_file("autodrive-budget-summary.md")
        .expect("summary");
    let resume = summary
        .split("## Resume")
        .nth(1)
        .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or_else(|| panic!("no resume command:\n{summary}"))
        .to_string();
    assert!(
        resume.starts_with("amplihack recipe run auto-drive-to-merge "),
        "{resume}"
    );
    let doc = read("docs/reference/auto-drive-to-merge.md");
    let mut needed = Vec::new();
    for name in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let text = read(&format!("amplifier-bundle/recipes/{name}.yaml"));
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix("write_context_value ") else {
                continue;
            };
            let key = rest.split_whitespace().next().unwrap().to_string();
            if !needed.contains(&key) {
                needed.push(key);
            }
        }
    }
    assert!(needed.iter().any(|k| k == "task_description"), "{needed:?}");
    for key in &needed {
        let flag = format!("-c {key}=");
        assert!(
            resume.contains(&flag),
            "the printed resume command lacks {flag}: {resume}"
        );
        let after = &resume[resume.find(&flag).unwrap() + flag.len()..];
        assert!(
            !after.starts_with(' ') && !after.starts_with("\"\""),
            "{key} is empty in {resume}"
        );
        // The documented resume commands name it too.
        let section = doc
            .split("### Resuming after BUDGET_EXHAUSTED")
            .nth(1)
            .expect("resume section");
        let section = &section[..section.find("\n### ").unwrap_or(section.len())];
        assert!(
            section.contains(&flag),
            "the documented resume lacks {flag}"
        );
    }
    assert!(
        resume.contains("-c task_description=\"<the original task>\""),
        "a placeholder, never the task text: {resume}"
    );
    fx.assert_no_real_home();
}

#[test]
fn total_rounds_counts_both_loops() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    let crusty = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "2")]);
    assert_eq!(
        result(&crusty)["loop_result"],
        "BUDGET_EXHAUSTED",
        "{}",
        show(&crusty)
    );
    let merge = run_with(
        &fx,
        "merge-ready",
        &[(BUDGET_KEYS[0], "12"), (BUDGET_KEYS[1], "2")],
    );
    let r = result(&merge);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED", "{}", show(&merge));
    assert_eq!(r["round_label"], "?", "no merge-ready round was recorded");
    assert_eq!(
        fx.round_calls().len(),
        2,
        "the merge-ready loop started no round"
    );
    let summary = fx.state_file("autodrive-budget-summary.md").unwrap();
    assert!(
        summary.contains("Limits reached: autodrive_budget_total_rounds"),
        "{summary}"
    );
    assert!(summary.contains("| total rounds | 2 | 2 |"), "{summary}");
    assert!(
        summary.contains("merge-ready"),
        "remaining work names the loop:\n{summary}"
    );
}

#[test]
fn wall_seconds_counts_child_time() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|1|0|a"]);
    let o = run_with(&fx, "crusty", &[(BUDGET_KEYS[2], "1")]);
    let r = result(&o);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED", "{}", show(&o));
    assert_eq!(r["round_label"], "round-1");
    assert!(fx.evaluator_calls().is_empty());
    let summary = fx.state_file("autodrive-budget-summary.md").unwrap();
    assert!(
        summary.contains("Limits reached: autodrive_budget_wall_seconds"),
        "{summary}"
    );
}

fn usage_rows(fx: &Fixture) -> Vec<Vec<String>> {
    fx.state_file("autodrive-usage.tsv")
        .expect("usage file")
        .lines()
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// Issue #1977: the wall budget leaves usage-limit waits out. Both rows are
/// driven through `autodrive_budget.sh` with the child's duration and the
/// ledger given explicitly, so nothing here depends on how long anything
/// took on this host: the same 5 s child reaches a 3 s wall limit with no
/// wait, and stays under it once a 3 s wait is taken out. Dropping the
/// subtraction in `add` makes the first half fail.
#[test]
fn a_usage_limit_wait_does_not_spend_the_wall_budget() {
    let dir = tempfile::tempdir().unwrap();
    let wall = &["autodrive_budget_wall_seconds=3"];

    // A 5 s child that waited 3 s leaves 2 s of wall time, under the limit.
    let waited = dir.path().join("waited");
    private_dir(&waited);
    let ledger = waited.join(".wait-crusty-round-1-round");
    std::fs::write(&ledger, "3\n").unwrap();
    let (w, l) = (waited.display().to_string(), ledger.display().to_string());
    let o = helper(
        "autodrive_budget.sh",
        &["add", &w, "crusty", "round-1", "round", "5", "--waited", &l],
    );
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(
        std::fs::read_to_string(waited.join("autodrive-usage.tsv")).unwrap(),
        "crusty\tround-1\tround\t1\t2\tunknown\n\
         crusty\tround-1\twait\t0\t3\tunknown\n"
    );
    let o = helper(
        "autodrive_budget.sh",
        &[&["check", &w, "crusty", "post", "round-1", "7"][..], wall].concat(),
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "2 s of wall time is under the 3 s limit: {}",
        show(&o)
    );
    assert!(
        !waited.join("autodrive-budget-summary.md").exists(),
        "no limit was reached, so no summary is written"
    );

    // The control: the same child with no wait spends all 5 s and reaches it.
    let spent = dir.path().join("spent");
    private_dir(&spent);
    let s = spent.display().to_string();
    let o = helper(
        "autodrive_budget.sh",
        &["add", &s, "crusty", "round-1", "round", "5"],
    );
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(
        std::fs::read_to_string(spent.join("autodrive-usage.tsv")).unwrap(),
        "crusty\tround-1\tround\t1\t5\tunknown\n"
    );
    let o = helper(
        "autodrive_budget.sh",
        &[&["check", &s, "crusty", "post", "round-1", "7"][..], wall].concat(),
    );
    assert_eq!(o.status.code(), Some(10), "{}", show(&o));
    let summary = std::fs::read_to_string(spent.join("autodrive-budget-summary.md")).unwrap();
    assert!(
        summary.contains("Limits reached: autodrive_budget_wall_seconds\n"),
        "{summary}"
    );
    assert!(summary.contains("| wall seconds | 5 | 3 |"), "{summary}");
    assert!(
        summary.contains("| usage-limit wait seconds | 0 | not counted |"),
        "{summary}"
    );
}

/// The loop itself hands the ledger to `add` and clears it afterwards: one
/// `wait` row of the waited seconds, the rounds limit the only one reached
/// (the wall limit is far out of reach, whatever this host's speed), and no
/// ledger left behind. What the wall budget does with that row is settled by
/// `a_usage_limit_wait_does_not_spend_the_wall_budget` above.
#[test]
fn a_usage_limit_wait_is_recorded_as_its_own_row_and_the_ledger_is_cleared() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a|3"]);
    let o = run_with(
        &fx,
        "crusty",
        &[(BUDGET_KEYS[0], "1"), (BUDGET_KEYS[2], "3600")],
    );
    let r = result(&o);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED", "{}", show(&o));
    let summary = fx.state_file("autodrive-budget-summary.md").unwrap();
    let rows = usage_rows(&fx);
    assert!(
        summary.contains("Limits reached: autodrive_budget_loop_rounds\n"),
        "the rounds limit is the only one reached:\n{summary}"
    );
    assert!(
        summary.contains("| usage-limit wait seconds | 3 | not counted |"),
        "{summary}"
    );
    let wait: Vec<_> = rows.iter().filter(|r| r[2] == "wait").collect();
    assert_eq!(wait.len(), 1, "{rows:?}");
    assert_eq!(
        wait[0][..5],
        ["crusty", "round-1", "wait", "0", "3"],
        "{rows:?}"
    );
    // The summary's wall seconds are the round row alone: the wait row is
    // reported apart and never added to it.
    let round = rows.iter().find(|r| r[2] == "round").expect("round row");
    assert!(
        summary.contains(&format!("| wall seconds | {} | 3600 |", round[4])),
        "{summary}\n{rows:?}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&fx.state_file("autodrive-usage.json").unwrap()).unwrap();
    assert_eq!(json["total"]["wait_seconds"], 3);
    assert_eq!(json["total"]["sessions"], 1, "a wait is not a session");
    assert_eq!(json["loops"]["crusty"]["wait_seconds"], 3);
    assert!(
        std::fs::read_dir(&fx.state).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".wait-")),
        "a wait ledger was left in the state dir"
    );
}

#[test]
fn a_wait_ledger_never_takes_out_more_than_the_child_ran() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    private_dir(&state);
    let ledger = state.join(".wait-crusty-round-1-round");
    std::fs::write(&ledger, "999999\nnot-a-number\n").unwrap();
    let st = state.display().to_string();
    let l = ledger.display().to_string();
    // Supply the child's measured duration explicitly so the credit must
    // equal that duration, independent of scheduling or clock granularity.
    let o = helper(
        "autodrive_budget.sh",
        &[
            "add", &st, "crusty", "round-1", "round", "7", "--waited", &l,
        ],
    );
    assert!(o.status.success(), "{}", show(&o));
    assert!(
        stderr(&o).contains("malformed line(s) in the usage-limit wait ledger"),
        "{}",
        show(&o)
    );
    let tsv = std::fs::read_to_string(state.join("autodrive-usage.tsv")).unwrap();
    assert_eq!(
        tsv,
        "crusty\tround-1\tround\t1\t0\tunknown\n\
         crusty\tround-1\twait\t0\t7\tunknown\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("autodrive-usage.json")).unwrap())
            .unwrap();
    assert_eq!(json["total"]["seconds"], 0);
    assert_eq!(json["total"]["wait_seconds"], 7);
}

#[test]
fn an_untrusted_wait_ledger_subtracts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    private_dir(&state);
    let outside = dir.path().join("outside");
    std::fs::write(&outside, "50\n").unwrap();
    let ledger = state.join(".wait-crusty-round-1-round");
    symlink(&outside, &ledger).unwrap();
    let st = state.display().to_string();
    let l = ledger.display().to_string();
    let o = helper(
        "autodrive_budget.sh",
        &[
            "add", &st, "crusty", "round-1", "round", "60", "--waited", &l,
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    assert!(stderr(&o).contains("WARNING"), "{}", show(&o));
    let tsv = std::fs::read_to_string(state.join("autodrive-usage.tsv")).unwrap();
    assert_eq!(tsv, "crusty\tround-1\tround\t1\t60\tunknown\n");
}

#[test]
fn a_nested_loops_waits_are_not_counted_again_by_the_outer_step() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    private_dir(&state);
    let st = state.display().to_string();
    let add = |a: &[&str]| {
        let mut all = vec!["add", st.as_str()];
        all.extend_from_slice(a);
        let o = helper("autodrive_budget.sh", &all);
        assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    };
    let ledger = state.join("nested-ledger");
    std::fs::write(&ledger, "40\n").unwrap();
    let l = ledger.display().to_string();
    // A nested crusty round of 50 s, 40 of them waiting; then the outer
    // before-round step that ran it for 55 s in all.
    add(&["crusty", "round-1", "round", "50", "--waited", &l]);
    add(&["merge-ready", "round-1", "before", "55", "--since", "0"]);
    let tsv = std::fs::read_to_string(state.join("autodrive-usage.tsv")).unwrap();
    assert_eq!(
        tsv,
        "crusty\tround-1\tround\t1\t10\tunknown\n\
         crusty\tround-1\twait\t0\t40\tunknown\n\
         merge-ready\tround-1\tbefore\t0\t5\tunknown\n"
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(state.join("autodrive-usage.json")).unwrap())
            .unwrap();
    assert_eq!(json["total"]["seconds"], 15);
    assert_eq!(json["total"]["wait_seconds"], 40);
}

#[test]
fn a_clean_round_is_not_stopped_after_it_ran() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CLEAN|0|0|0|"]);
    fx.set("eval", "done");
    let done = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "1")]);
    assert_eq!(done.status.code(), Some(0), "{}", show(&done));
    assert_eq!(result(&done)["loop_result"], "DONE");
    assert!(fx.evaluator_calls().is_empty());
    assert_eq!(result(&done)["source"], "objective-completion");
}

/// PR #1862 review: a round stopped as STUCK appends its own row, so a
/// relaunch that stops at the pre-round budget check names that round and
/// its open items, not the round before it.
#[test]
fn a_resume_after_a_stuck_stop_names_the_stuck_round_in_the_summary() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|0|a", "CONCERNS|0|0|0|b"]);
    let o = fx.run("crusty", &[]);
    let r = result(&o);
    assert_eq!(r["loop_result"], "STUCK", "{}", show(&o));
    assert_eq!(r["round_label"], "round-3");
    let rows = fx.state_file("crusty-progress.tsv").unwrap();
    assert_eq!(
        rows.lines().count(),
        3,
        "the STUCK round has a row:\n{rows}"
    );
    assert!(
        rows.lines().last().unwrap().starts_with("round-3\t"),
        "{rows}"
    );

    let again = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "3")]);
    let r = result(&again);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED", "{}", show(&again));
    assert_eq!(r["round_label"], "round-3");
    assert_eq!(fx.round_calls().len(), 3, "a pre-round stop runs no round");
    let summary = fx.state_file("autodrive-budget-summary.md").unwrap();
    assert!(
        summary.contains("at round label `round-3` (pre-round check)"),
        "{summary}"
    );
    assert!(summary.contains("## Open items of round-3"), "{summary}");
}

#[test]
fn a_continued_clean_round_stops_at_the_next_pre_round_check() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["MERGE_READY|0|0|0|"]);
    let o = run_with(&fx, "merge-ready", &[(BUDGET_KEYS[0], "1")]);
    let r = result(&o);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED", "{}", show(&o));
    assert_eq!(r["round_label"], "round-1");
    assert_eq!(fx.evaluator_calls(), ["round-1"]);
    assert_eq!(fx.round_calls(), ["round-1"]);
}

#[test]
fn zero_and_empty_limits_are_unlimited() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    fx.set("eval_stuck_after", "3");
    let o = run_with(
        &fx,
        "crusty",
        &[
            (BUDGET_KEYS[0], "0"),
            (BUDGET_KEYS[1], ""),
            (BUDGET_KEYS[2], "0"),
        ],
    );
    assert_eq!(result(&o)["loop_result"], "STUCK", "{}", show(&o));
    assert_eq!(fx.round_calls().len(), 4);
    assert!(fx.state_file("autodrive-budget-summary.md").is_none());
}

#[test]
fn limits_are_read_in_base_ten() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    fx.set("eval_stuck_after", "20");
    // `08` is an invalid octal literal to bash arithmetic.
    let o = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "08")]);
    let r = result(&o);
    assert_eq!(r["loop_result"], "BUDGET_EXHAUSTED", "{}", show(&o));
    assert_eq!(r["round_label"], "round-8");
}

#[test]
fn malformed_limits_and_unknown_keys_exit_2_before_any_round() {
    for (key, value) in [
        (BUDGET_KEYS[0], "abc"),
        (BUDGET_KEYS[0], "-1"),
        (BUDGET_KEYS[1], "1e3"),
        (BUDGET_KEYS[2], "1234567890123"),
        (BUDGET_KEYS[2], "a[$(touch pwned)]"),
        ("autodrive_budget_tokens", "5"),
    ] {
        let Some(fx) = Fixture::new() else { return };
        let o = run_with(&fx, "crusty", &[(key, value)]);
        assert_eq!(o.status.code(), Some(2), "{key}={value}\n{}", show(&o));
        assert!(
            stderr(&o).contains(key),
            "the error names {key}\n{}",
            show(&o)
        );
        assert!(fx.round_calls().is_empty(), "{key}={value}: a round ran");
        assert!(!fx.cwd.join("pwned").exists() && !fx.repo.join("pwned").exists());
    }
}

#[test]
fn budget_keys_never_reach_a_round_recipe() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    run_with(
        &fx,
        "crusty",
        &[
            (BUDGET_KEYS[0], "2"),
            (BUDGET_KEYS[1], "20"),
            (BUDGET_KEYS[2], "14400"),
        ],
    );
    let argvs = fx.recipe_argvs();
    assert!(!argvs.is_empty());
    for argv in argvs {
        assert!(
            argv.iter().all(|a| !a.contains("autodrive_budget")),
            "a budget key reached a child: {argv:?}"
        );
    }
}

#[test]
fn usage_is_recorded_per_child_with_tokens_unknown() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "2")]);
    let tsv = fx.state_file("autodrive-usage.tsv").expect("usage file");
    let rows: Vec<Vec<&str>> = tsv.lines().map(|l| l.split('\t').collect()).collect();
    assert!(rows.iter().all(|r| r.len() == 6), "{tsv}");
    assert!(
        rows.iter().all(|r| r[0] == "crusty" && r[5] == "unknown"),
        "{tsv}"
    );
    let phases: Vec<&str> = rows.iter().map(|r| r[2]).collect();
    assert_eq!(phases.iter().filter(|p| **p == "round").count(), 2, "{tsv}");
    assert_eq!(
        phases.iter().filter(|p| **p == "evaluator").count(),
        1,
        "{tsv}"
    );
    assert!(
        rows.iter()
            .all(|r| r[3].parse::<u64>().is_ok() && r[4].parse::<u64>().is_ok())
    );
    let json: serde_json::Value =
        serde_json::from_str(&fx.state_file("autodrive-usage.json").expect("totals")).unwrap();
    assert_eq!(json["total"]["rounds"], 2);
    assert_eq!(json["total"]["tokens"], "unknown");
    assert_eq!(json["loops"]["crusty"]["rounds"], 2);
    for f in [
        "autodrive-usage.tsv",
        "autodrive-usage.json",
        "crusty-result.json",
    ] {
        assert_eq!(fx.mode(f), 0o600, "{f}");
    }
}

#[test]
fn an_untrusted_usage_file_exits_2_and_starts_nothing() {
    for kind in ["symlink", "0644"] {
        let Some(fx) = Fixture::new() else { return };
        let usage = fx.state.join("autodrive-usage.tsv");
        if kind == "symlink" {
            let outside = fx.tmp.path().join("usage-outside.tsv");
            std::fs::write(&outside, "").unwrap();
            symlink(&outside, &usage).unwrap();
        } else {
            std::fs::write(&usage, "").unwrap();
            std::fs::set_permissions(&usage, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let o = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "5")]);
        assert_eq!(o.status.code(), Some(2), "{kind}\n{}", show(&o));
        assert!(stderr(&o).contains("ERROR"), "{kind}\n{}", show(&o));
        assert!(
            !stdout(&o).contains("loop_result"),
            "{kind}: no result line"
        );
        assert!(fx.state_file("crusty-result.json").is_none(), "{kind}");
        assert!(fx.round_calls().is_empty(), "{kind}: a round ran");
        assert!(fx.gh_calls().is_empty(), "{kind}: a comment was posted");
    }
}

#[test]
fn malformed_usage_rows_are_skipped_with_a_warning() {
    let Some(fx) = Fixture::new() else { return };
    private_file(
        &fx.state.join("autodrive-usage.tsv"),
        "crusty\tround-1\tround\t1\t$(touch pwned)\tunknown\ngarbage\n",
    );
    fx.plan(&["CONCERNS|1|0|0|a"]);
    let o = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "1")]);
    assert!(stderr(&o).contains("WARNING"), "{}", show(&o));
    assert_eq!(result(&o)["loop_result"], "BUDGET_EXHAUSTED");
    assert!(!fx.cwd.join("pwned").exists() && !fx.state.join("pwned").exists());
}

// ── Before-round step and the nested crusty loop ─────────────────────────────

fn before_script(fx: &Fixture, body: &str) -> String {
    let p = fx.tmp.path().join("before.sh");
    std::fs::write(&p, body).unwrap();
    p.display().to_string()
}

#[test]
fn a_nested_budget_stop_is_reported_as_budget_exhausted() {
    let Some(fx) = Fixture::new() else { return };
    let b = before_script(
        &fx,
        "echo '{\"before_round_result\":\"crusty-rereview-budget-exhausted\"}'\nexit 1\n",
    );
    let o = fx.run("merge-ready", &["--before-round", &b]);
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert_eq!(
        result(&o)["loop_result"],
        "BUDGET_EXHAUSTED",
        "{}",
        show(&o)
    );
    assert!(fx.round_calls().is_empty());
    let summary = fx
        .state_file("autodrive-budget-summary.md")
        .expect("summary");
    assert!(summary.contains("(before-round step)"), "{summary}");
    // The nested crusty loop posts its own stop; the outer loop does not post it again.
    assert!(fx.gh_calls().is_empty(), "a second comment was posted");
}

#[test]
fn an_ordinary_before_round_failure_keeps_its_result() {
    let Some(fx) = Fixture::new() else { return };
    let b = before_script(
        &fx,
        "echo '{\"before_round_result\":\"crusty-rereview-failed\"}'\nexit 1\n",
    );
    let o = fx.run("merge-ready", &["--before-round", &b]);
    assert_eq!(
        result(&o)["loop_result"],
        "BEFORE_ROUND_FAILED",
        "{}",
        show(&o)
    );
    assert!(fx.state_file("merge-ready-result.json").is_some());
}

#[test]
fn the_before_round_step_is_timed_as_its_own_phase() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    let b = before_script(&fx, "exit 0\n");
    let a = args(&[(BUDGET_KEYS[0], "1")]);
    let mut all: Vec<&str> = vec!["--before-round", &b];
    all.extend(a.iter().map(String::as_str));
    fx.run("merge-ready", &all);
    let tsv = fx.state_file("autodrive-usage.tsv").expect("usage file");
    assert!(
        tsv.lines()
            .any(|l| l.starts_with("merge-ready\tround-1\tbefore\t")),
        "{tsv}"
    );
}

// ── PR comments ──────────────────────────────────────────────────────────────

#[test]
fn the_stop_comment_is_posted_through_gh_and_sanitised() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|0|ping @example-user,see #123,```evil,file:/srv/private/secret"]);
    let o = fx.run("crusty", &[]);
    assert!(
        result(&o)["reason"].as_str().unwrap().contains(DECIDED),
        "{}",
        show(&o)
    );
    let calls = fx.gh_calls();
    assert_eq!(calls.len(), 1, "{}", show(&o));
    let (argv, body, env) = &calls[0];
    assert_eq!(
        &argv[..4],
        ["pr", "comment", "1743", "--body-file"],
        "{argv:?}"
    );
    assert_eq!(argv.len(), 5, "{argv:?}");
    assert_eq!(env, "1", "GH_PROMPT_DISABLED");
    assert!(!body.contains("@example-user"), "{body}");
    assert!(!body.contains("#123"), "{body}");
    assert!(!body.contains("```evil"), "{body}");
    assert!(
        !body.contains(&fx.state.display().to_string()),
        "absolute paths: {body}"
    );
    // PR #1862 review: a path after a colon is cut to its base name too.
    assert!(!body.contains("/srv/private"), "{body}");
    assert!(body.contains("file:secret"), "{body}");
    assert!(body.len() <= 60_000);
}

#[test]
fn the_budget_comment_carries_the_summary() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|1|0|0|a"]);
    run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "1")]);
    let calls = fx.gh_calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].1.contains("autodrive_budget_loop_rounds"),
        "{}",
        calls[0].1
    );
}

#[test]
fn no_comment_without_a_valid_pr_number() {
    for pr in [None, Some("17;43"), Some("12345678901")] {
        let Some(fx) = Fixture::new() else { return };
        fx.plan(&["CONCERNS|0|0|0|a"]);
        let o = fx.run_pr("crusty", pr, &[]);
        assert_eq!(result(&o)["loop_result"], "STUCK", "{pr:?}\n{}", show(&o));
        assert!(fx.gh_calls().is_empty(), "{pr:?}");
    }
}

#[test]
fn a_failed_comment_warns_and_keeps_the_result() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|0|a"]);
    fx.set("gh_rc", "7");
    let o = fx.run("crusty", &[]);
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(result(&o)["loop_result"], "STUCK");
    let err = stderr(&o);
    assert!(
        err.lines()
            .any(|l| l.contains("WARNING") && l.contains('7')),
        "{err}"
    );
}

/// Runs `autodrive_budget.sh post` on `text` with the stub `gh` first on
/// `PATH`, and returns the comment body the stub received.
fn posted(text: &str) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let st = tmp.path().join("state");
    private_dir(&st);
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::support::exec_stub::write_executable(&bin.join("gh"), GH_STUB, 0o755);
    let body = findings(tmp.path(), "reason.txt", text);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = crate::isolated_amplihack::isolated("bash")
        .arg(tool("autodrive_budget.sh"))
        .args(["post", &st.display().to_string(), "1743", &body])
        .env("PATH", path)
        .env("STUB_STATE", tmp.path())
        .env("STUB_LOG", tmp.path())
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    std::fs::read_to_string(tmp.path().join("gh-body-1")).expect("no comment body")
}

/// PR #1862 review (crusty round 4, #1865 item 1): the stop comment goes to
/// a public PR, so every credential format the findings could quote is
/// masked before posting. One case per format. The secrets are assembled at
/// run time so the test file itself holds none.
#[test]
fn the_stop_comment_masks_secrets_one_case_per_format() {
    let a = |n: usize| "A1b2C3d4E5".repeat(n.div_ceil(10))[..n].to_string();
    let up = |n: usize| "Q7W3R9T2Y5".repeat(n.div_ceil(10))[..n].to_string();
    let cases: Vec<(&str, String)> = vec![
        ("ghp_", format!("gh{}_{}", "p", a(36))),
        ("gho_", format!("gh{}_{}", "o", a(36))),
        ("ghu_", format!("gh{}_{}", "u", a(36))),
        ("ghs_", format!("gh{}_{}", "s", a(36))),
        ("ghr_", format!("gh{}_{}", "r", a(36))),
        (
            "github_pat_",
            format!("github_{}_{}_{}", "pat", a(22), a(59)),
        ),
        ("anthropic sk-ant-", format!("sk-{}-api03-{}", "ant", a(40))),
        ("openai sk-proj-", format!("sk-{}-{}", "proj", a(40))),
        ("openai sk-", format!("sk-{}", a(48))),
        ("aws AKIA", format!("AK{}{}", "IA", up(16))),
        ("aws ASIA", format!("AS{}{}", "IA", up(16))),
        (
            "slack xoxb-",
            format!("xo{}-{}-{}", "xb", "1234567890", a(24)),
        ),
        ("gitlab glpat-", format!("glp{}-{}", "at", a(20))),
        ("npm_", format!("np{}_{}", "m", a(36))),
        ("google AIza", format!("AI{}{}", "za", a(35))),
        (
            "jwt",
            format!("ey{}.{}.{}", "JhbGciOiJIUzI1NiJ9", a(20), a(30)),
        ),
    ];
    for (name, secret) in &cases {
        let body = posted(&format!("crusty quoted: found {secret} in the log\n"));
        assert!(!body.contains(secret.as_str()), "{name}: {body}");
        assert!(
            body.contains("found [REDACTED] in the log"),
            "{name}: {body}"
        );
    }

    // Shapes that only their context gives away.
    let v = a(24);
    let contextual: Vec<(&str, String, &str)> = vec![
        (
            "bearer header",
            format!("Authorization: Bearer {v}"),
            "Authorization: Bearer [REDACTED]",
        ),
        (
            "url credentials",
            format!("https://x-access-token:{v}(at)github.com/o/r.git"),
            "https://[REDACTED]",
        ),
        (
            "env token",
            format!("GITHUB_TOKEN={v}"),
            "GITHUB_TOKEN=[REDACTED]",
        ),
        (
            "env secret key",
            format!("AWS_SECRET_ACCESS_KEY={v}"),
            "AWS_SECRET_ACCESS_KEY=[REDACTED]",
        ),
        (
            "json password",
            format!(r#"{{"password": "{v} x"}}"#),
            r#""password": "[REDACTED]""#,
        ),
        (
            "yaml api key",
            format!("api_key: {v}"),
            "api_key: [REDACTED]",
        ),
        (
            "azure account key",
            format!("AccountKey={v}==;EndpointSuffix=core"),
            "AccountKey=[REDACTED];EndpointSuffix=core",
        ),
        (
            "sas signature",
            format!("?sv=2024&sig={v}"),
            "sig=[REDACTED]",
        ),
        (
            "aws presigned url signature",
            format!("?X-Amz-Expires=60&X-Amz-Signature={v}&x=1"),
            "X-Amz-Signature=[REDACTED]&x=1",
        ),
        (
            "aws signature header",
            format!("x-amz-signature: {v}"),
            "x-amz-signature: [REDACTED]",
        ),
        (
            "basic header",
            format!("Authorization: Basic {v}=="),
            "Authorization: Basic [REDACTED]",
        ),
        (
            "bearer in prose",
            format!("retried with bearer {v} and failed"),
            "with bearer [REDACTED] and failed",
        ),
    ];
    for (name, text, kept) in &contextual {
        let text = text.replace("(at)", "@");
        let body = posted(&format!("{text}\n"));
        assert!(!body.contains(&v), "{name}: {body}");
        assert!(body.contains(kept), "{name}: {body}");
    }

    // In an Authorization header the credentials are masked even when they
    // are letters only, which the prose-safe Bearer/Basic/token rule skips.
    let letters = "QwErTyUiOpAsDfGhJkLzXcVb";
    let body = posted(&format!("Authorization: Bearer {letters}\n"));
    assert!(!body.contains(letters), "{body}");
    assert!(body.contains("Authorization: Bearer [REDACTED]"), "{body}");

    // PR #1862 review (crusty round 6): an Authorization value with no
    // scheme word, or written with `=`, is masked too.
    let raw = "s3cr3tvaluewithnoscheme123";
    let body = posted(&format!("Authorization: {raw}\n"));
    assert!(!body.contains(raw), "schemeless: {body}");
    assert!(
        body.contains("Authorization: [REDACTED]"),
        "schemeless: {body}"
    );
    let body = posted(&format!("Proxy-Authorization: Custom {letters} tail\n"));
    assert!(!body.contains(letters), "unknown scheme: {body}");
    assert!(
        body.contains("Proxy-Authorization: [REDACTED]"),
        "unknown scheme: {body}"
    );
    let body = posted(&format!("authorization=\"Bearer {letters}\"\n"));
    assert!(!body.contains(letters), "authorization=: {body}");
    assert!(
        body.contains("authorization=\"[REDACTED]\""),
        "authorization=: {body}"
    );
    let body = posted(&format!("authorization={raw}&next=1\n"));
    assert!(!body.contains(raw), "unquoted authorization=: {body}");
    assert!(
        body.contains("authorization=[REDACTED]"),
        "unquoted authorization=: {body}"
    );

    // A private key block, whole and cut off by the 8 KiB findings limit.
    let pem = format!(
        "before\n-----BEGIN {k}-----\n{v}\n{v}\n-----END {k}-----\nafter\n",
        k = "RSA PRIVATE KEY"
    );
    let body = posted(&pem);
    assert!(!body.contains(&v), "{body}");
    assert!(
        body.contains("before\n[REDACTED PRIVATE KEY]\nafter"),
        "{body}"
    );
    let body = posted(&format!(
        "before\n-----BEGIN {}-----\n{v}\n",
        "OPENSSH PRIVATE KEY"
    ));
    assert!(!body.contains(&v), "a cut-off block: {body}");

    // The masking reaches the comment end to end, through the STUCK stop.
    let Some(fx) = Fixture::new() else { return };
    let token = format!("gh{}_{}", "p", a(36));
    fx.plan(&[&format!("CONCERNS|0|0|0|leaked {token}")]);
    fx.run("crusty", &[]);
    let calls = fx.gh_calls();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].1.contains(&token), "{}", calls[0].1);
    assert!(calls[0].1.contains("leaked [REDACTED]"), "{}", calls[0].1);
}

/// Masking must not eat the text a reader needs: SHAs, counts and prose.
#[test]
fn the_stop_comment_keeps_ordinary_text() {
    let text = format!(
        "head {SHA}\n12 tokens used; the token budget is unlimited\ndesign=simple, branch: feat/x\nthe basic implementation is fine\nToken authentication and bearer authentication differ\n"
    );
    let body = posted(&text);
    assert!(body.contains(SHA), "{body}");
    assert!(
        body.contains("12 tokens used; the token budget is unlimited"),
        "{body}"
    );
    assert!(body.contains("design=simple, branch: feat/x"), "{body}");
    assert!(body.contains("the basic implementation is fine"), "{body}");
    assert!(
        body.contains("Token authentication and bearer authentication differ"),
        "{body}"
    );
    assert!(!body.contains("[REDACTED]"), "{body}");
}

/// PR #1862 review (crusty round 6): the masking ships to macOS, where sed
/// is BSD sed. `\xHH` is a GNU extension; BSD sed would read `[\x01\x02]`
/// as the characters `\`, `x`, `0`, `1`, `2` and delete them from the
/// comment. The marker bytes must come from printf, and no sed expression
/// may use a `\x` escape. CI runs on Linux only, so the source is checked.
#[test]
fn mask_secrets_uses_no_gnu_only_sed_escapes() {
    let src = std::fs::read_to_string(tool("autodrive_budget.sh")).unwrap();
    let code: Vec<&str> = src
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect();
    for l in &code {
        assert!(!l.contains("\\x"), "GNU-only \\x escape: {l}");
    }
    assert!(code.iter().any(|l| l.contains(r#"M1="$(printf '\001')""#)));
    assert!(code.iter().any(|l| l.contains(r#"M2="$(printf '\002')""#)));

    // Text full of the characters BSD sed would have stripped survives.
    let text = "commit 0x1a2b 2021-12-01 xx10\n";
    let body = posted(text);
    assert!(body.contains("commit 0x1a2b 2021-12-01 xx10"), "{body}");
}

// ── Helpers on their own ─────────────────────────────────────────────────────

fn helper(name: &str, a: &[&str]) -> Output {
    crate::isolated_amplihack::isolated("bash")
        .arg(tool(name))
        .args(a)
        .output()
        .expect("spawn helper")
}

fn findings(dir: &Path, name: &str, body: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, body).unwrap();
    p.display().to_string()
}

const SHA: &str = "3f9c2a1e6b0d4c7a8e5f1b2c9d0a3e4f5b6c7d8e";

/// The sha256 of a joined open-item set, as the last column of a row.
fn h(joined: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(joined.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// PR #1862 review: two sets that share the first 3,700 bytes (the display
/// cut) differ, because the rule compares the hash of the full set. A row
/// with no hash column (written before it existed) compares the cut text.
#[test]
fn progress_helper_compares_the_full_set_by_hash_not_the_cut_text() {
    let tmp = tempfile::tempdir().unwrap();
    let st = tmp.path().join("state");
    private_dir(&st);
    let s = st.display().to_string();
    let common: String = (0..30)
        .map(|i| format!("{i:02}-{}\n", "x".repeat(190)))
        .collect();
    let a = findings(tmp.path(), "a", &format!("{common}zz-last-a\n"));
    let b = findings(tmp.path(), "b", &format!("{common}zz-last-b\n"));
    let o = helper(
        "autodrive_progress.sh",
        &["record", &s, "m", "round-1", SHA, "0", &a],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let rows = std::fs::read_to_string(st.join("m-progress.tsv")).unwrap();
    assert!(
        rows.len() <= 4096,
        "a row stays within 4 KiB: {}",
        rows.len()
    );
    let cols: Vec<&str> = rows.trim_end().split('\t').collect();
    assert!(cols[3].ends_with("…(truncated)"), "{rows}");
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "m", "round-2", SHA, &b],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "a different full set\n{}",
        show(&o)
    );
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "m", "round-2", SHA, &a],
    );
    assert_eq!(o.status.code(), Some(10), "the same full set\n{}", show(&o));

    // A row without the hash column falls back to the cut text.
    std::fs::write(
        st.join("o-progress.tsv"),
        format!("round-1\t{SHA}\t0\tq\t\n"),
    )
    .unwrap();
    std::fs::set_permissions(
        st.join("o-progress.tsv"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let q = findings(tmp.path(), "q", "q\n");
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "o", "round-2", SHA, &q],
    );
    assert_eq!(o.status.code(), Some(10), "{}", show(&o));
}

#[test]
fn progress_helper_records_and_checks_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let st = tmp.path().join("state");
    private_dir(&st);
    let s = st.display().to_string();
    let f1 = findings(tmp.path(), "f1", "b\r\n  a \n\na\n");
    let o = helper(
        "autodrive_progress.sh",
        &["record", &s, "crusty", "round-1", SHA, "0", &f1],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let rows = std::fs::read_to_string(st.join("crusty-progress.tsv")).unwrap();
    assert_eq!(rows, format!("round-1\t{SHA}\t0\ta,b\t\t{}\n", h("a,b")));
    let mode = std::fs::metadata(st.join("crusty-progress.tsv"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);

    let same = findings(tmp.path(), "f2", "a\nb\n");
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "crusty", "round-2", SHA, &same],
    );
    assert_eq!(o.status.code(), Some(10), "{}", show(&o));
    let reason = std::fs::read_to_string(st.join("crusty-stuck-reason.txt")).unwrap();
    assert_eq!(
        reason.trim_end(),
        format!("{DECIDED}: head {SHA} and open items unchanged since round-1: a,b")
    );

    for (label, sha, items) in [
        (
            "other sha",
            "0000000000000000000000000000000000000000",
            "a\nb\n",
        ),
        ("empty sha", "", "a\nb\n"),
        ("other items", SHA, "a\n"),
    ] {
        let f = findings(tmp.path(), "f3", items);
        let o = helper(
            "autodrive_progress.sh",
            &["check", &s, "crusty", "round-2", sha, &f],
        );
        assert_eq!(o.status.code(), Some(0), "{label}\n{}", show(&o));
    }
}

/// PR #1862 review: a merge round with nothing to commit while CI changes or
/// is still running made progress or is waiting; it is never STUCK in code.
#[test]
fn progress_helper_counts_a_ci_change_as_progress_and_never_stops_on_pending_ci() {
    let tmp = tempfile::tempdir().unwrap();
    let st = tmp.path().join("state");
    private_dir(&st);
    let s = st.display().to_string();
    let f = findings(tmp.path(), "f", "ci-red\n");
    let o = helper(
        "autodrive_progress.sh",
        &[
            "record",
            &s,
            "merge",
            "round-1",
            SHA,
            "0",
            &f,
            "PENDING:0/9",
        ],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let rows = std::fs::read_to_string(st.join("merge-progress.tsv")).unwrap();
    assert_eq!(
        rows,
        format!("round-1\t{SHA}\t0\tci-red\tPENDING:0/9\t{}\n", h("ci-red"))
    );
    for (label, ci) in [
        ("pending CI is waited on", "PENDING:0/9"),
        ("a CI change is progress", "RED:1/9"),
        ("no CI signal differs from one", ""),
    ] {
        let o = helper(
            "autodrive_progress.sh",
            &["check", &s, "merge", "round-2", SHA, &f, ci],
        );
        assert_eq!(o.status.code(), Some(0), "{label}\n{}", show(&o));
    }
    helper(
        "autodrive_progress.sh",
        &["record", &s, "merge", "round-2", SHA, "0", &f, "RED:1/9"],
    );
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "merge", "round-3", SHA, &f, "RED:1/9"],
    );
    assert_eq!(
        o.status.code(),
        Some(10),
        "same head, items and settled CI is no progress\n{}",
        show(&o)
    );
}

#[test]
fn progress_helper_never_matches_an_empty_sha_or_a_clean_row() {
    let tmp = tempfile::tempdir().unwrap();
    let st = tmp.path().join("state");
    private_dir(&st);
    let s = st.display().to_string();
    let f = findings(tmp.path(), "f", "");
    helper(
        "autodrive_progress.sh",
        &["record", &s, "m", "round-1", "", "0", &f],
    );
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "m", "round-2", "", &f],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    helper(
        "autodrive_progress.sh",
        &["record", &s, "m", "round-2", SHA, "1", &f],
    );
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "m", "round-3", SHA, &f],
    );
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let missing = tmp.path().join("missing").display().to_string();
    helper(
        "autodrive_progress.sh",
        &["record", &s, "m", "round-3", SHA, "0", &missing],
    );
    let o = helper(
        "autodrive_progress.sh",
        &["check", &s, "m", "round-4", SHA, &missing],
    );
    assert_eq!(
        o.status.code(),
        Some(10),
        "a missing findings file is the empty set"
    );
    let reason = std::fs::read_to_string(st.join("m-stuck-reason.txt")).unwrap();
    assert!(
        reason.trim_end().ends_with("since round-3: none"),
        "{reason}"
    );
}

#[test]
fn progress_helper_neutralises_separators_and_refuses_symlinks() {
    let tmp = tempfile::tempdir().unwrap();
    let st = tmp.path().join("state");
    private_dir(&st);
    let s = st.display().to_string();
    let f = findings(tmp.path(), "f", "x,y\nt\tu\n");
    helper(
        "autodrive_progress.sh",
        &["record", &s, "c", "round-1", SHA, "0", &f],
    );
    let rows = std::fs::read_to_string(st.join("c-progress.tsv")).unwrap();
    assert_eq!(
        rows,
        format!("round-1\t{SHA}\t0\tt_u,x_y\t\t{}\n", h("t_u,x_y"))
    );

    let outside = tmp.path().join("outside");
    std::fs::write(&outside, "kept\n").unwrap();
    symlink(&outside, st.join("d-progress.tsv")).unwrap();
    helper(
        "autodrive_progress.sh",
        &["record", &s, "d", "round-1", SHA, "0", &f],
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "kept\n");
}

#[test]
fn new_helpers_reject_bad_usage_with_exit_2() {
    for (name, a) in [
        ("autodrive_progress.sh", vec![]),
        ("autodrive_progress.sh", vec!["bogus"]),
        ("autodrive_progress.sh", vec!["check", "/nonexistent"]),
        ("autodrive_budget.sh", vec![]),
        ("autodrive_budget.sh", vec!["bogus"]),
    ] {
        let o = helper(name, &a);
        assert_eq!(o.status.code(), Some(2), "{name} {a:?}\n{}", show(&o));
    }
}

#[test]
fn escalate_has_a_budget_wording() {
    let o = crate::isolated_amplihack::isolated("bash")
        .arg("-c")
        .arg(r#". "$1"; LOOP_NAME=crusty ROUND_LABEL=round-3 RECORD=r; escalate BUDGET_EXHAUSTED "why" budget"#)
        .arg("x")
        .arg(tool("autodrive_escalate.sh"))
        .output()
        .unwrap();
    let err = stderr(&o);
    assert!(
        err.contains("This is a usage budget set by the operator"),
        "{err}"
    );
    assert!(!err.contains("absence of progress"), "{err}");
    assert!(stdout(&o).contains(r#""loop_result":"BUDGET_EXHAUSTED""#));
}

// ── Recipes and structure ────────────────────────────────────────────────────

fn yaml_str(v: &serde_yaml::Value) -> Option<String> {
    v.as_str()
        .map(str::to_string)
        .or_else(|| v.as_u64().map(|n| n.to_string()))
}

#[test]
fn composer_declares_defaults_and_forwards_them_to_both_loops() {
    let r = recipe("auto-drive-to-merge");
    for (key, default) in BUDGET_KEYS.iter().zip(["12", "20", "14400"]) {
        assert_eq!(
            yaml_str(&r["context"][*key]).as_deref(),
            Some(default),
            "{key}"
        );
    }
    for step in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let s = r["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|s| s["id"].as_str() == Some(step))
            .unwrap_or_else(|| panic!("{step}"));
        for key in BUDGET_KEYS {
            assert_eq!(
                s["context"][key].as_str(),
                Some(format!("{{{{{key}}}}}").as_str()),
                "{step} forwards {key}"
            );
        }
    }
}

#[test]
fn loop_recipes_declare_empty_keys_and_pass_them_from_the_environment() {
    for name in ["autodrive-crusty-loop", "autodrive-merge-loop"] {
        let r = recipe(name);
        let mut driver = None;
        for s in r["steps"].as_sequence().unwrap() {
            let cmd = s["command"].as_str().unwrap_or("");
            assert!(
                !cmd.contains("{{autodrive_budget"),
                "{name}: a placeholder in a command"
            );
            if cmd.contains("--loop-name") {
                driver = Some(cmd.to_string());
            }
        }
        let cmd = driver.unwrap_or_else(|| panic!("{name}: no driver step"));
        for key in BUDGET_KEYS {
            assert_eq!(
                r["context"][key].as_str(),
                Some(""),
                "{name}: {key} default"
            );
            assert!(
                cmd.contains(&format!("RECIPE_VAR_{key}")),
                "{name}: reads {key}"
            );
            assert!(cmd.contains(&format!("{key}=")), "{name}: passes {key}");
        }
    }
}

#[test]
fn rereview_forwards_only_the_budget_keys_and_maps_a_nested_stop() {
    let text = read("amplifier-bundle/tools/autodrive_crusty_rereview.sh");
    for key in BUDGET_KEYS {
        assert!(text.contains(key), "forwards {key}");
    }
    assert!(text.contains("crusty-rereview-budget-exhausted"));
    assert!(text.contains("BUDGET_EXHAUSTED"));
}

#[test]
fn new_helpers_stay_small_and_never_merge_or_bypass() {
    for name in [
        "autodrive_progress.sh",
        "autodrive_budget.sh",
        "autodrive_loop.sh",
        "autodrive_escalate.sh",
    ] {
        let text = read(&format!("amplifier-bundle/tools/{name}"));
        assert!(
            text.lines().count() <= 400,
            "{name} exceeds the brick budget"
        );
    }
    for name in ["autodrive_progress.sh", "autodrive_budget.sh"] {
        let text = read(&format!("amplifier-bundle/tools/{name}"));
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            for bad in [
                "pr merge",
                "--admin",
                "--no-verify",
                "gh api",
                "eval ",
                "source ",
            ] {
                assert!(!line.contains(bad), "{name}: `{bad}` in: {line}");
            }
        }
    }
    let driver = read("amplifier-bundle/tools/autodrive_loop.sh");
    assert!(
        !driver.contains("not a wall-clock budget"),
        "D11: header wording"
    );
    assert!(driver.contains("no iteration cap"));
}

// Report reduction is covered by orch::autodrive's typed outcome tests,
// including no-progress, budget, usage-limit, dependency, and orphan stops.
