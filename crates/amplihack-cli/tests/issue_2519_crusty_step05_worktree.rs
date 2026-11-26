//! Issue #2519: step-05 of `autodrive-crusty-round.yaml`, through
//! `autodrive_crusty_concerns_evidence.sh`, measures the head and the commit
//! count in the PR's linked worktree, the folder step-04 commits in, and never
//! in the folder the step happened to start in.
//!
//! The step-05 body is extracted from the live YAML and run in bash as
//! `cd <start> && <body>`, with `AMPLIHACK_HOME` at this checkout so the body
//! runs this checkout's script. The runner's `working_dir` is deliberately not
//! simulated: the script's own `cd` into `repo_path` must do the work. The real
//! `autodrive_worktree_guard.sh commit-dir` check runs against a real linked
//! worktree (`git worktree add ... -b quick/demo`), with the fake `gh`
//! answering `headRefName` `quick/demo` for PR 7.
//!
//! HOME is a temporary folder, PATH holds the fakes ahead of the system
//! directories, and nothing reaches the network. Every test skips, with a
//! note, when `jq` is not installed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    Fakes, LightRepo, fake_bin, git, jq_available, run_bash, show, stderr, stdout, step_command,
    tools_dir,
};

const ROUND: &str = "autodrive-crusty-round";
const STEP05: &str = "step-05-verify-concerns-addressed";
const PR: &str = "7";

fn skip() -> bool {
    if !jq_available() {
        eprintln!("skip: `jq` not available on PATH");
        return true;
    }
    false
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|e| panic!("canonicalize {}: {e}", p.display()))
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A clone (`root`, the main checkout on `main`) with a linked worktree
/// `worktrees/pr-7` on `quick/demo`, the PR head branch the fake `gh` names.
struct Fixture {
    repo: LightRepo,
    root: PathBuf,
    worktree: PathBuf,
    fakes: Fakes,
    home: PathBuf,
    /// The reviewed head: the worktree's HEAD when the round started.
    h0: String,
}

impl Fixture {
    fn new() -> Self {
        let repo = LightRepo::new();
        let worktree_path = repo.tmp.path().join("worktrees").join("pr-7");
        git(
            &repo.wt,
            &[
                "worktree",
                "add",
                "-q",
                worktree_path.to_str().unwrap(),
                "-b",
                "quick/demo",
            ],
        );
        let root = canon(&repo.wt);
        let worktree = canon(&worktree_path);
        let h0 = git(&worktree, &["rev-parse", "HEAD"]);
        let fakes = fake_bin(repo.tmp.path());
        let home = repo.home.clone();
        Fixture {
            repo,
            root,
            worktree,
            fakes,
            home,
            h0,
        }
    }

    /// step-04's commit: a change committed in the PR worktree.
    fn commit_in_worktree(&self) -> String {
        std::fs::write(self.worktree.join("docs/fix.md"), "# fix\n").unwrap();
        git(&self.worktree, &["add", "-A"]);
        git(&self.worktree, &["commit", "-q", "-m", "address concerns"]);
        git(&self.worktree, &["rev-parse", "HEAD"])
    }

    fn context(&self, head_sha: &str) -> String {
        json!({ "pr": PR, "head_sha": head_sha }).to_string()
    }

    /// Runs `cd <start> && <step-05 body>` with `repo_path`, `pr_number`,
    /// the round context and `REPO_PATH` as given.
    fn run(&self, start: &Path, repo_path: &str, head_sha: &str, repo_path_env: &str) -> Output {
        self.run_with(start, repo_path, head_sha, repo_path_env, &[])
    }

