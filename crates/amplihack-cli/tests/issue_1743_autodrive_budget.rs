//! Issue #1743: auto-drive stops a loop in code when a round changes nothing,
//! and each run has a usage budget.
//!
//! Contract (docs/reference/auto-drive-to-merge.md, "No-progress stop and
//! usage budget"):
//!
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
//!   `autodrive-budget-summary.md`, and both post a sanitised PR comment
//!   through `gh pr comment --body-file`.
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
      [ -n "$in" ] && v="$(printf '%s' "$in" | jq -r --arg f "$f" '.[$f] // empty | if type == "string" then . else tojson end' 2>/dev/null)"
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
  for a in "$@"; do [ "$prev" = "-c" ] && case "$a" in loop_round_label=*) label="${a#loop_round_label=}" ;; esac; prev=$a; done
  printf '%s\n' "$label" >> "$S/evaluator-calls"
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
IFS='|' read -r verdict commit sleep_s light items <<<"$line"
[ "${sleep_s:-0}" -gt 0 ] && sleep "$sleep_s"
if [ "$commit" = "1" ]; then
  t="$(git -C "$wd" write-tree)"
  c="$(git -C "$wd" -c user.name=t -c user.email=t@t commit-tree "$t" -p HEAD -m "stub $label")"
  git -C "$wd" update-ref HEAD "$c"
fi
field=crusty_verdict; [ "$name" = "autodrive-merge-round" ] && field=merge_ready_verdict
if [ "$verdict" != "-" ]; then
  printf '{"%s":"%s"}\n' "$field" "$verdict" > "$record"
  : > "$record.findings"
  if [ -n "$items" ]; then printf '%s\n' "$items" | tr ',' '\n' > "$record.findings"; fi
  [ "$light" = "1" ] && printf '{"verdict_source":"light-minor-rule"}\n' > "$record.light.json"
fi
echo "round ran"
exit 0
"#;

const GH_STUB: &str = r#"#!/usr/bin/env bash
set -u
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
        let tmp = tempfile::tempdir().unwrap();
        let p = |n: &str| tmp.path().join(n);
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

    /// One line per round label: `VERDICT|COMMIT|SLEEP|LIGHT|item,item`.
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
    fn assert_no_real_home(&self) {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let home = PathBuf::from(home);
        let forbidden = [home.join(".claude"), home.join(".amplihack")];
        for dir in [&self.log, &self.state, &self.stub] {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if !p.is_file() {
                    continue;
                }
                let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
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
    fx.plan(&["CLEAN|0|0|0|", "CONCERNS|0|0|0|"]);
    let o = fx.run("crusty", &[]);
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
    let rows = fx.state_file("crusty-progress.tsv").unwrap();
    let first: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert_eq!(first, ["round-1", fx.head().as_str(), "1", ""], "{rows}");
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
fn rejected_light_rounds_on_one_head_with_empty_findings_do_not_stop() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CONCERNS|0|0|1|"]);
    fx.set("eval_stuck_after", "3");
    let o = fx.run("crusty", &[]);
    let r = result(&o);
    assert!(
        !r["reason"].as_str().unwrap().contains(DECIDED),
        "{}",
        show(&o)
    );
    assert_eq!(fx.evaluator_calls().len(), 4);
}

#[test]
fn the_round_after_a_light_round_is_exempt_too() {
    let Some(fx) = Fixture::new() else { return };
    // Round 1 is a rejected light round; round 2 is a normal non-clean round
    // with no findings on the same head; round 3 repeats round 2.
    fx.plan(&["CONCERNS|0|0|1|", "CONCERNS|0|0|0|"]);
    let o = fx.run("crusty", &[]);
    let r = result(&o);
    assert_eq!(r["round_label"], "round-3", "{}", show(&o));
    assert!(r["reason"].as_str().unwrap().contains(DECIDED));
    assert_eq!(fx.evaluator_calls(), ["round-1", "round-2"]);
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

#[test]
fn a_clean_round_is_not_stopped_after_it_ran() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CLEAN|0|0|0|"]);
    fx.set("eval", "done");
    let done = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "1")]);
    assert_eq!(done.status.code(), Some(0), "{}", show(&done));
    assert_eq!(result(&done)["loop_result"], "DONE");
    assert_eq!(fx.evaluator_calls(), ["round-1"]);
}