    /// As `run`, with `extra` added to the inherited environment.
    fn run_with(
        &self,
        start: &Path,
        repo_path: &str,
        head_sha: &str,
        repo_path_env: &str,
        extra: &[(&str, &OsStr)],
    ) -> Output {
        let body = step_command(ROUND, STEP05);
        let script = format!("cd {} && {}", shell_quote(&s(start)), body);
        let tools = tools_dir();
        let ctx = self.context(head_sha);
        let path = std::env::join_paths(
            std::iter::once(self.fakes.bin.clone()).chain(
                ["/usr/local/bin", "/usr/bin", "/bin"]
                    .iter()
                    .map(PathBuf::from),
            ),
        )
        .unwrap();
        let amplihack_home = tools
            .parent()
            .and_then(Path::parent)
            .expect("tools dir sits two levels below the checkout root")
            .to_path_buf();
        let mut env: Vec<(&str, &OsStr)> = vec![
            ("PATH", path.as_os_str()),
            ("AMPLIHACK_HOME", amplihack_home.as_os_str()),
            ("RECIPE_VAR_repo_path", OsStr::new(repo_path)),
            ("RECIPE_VAR_pr_number", OsStr::new(PR)),
            ("RECIPE_VAR_crusty_round_context", OsStr::new(&ctx)),
            ("REPO_PATH", OsStr::new(repo_path_env)),
        ];
        env.extend_from_slice(extra);
        run_bash(&self.home, &self.fakes, Some(&script), &[], &env, "")
    }
}

fn shell_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', r"'\''"))
}

fn evidence(o: &Output) -> Value {
    let out = stdout(o);
    let line = out
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("step-05 printed no JSON\n{}", show(o)));
    serde_json::from_str(line).unwrap_or_else(|e| panic!("bad JSON {line:?}: {e}\n{}", show(o)))
}

/// The values of the `<key>=<reason>` marker lines step-05 writes to stderr.
/// The markers are the script's stable interface; the prose beside them is
/// not asserted, so rewording a diagnostic breaks no test.
fn markers(text: &str, key: &str) -> Vec<String> {
    let prefix = format!("{key}=");
    text.lines()
        .filter_map(|l| l.strip_prefix(&prefix))
        .map(str::to_owned)
        .collect()
}

fn warnings(o: &Output) -> Vec<String> {
    markers(&stderr(o), "step05_warning")
}

/// A refusal: exit 1, exactly one `step05_refusal=<reason>` marker naming
/// `reason`, and nothing on stdout for `parse_json` to read as `commits: 0`.
/// Returns stderr so a test can check the paths it reports.
fn assert_stopped(o: &Output, reason: &str) -> String {
    assert_eq!(
        o.status.code(),
        Some(1),
        "step-05 must stop, not report\n{}",
        show(o)
    );
    let err = stderr(o);
    assert_eq!(
        markers(&err, "step05_refusal"),
        vec![reason.to_owned()],
        "{}",
        show(o)
    );
    assert!(
        stdout(o).trim().is_empty(),
        "a stopped step-05 writes nothing to stdout\n{}",
        show(o)
    );
    err
}

// --- Case 1: step-04 committed in the PR worktree; start is the clone root ---

#[test]
fn step05_reports_the_commit_step04_made_in_the_pr_worktree() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let h1 = f.commit_in_worktree();
    assert_ne!(h1, f.h0);
    // Started in the clone root, with REPO_PATH also naming the clone root:
    // the old body measured there and reported 0 commits.
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    assert!(o.status.success(), "{}", show(&o));
    let v = evidence(&o);
    assert_eq!(v["head_sha"], json!(h1), "{}", show(&o));
    assert_eq!(v["base_sha"], json!(f.h0), "{}", show(&o));
    assert!(
        v["commits"].as_u64().unwrap_or(0) >= 1,
        "commits must count step-04's commit\n{}",
        show(&o)
    );
    assert_eq!(v["git_observed"], json!("true"), "{}", show(&o));
    assert_eq!(v["dirty_worktree"], json!("false"), "{}", show(&o));
    assert_eq!(v["head_descends_from_base"], json!("true"), "{}", show(&o));
    assert!(
        warnings(&o).is_empty(),
        "a round with a commit must not warn\n{}",
        show(&o)
    );
}

#[test]
fn step05_keeps_the_zero_commit_warning_for_a_measured_zero() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    assert!(o.status.success(), "{}", show(&o));
    let v = evidence(&o);
    assert_eq!(v["commits"], json!(0), "{}", show(&o));
    assert_eq!(v["head_sha"], json!(f.h0), "{}", show(&o));
    assert_eq!(v["git_observed"], json!("true"), "{}", show(&o));
    assert_eq!(v["head_descends_from_base"], json!("true"), "{}", show(&o));
    assert_eq!(warnings(&o), vec!["no-commit"], "{}", show(&o));
}

/// A head rewound behind the reviewed head counts 0 commits; the JSON says
/// HEAD does not descend from the base, so the evaluator need not read stderr.
#[test]
fn step05_reports_a_head_that_does_not_descend_from_the_base() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let h1 = f.commit_in_worktree();
    git(&f.worktree, &["reset", "-q", "--hard", "HEAD~1"]);
    let rewound = git(&f.worktree, &["rev-parse", "HEAD"]);
    let o = f.run(&f.root, &s(&f.worktree), &h1, &s(&f.root));
    assert!(o.status.success(), "{}", show(&o));
    let v = evidence(&o);
    assert_eq!(v["base_sha"], json!(h1), "{}", show(&o));
    assert_eq!(v["head_sha"], json!(rewound.trim()), "{}", show(&o));
    assert_eq!(v["commits"], json!(0), "{}", show(&o));
    assert_eq!(v["head_descends_from_base"], json!("false"), "{}", show(&o));
    let mut w = warnings(&o);
    w.sort();
    assert_eq!(w, vec!["head-not-descendant", "no-commit"], "{}", show(&o));
}

#[test]
fn step05_reports_a_dirty_pr_worktree() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    f.commit_in_worktree();
    std::fs::write(f.worktree.join("README.md"), "# changed, not committed\n").unwrap();
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(
        evidence(&o)["dirty_worktree"],
        json!("true"),
        "{}",
        show(&o)
    );
}

/// An inherited `GIT_COMMON_DIR` naming another repository (as under a git
/// hook) must not move step-05's own git calls away from the worktree the
/// guard checked: step-05 clears it through `git-repo-env.sh`.
#[test]
fn step05_measures_the_pr_worktree_despite_an_inherited_git_common_dir() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let h1 = f.commit_in_worktree();
    let other = f.repo.tmp.path().join("other-repo");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    std::fs::write(other.join("x.txt"), "other\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "other"]);
    let other_git = canon(&other.join(".git"));
    let o = f.run_with(
        &f.root,
        &s(&f.worktree),
        &f.h0,
        &s(&f.root),
        &[("GIT_COMMON_DIR", other_git.as_os_str())],
    );
    assert!(o.status.success(), "{}", show(&o));
    let v = evidence(&o);
    assert_eq!(v["head_sha"], json!(h1), "{}", show(&o));
    assert_eq!(v["base_sha"], json!(f.h0), "{}", show(&o));
    assert_eq!(v["commits"], json!(1), "{}", show(&o));
}

// --- Case 2: repo_path is not the PR worktree; step-05 stops ---

#[test]
fn step05_stops_when_repo_path_is_the_main_checkout() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    f.commit_in_worktree();
    // Started in the PR worktree and REPO_PATH names it too: neither may be
    // used in place of repo_path.
    let o = f.run(&f.worktree, &s(&f.root), &f.h0, &s(&f.worktree));
    let line = assert_stopped(&o, "not-pr-worktree");
    assert!(line.contains(&s(&f.root)), "STOP gives repo_path: {line}");
    assert!(
        line.contains(&s(&f.worktree)),
        "STOP gives the start folder: {line}"
    );
}

#[test]
fn step05_stops_when_the_worktree_is_on_another_branch() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    git(&f.worktree, &["checkout", "-q", "-b", "other/branch"]);
    let o = f.run(&f.root, &s(&f.worktree), &f.h0, &s(&f.root));
    let line = assert_stopped(&o, "not-pr-worktree");
    assert!(line.contains(&s(&f.worktree)), "{line}");
    assert!(line.contains(&s(&f.root)), "{line}");
}

#[test]
fn step05_stops_when_repo_path_does_not_exist() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let absent = canon(f.repo.tmp.path()).join("absent");
    let o = f.run(&f.root, &s(&absent), &f.h0, &s(&f.worktree));
    let line = assert_stopped(&o, "repo-path-unreachable");
    assert!(line.contains(&s(&absent)), "STOP names repo_path: {line}");
    assert!(
        line.contains(&s(&f.root)),
        "STOP names the start folder: {line}"
    );
}

#[test]
fn step05_stops_when_repo_path_is_empty() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    f.commit_in_worktree();
    // Started in the PR worktree itself: an empty repo_path must not fall
    // back to it (`cd ""` would succeed and stay put).
    let o = f.run(&f.worktree, "", &f.h0, &s(&f.worktree));
    assert_stopped(&o, "no-repo-path");
}