#[test]
fn a_continued_clean_round_stops_at_the_next_pre_round_check() {
    let Some(fx) = Fixture::new() else { return };
    fx.plan(&["CLEAN|0|0|0|"]);
    let o = run_with(&fx, "crusty", &[(BUDGET_KEYS[0], "1")]);
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
    fx.plan(&["CONCERNS|0|0|0|ping @example-user,see #123,```evil"]);
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
    assert_eq!(rows, format!("round-1\t{SHA}\t0\ta,b\n"));
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
    assert_eq!(rows, format!("round-1\t{SHA}\t0\tt_u,x_y\n"));

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

// ── The run report ───────────────────────────────────────────────────────────

fn report(crusty: &str, merge_loop: &str, merge_result: &str) -> Output {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    crate::support::exec_stub::write_executable(&bin.join("amplihack"), AMPLIHACK_STUB, 0o755);
    let cmd =
        crate::support::auto_drive_light::step_command("auto-drive-to-merge", "autodrive-report");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    crate::isolated_amplihack::isolated("bash")
        .arg("-c")
        .arg(cmd)
        .env("PATH", path)
        .env("STUB_STATE", tmp.path())
        .env("STUB_LOG", tmp.path())
        .env("RECIPE_VAR_pr_number", "1743")
        .env("RECIPE_VAR_crusty_loop_result", crusty)
        .env("RECIPE_VAR_merge_loop_result", merge_loop)
        .env(
            "RECIPE_VAR_merge_gate_result",
            format!(r#"{{"pr":"1743","merge_result":"{merge_result}"}}"#),
        )
        .output()
        .unwrap()
}

fn report_json(o: &Output) -> serde_json::Value {
    stdout(o)
        .lines()
        .rev()
        .find_map(|l| serde_json::from_str(l).ok())
        .unwrap_or_else(|| panic!("no JSON\n{}", show(o)))
}

const DONE: &str = r#"{"loop":"x","loop_result":"DONE","round_label":"round-1"}"#;

#[test]
fn report_treats_a_budget_or_code_stuck_stop_as_not_merged() {
    if !jq_available() {
        return;
    }
    let budget =
        r#"{"loop":"merge-ready","loop_result":"BUDGET_EXHAUSTED","round_label":"round-12"}"#;
    let o = report(DONE, budget, "MERGED");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    let j = report_json(&o);
    assert_eq!(j["outcome"], "NOT_MERGED");
    assert_eq!(j["merge_loop"], "BUDGET_EXHAUSTED");
    assert!(
        stderr(&o).contains("the merge-ready loop stopped as BUDGET_EXHAUSTED"),
        "{}",
        show(&o)
    );

    let stuck = r#"{"loop":"crusty","loop_result":"STUCK","round_label":"round-2"}"#;
    let o = report(stuck, DONE, "DRY_RUN");
    assert_eq!(o.status.code(), Some(1), "{}", show(&o));
    assert_eq!(report_json(&o)["outcome"], "NOT_MERGED");
}

#[test]
fn report_reduces_merge_loop_to_a_plain_token() {
    if !jq_available() {
        return;
    }
    let o = report(DONE, r#"{"loop_result":"DONE\"x"}"#, "MERGED");
    assert_eq!(report_json(&o)["merge_loop"], "unknown", "{}", show(&o));
    let o = report(DONE, DONE, "MERGED");
    assert_eq!(o.status.code(), Some(0), "{}", show(&o));
    let j = report_json(&o);
    assert_eq!(j["outcome"], "MERGED");
    assert_eq!(j["merge_loop"], "DONE");
}