// --- head_sha from the round context is data, not a git expression ---

#[test]
fn step05_stops_on_a_head_sha_that_is_not_hex() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    // `HEAD` and `HEAD~1` resolve in git, but are not commit ids.
    for given in ["HEAD", "HEAD~1", ""] {
        let o = f.run(&f.root, &s(&f.worktree), given, &s(&f.root));
        assert_stopped(&o, "no-usable-base");
    }
}

#[test]
fn step05_resolves_a_short_or_upper_case_head_sha_to_the_full_commit() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let h1 = f.commit_in_worktree();
    let short = f.h0[..12].to_string();
    let upper = f.h0.to_uppercase();
    for given in [short.as_str(), upper.as_str()] {
        let o = f.run(&f.root, &s(&f.worktree), given, &s(&f.root));
        assert!(o.status.success(), "{}", show(&o));
        let v = evidence(&o);
        assert_eq!(v["base_sha"], json!(f.h0), "{}", show(&o));
        assert_eq!(v["head_sha"], json!(h1), "{}", show(&o));
        assert_eq!(v["commits"], json!(1), "{}", show(&o));
    }
}

#[test]
fn step05_masks_control_characters_in_a_rejected_head_sha() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let o = f.run(&f.root, &s(&f.worktree), "abc\u{1b}[31mdef", &s(&f.root));
    assert_stopped(&o, "no-usable-base");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!err.contains('\u{1b}'), "{}", show(&o));
}

#[test]
fn step05_stops_on_a_hex_head_sha_that_is_not_a_commit_here() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let o = f.run(&f.root, &s(&f.worktree), missing, &s(&f.root));
    assert_stopped(&o, "base-not-a-commit");
}

// --- the base is the loop's measurement, not the agent-written head_sha ---

/// The loop measures `reviewed_head_sha` with `git rev-parse` before the
/// round. When it reaches step-05, it is the base, and the round context's
/// agent-written `head_sha` is not read at all, so a bad value there cannot
/// stop the step.
#[test]
fn step05_measures_from_the_loop_measured_reviewed_head_sha() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    let h1 = f.commit_in_worktree();
    for given in ["HEAD~1", "", "not-a-commit"] {
        let o = f.run_with(
            &f.root,
            &s(&f.worktree),
            given,
            &s(&f.root),
            &[("RECIPE_VAR_reviewed_head_sha", OsStr::new(&f.h0))],
        );
        assert!(
            o.status.success(),
            "context head_sha {given:?}: {}",
            show(&o)
        );
        let v = evidence(&o);
        assert_eq!(v["base_sha"], json!(f.h0), "{}", show(&o));
        assert_eq!(v["head_sha"], json!(h1), "{}", show(&o));
        assert_eq!(v["commits"], json!(1), "{}", show(&o));
    }
}

/// A `reviewed_head_sha` that is not a full lowercase commit id is not a
/// measurement, so the step falls back to the round context's `head_sha`.
#[test]
fn step05_falls_back_to_the_context_head_sha_without_a_measurement() {
    if skip() {
        return;
    }
    let f = Fixture::new();
    f.commit_in_worktree();
    let upper = f.h0.to_uppercase();
    for measured in ["HEAD", upper.as_str(), &f.h0[..12]] {
        let o = f.run_with(
            &f.root,
            &s(&f.worktree),
            &f.h0,
            &s(&f.root),
            &[("RECIPE_VAR_reviewed_head_sha", OsStr::new(measured))],
        );
        assert!(o.status.success(), "measured {measured:?}: {}", show(&o));
        assert_eq!(evidence(&o)["base_sha"], json!(f.h0), "{}", show(&o));
    }
    // With neither a measurement nor a usable context head, it stops.
    let o = f.run_with(
        &f.root,
        &s(&f.worktree),
        "HEAD~1",
        &s(&f.root),
        &[("RECIPE_VAR_reviewed_head_sha", OsStr::new(""))],
    );
    assert_stopped(&o, "no-usable-base");
}

// --- a STOP reaches the next turn's agent; it does not end the run ---

/// Plays the Crusty round recipe for `autodrive_pr_goal.sh act review`: it
/// runs this checkout's real step-05 script with the `repo_path`,
/// `pr_number` and `reviewed_head_sha` the goal tool passed, and returns its
/// exit status as the round's, as the recipe runner does for a failed step.
const ROUND_STUB: &str = r#"#!/usr/bin/env bash
set -u
[ "${1:-} ${2:-}" = "recipe run" ] || { echo "unexpected amplihack call: $*" >&2; exit 3; }
rp=""; pr=""; head=""; prev=""
for a in "$@"; do
  if [ "$prev" = "-c" ]; then
    case "$a" in
      repo_path=*) rp="${a#repo_path=}" ;;
      pr_number=*) pr="${a#pr_number=}" ;;
      reviewed_head_sha=*) head="${a#reviewed_head_sha=}" ;;
    esac
  fi
  prev=$a
done
echo "step-05-verify-concerns-addressed: running"
RECIPE_VAR_repo_path="$rp" RECIPE_VAR_pr_number="$pr" RECIPE_VAR_reviewed_head_sha="$head" \
  RECIPE_VAR_crusty_round_context='{}' bash "$STEP05_SCRIPT"
"#;

/// #2519 with #3129: step-05 refuses to measure in a checkout that is not the
/// PR's linked worktree. The refusal fails the round, the goal tool records
/// the turn as `review-failed` and exits 0, so the loop goes on, and the
/// refusal marker is in the turn's review.log for the next turn's agent
/// to read as `previous`.
#[test]
fn step05_stop_reaches_the_next_turn_as_a_failed_review() {
    use crate::support::step_recovery::{private_dir, repo_root};
    if skip() {
        return;
    }
    let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
    let root = canon(tmp.path());
    let p = |n: &str| root.join(n);
    for d in [p("bin"), p("repo"), p("home")] {
        std::fs::create_dir_all(d).unwrap();
    }
    private_dir(&p("state"));
    private_dir(&p("state").join("turns"));
    let td = p("state").join("turns").join("001");
    private_dir(&td);
    std::fs::write(p("task"), "issue 2519 fixture\n").unwrap();
    // A plain checkout, not a linked worktree: the guard refuses it.
    git(&p("repo"), &["init", "-q"]);
    git(&p("repo"), &["commit", "-q", "--allow-empty", "-m", "init"]);
    crate::support::exec_stub::write_executable(&p("bin").join("amplihack"), ROUND_STUB, 0o755);
    crate::support::exec_stub::write_executable(
        &p("bin").join("gh"),
        "#!/bin/sh\necho \"gh is not reachable in this test\" >&2\nexit 1\n",
        0o755,
    );
    let path = format!("{}:/usr/local/bin:/usr/bin:/bin", p("bin").display());
    let o = crate::isolated_amplihack::isolated("bash")
        .arg(repo_root().join("amplifier-bundle/tools/autodrive_pr_goal.sh"))
        .args(["act", "review"])
        .arg(&td)
        .env("PATH", path)
        .env("HOME", p("home"))
        .env("TMPDIR", p("home"))
        .env("AUTODRIVE_GOAL_CONTRACT", "1")
        .env("AUTODRIVE_STATE_DIR", p("state"))
        .env("AUTODRIVE_REPO", p("repo"))
        .env("AUTODRIVE_WORKTREE", p("repo"))
        .env("AUTODRIVE_TASK_FILE", p("task"))
        .env("RECIPE_VAR_pr_number", PR)
        .env("AMPLIHACK_BIN", p("bin").join("amplihack"))
        .env(
            "STEP05_SCRIPT",
            tools_dir().join("autodrive_crusty_concerns_evidence.sh"),
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(p("repo"))
        .output()
        .expect("spawn autodrive_pr_goal.sh");
    assert_eq!(o.status.code(), Some(0), "the loop goes on\n{}", show(&o));
    let result: Value = serde_json::from_str(
        &std::fs::read_to_string(td.join("result.json"))
            .unwrap_or_else(|e| panic!("no result.json ({e})\n{}", show(&o))),
    )
    .unwrap();
    assert_eq!(result["outcome"], json!("review-failed"), "{result}");
    assert!(
        result.to_string().contains("review.log"),
        "the result points the next turn at the log: {result}"
    );
    let log = std::fs::read_to_string(td.join("review.log")).unwrap();
    assert_eq!(
        markers(&log, "step05_refusal"),
        vec!["not-pr-worktree"],
        "the refusal is in the turn's review.log\n{log}"
    );
}
