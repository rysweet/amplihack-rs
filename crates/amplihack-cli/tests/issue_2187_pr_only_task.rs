//! Issue #2187 — an auto-drive run given only `pr_number` takes its task from
//! the PR (and the same-repo issue the PR body links).
//!
//! Contract under test:
//! * `autodrive_task_from_pr <pr> <out-file> [repo-dir]` in
//!   `amplifier-bundle/tools/autodrive_task_from_pr.sh` writes
//!   `PR #N: <title>\n\n<body>` and, when the body links an issue with
//!   `Closes|Fixes|Resolves|Refs #M` (any case, whitespace, then `#M`), appends
//!   `\n\nLinked issue #M: <title>\n\n<body>`. It refuses fork PRs
//!   (`isCrossRepository` must be `false`), follows an issue only when its
//!   `author_association` is OWNER, MEMBER or COLLABORATOR, fails on any
//!   unreadable PR or issue, and never leaves a partial or stray file.
//! * Step-02 of `autodrive-crusty-loop.yaml` and `autodrive-merge-loop.yaml`
//!   derive the task only when `task_description` is empty or absent AND the
//!   operator's PR number (`autodrive_pr_input` when set, even empty; else
//!   `pr_number`) equals the preflight PR as a string. Otherwise today's
//!   behaviour holds: absent key exits 1, empty key reaches the loop, which
//!   refuses with exit 2. An operator task always wins and makes no gh call.
//!
//! A stub `gh` first on `PATH` logs each call (one line per call) and its
//! working directory, and answers from fixture files. No sleep, no timeout,
//! no network: every assertion waits only on exit statuses and files. Each
//! recipe case runs on both loop recipes. They skip, with a note, when `jq`
//! is not installed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RECIPES: [(&str, &str, &str); 2] = [
    (
        "autodrive-crusty-loop.yaml",
        "step-02-crusty-loop",
        "RECIPE_VAR_crusty_loop_preflight",
    ),
    (
        "autodrive-merge-loop.yaml",
        "step-02-merge-ready-loop",
        "RECIPE_VAR_merge_loop_preflight",
    ),
];

// Unique markers: none of them may ever reach argv, stdout or stderr.
const PR_TITLE: &str = "PRTITLE-zq7 add the widget";
const PR_BODY: &str =
    "PRBODY-zq7 first line $(touch pwned) `touch pwned2` %s -n\nsecond line\n\nFixes #42 for good";
const PR_BODY_NO_LINK: &str = "PRBODY-zq7 nothing linked here; see owner/repo#42 and GH-42";
const ISSUE_TITLE: &str = "ISSTITLE-zq7 widget is missing";
const ISSUE_BODY: &str = "ISSBODY-zq7 the widget must exist\n\n- [ ] done";
const MARKERS: [&str; 4] = ["PRTITLE-zq7", "PRBODY-zq7", "ISSTITLE-zq7", "ISSBODY-zq7"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn tools_src() -> PathBuf {
    repo_root().join("amplifier-bundle/tools")
}

fn jq_available() -> bool {
    let ok = Command::new("jq")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("skip: `jq` not available on PATH; the #2187 helper needs it");
    }
    ok
}

fn step_command(recipe: &str, step_id: &str) -> String {
    let path = repo_root().join("amplifier-bundle/recipes").join(recipe);
    let text = std::fs::read_to_string(&path).unwrap();
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    doc["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(step_id))
        .and_then(|s| s["command"].as_str())
        .unwrap_or_else(|| panic!("{recipe}: step {step_id} has no command"))
        .to_string()
}

fn executable(path: &Path, text: &str) {
    crate::support::exec_stub::write_executable(path, text, 0o755);
}

const AMPLIHACK_STUB: &str = r#"#!/usr/bin/env bash
case "${3:-}" in
  extract-json) cat ;;
  extract-field) jq -r --arg f "$5" '.[$f] // empty' ;;
esac
"#;

/// Stand-in loop: records its argv (NUL separated) and copies `--task-file`.
const LOOP_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\0' "$@" > "$LOOP_OUT/argv"
while [ $# -gt 0 ]; do
  if [ "$1" = "--task-file" ]; then cp -- "$2" "$LOOP_OUT/task"; fi
  shift
done
exit 0
"#;

const RESOLVER_STUB: &str = "#!/usr/bin/env bash\nprintf '%s\\n' \"$PR_WORKTREE_OUT\"\n";
const GUARD_STUB: &str =
    "#!/usr/bin/env bash\n[ \"$1\" = review-dir ] && printf '%s\\n' \"$REPO_PATH\"\n";

/// Fake `gh`: one log line per call (`$*`), its cwd to `GH_CWD`, answers from
/// fixture files. Any call it does not know fails, so an unexpected call is
/// visible as a failed run.
const GH_STUB: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "$GH_LOG"
pwd -P >> "$GH_CWD"
case "${1:-} ${2:-}" in
  "pr view")
    [ -z "${GH_PR_FAIL:-}" ] || { echo "gh: could not resolve PR" >&2; exit 1; }
    cat -- "$GH_PR_JSON"; exit 0 ;;
  "issue view")
    [ -z "${GH_ISSUE_FAIL:-}" ] || { echo "gh: could not resolve issue" >&2; exit 1; }
    cat -- "$GH_ISSUE_JSON"; exit 0 ;;
esac
if [ "${1:-}" = api ]; then
  [ -z "${GH_API_FAIL:-}" ] || { echo "gh: HTTP 404" >&2; exit 1; }
  printf '%s\n' "${GH_ISSUE_ASSOC:-MEMBER}"; exit 0
fi
echo "fake gh: unexpected call: $*" >&2
exit 1
"#;

fn pr_json(title: &str, body: &str) -> String {
    serde_json::json!({ "title": title, "body": body, "isCrossRepository": false }).to_string()
}

fn issue_json(title: &str, body: &str) -> String {
    serde_json::json!({ "title": title, "body": body }).to_string()
}

fn expected_with_issue() -> String {
    format!("PR #1570: {PR_TITLE}\n\n{PR_BODY}\n\nLinked issue #42: {ISSUE_TITLE}\n\n{ISSUE_BODY}")
}

fn expected_pr_only(body: &str) -> String {
    format!("PR #1570: {PR_TITLE}\n\n{body}")
}

/// One `gh` fixture set plus the directories it lives in.
struct Gh {
    bin: PathBuf,
    log: PathBuf,
    cwd: PathBuf,
    pr_json: PathBuf,
    issue_json: PathBuf,
    env: Vec<(String, String)>,
}

impl Gh {
    fn new(root: &Path, pr: &str, issue: &str) -> Gh {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        executable(&bin.join("gh"), GH_STUB);
        executable(&bin.join("amplihack"), AMPLIHACK_STUB);
        let pr_json = root.join("gh-pr.json");
        let issue_json = root.join("gh-issue.json");
        std::fs::write(&pr_json, pr).unwrap();
        std::fs::write(&issue_json, issue).unwrap();
        Gh {
            bin,
            log: root.join("gh-log"),
            cwd: root.join("gh-cwd"),
            pr_json,
            issue_json,
            env: Vec::new(),
        }
    }

    fn set(mut self, k: &str, v: &str) -> Gh {
        self.env.push((k.into(), v.into()));
        self
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn cwds(&self) -> Vec<String> {
        std::fs::read_to_string(&self.cwd)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn apply(&self, cmd: &mut Command) {
        cmd.env(
            "PATH",
            format!(
                "{}:{}",
                self.bin.display(),
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
            ),
        )
        .env("GH_LOG", &self.log)
        .env("GH_CWD", &self.cwd)
        .env("GH_PR_JSON", &self.pr_json)
        .env("GH_ISSUE_JSON", &self.issue_json);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
    }
}

fn called(calls: &[String], prefix: &str) -> bool {
    calls.iter().any(|c| c.starts_with(prefix))
}

fn assert_no_markers(what: &str, text: &str) {
    for m in MARKERS {
        assert!(
            !text.contains(m),
            "{what} must never contain PR or issue text ({m}); got:\n{text}"
        );
    }
}

// --------------------------------------------------------------------------
// Recipe step-02 harness
// --------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Loop {
    /// Stand-in that records argv and copies the task file.
    Stub,
    /// The real `autodrive_loop.sh`; it refuses an empty task (exit 2)
    /// before any round, so no round stub is needed.
    Real,
}

struct StepRun {
    out: Output,
    task_seen: Option<Vec<u8>>,
    loop_argv: Option<Vec<String>>,
    repo: PathBuf,
    gh: Gh,
    tmp: tempfile::TempDir,
}

impl StepRun {
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    fn task(&self) -> Option<String> {
        self.task_seen
            .as_ref()
            .map(|t| String::from_utf8_lossy(t).into_owned())
    }
}

struct StepOpts<'a> {
    /// JSON written to `AMPLIHACK_CONTEXT_FILE`.
    context: serde_json::Value,
    /// `RECIPE_VAR_*` entries (name without the prefix).
    vars: &'a [(&'a str, &'a str)],
    loop_kind: Loop,
    pr: String,
    issue: String,
    gh_env: &'a [(&'a str, &'a str)],
}

impl Default for StepOpts<'_> {
    fn default() -> Self {
        StepOpts {
            context: serde_json::json!({ "task_description": "" }),
            vars: &[("pr_number", "1570")],
            loop_kind: Loop::Stub,
            pr: pr_json(PR_TITLE, PR_BODY),
            issue: issue_json(ISSUE_TITLE, ISSUE_BODY),
            gh_env: &[],
        }
    }
}

fn run_step(recipe: &str, step_id: &str, preflight_var: &str, o: &StepOpts) -> StepRun {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let tools = root.join("home/amplifier-bundle/tools");
    let recipes = root.join("home/amplifier-bundle/recipes");
    let loop_out = root.join("loop-out");
    let repo = root.join("repo");
    let private_tmp = root.join("tmp");
    let pr_worktree = root.join("pr-worktree");
    for d in [
        &tools,
        &recipes,
        &loop_out,
        &repo,
        &private_tmp,
        &pr_worktree,
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    let mut gh = Gh::new(&root, &o.pr, &o.issue);
    for (k, v) in o.gh_env {
        gh = gh.set(k, v);
    }
    match o.loop_kind {
        Loop::Stub => executable(&tools.join("autodrive_loop.sh"), LOOP_STUB),
        Loop::Real => {
            for f in ["autodrive_loop.sh", "autodrive_escalate.sh"] {
                crate::support::exec_stub::copy_executable(&tools_src().join(f), &tools.join(f));
            }
        }
    }
    executable(&tools.join("autodrive_pr_worktree.sh"), RESOLVER_STUB);
    executable(&tools.join("autodrive_worktree_guard.sh"), GUARD_STUB);
    executable(
        &tools.join("autodrive_crusty_rereview.sh"),
        "#!/usr/bin/env bash\nexit 0\n",
    );
    // The real context reader, the real #2187 helper, and the state helper
    // (with what it sources) that the real loop loads.
    for f in [
        "recipe_context_value.sh",
        "autodrive_task_from_pr.sh",
        "autodrive_state.sh",
        "autodrive_unmerged.sh",
    ] {
        crate::support::exec_stub::copy_executable(&tools_src().join(f), &tools.join(f));
    }
    for r in ["autodrive-crusty-round", "autodrive-merge-round"] {
        std::fs::write(
            recipes.join(format!("{r}.yaml")),
            format!("name: \"{r}\"\n"),
        )
        .unwrap();
    }
    let ctx = root.join("context.json");
    std::fs::write(&ctx, o.context.to_string()).unwrap();
    let preflight = serde_json::json!({
        "state_dir": root.join("state"),
        "pr": "1570",
        "repo": repo,
    })
    .to_string();

    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(step_command(recipe, step_id)).env_clear();
    gh.apply(&mut cmd);
    cmd.env("HOME", &root)
        .env("TMPDIR", &private_tmp)
        .env("AMPLIHACK_HOME", root.join("home"))
        .env("REPO_PATH", &repo)
        .env("AMPLIHACK_CONTEXT_FILE", &ctx)
        .env(preflight_var, preflight)
        .env("LOOP_OUT", &loop_out)
        .env("PR_WORKTREE_OUT", &pr_worktree)
        .current_dir(&repo);
    for (k, v) in o.vars {
        cmd.env(format!("RECIPE_VAR_{k}"), v);
    }
    let out = cmd.output().expect("spawn bash");
    let loop_argv = std::fs::read(loop_out.join("argv")).ok().map(|b| {
        b.split(|c| *c == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect()
    });
    StepRun {
        task_seen: std::fs::read(loop_out.join("task")).ok(),
        loop_argv,
        repo,
        gh,
        out,
        tmp,
    }
}

fn each_recipe(o: &StepOpts, check: impl Fn(&str, &StepRun)) {
    for (recipe, step, preflight) in RECIPES {
        let run = run_step(recipe, step, preflight, o);
        check(recipe, &run);
    }
}

fn info_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains("task taken from PR"))
        .collect()
}

// ---- (a) PR only, with a linked issue -------------------------------------

#[test]
fn a_pr_only_with_linked_issue_takes_pr_and_issue_text() {
    if !jq_available() {
        return;
    }
    each_recipe(&StepOpts::default(), |recipe, run| {
        assert!(
            run.out.status.success(),
            "{recipe}: step must succeed; status {:?}; stderr:\n{}",
            run.out.status,
            run.stderr()
        );
        let task = run
            .task()
            .unwrap_or_else(|| panic!("{recipe}: the loop was never reached"));
        assert_eq!(
            task.trim_end_matches('\n'),
            expected_with_issue(),
            "{recipe}: task file must hold PR title/body then the linked issue"
        );
        let stderr = run.stderr();
        let info = info_lines(&stderr);
        assert_eq!(
            info.len(),
            1,
            "{recipe}: exactly one INFO line; got:\n{stderr}"
        );
        assert!(
            info[0].starts_with("INFO: auto-drive:")
                && info[0].contains("PR #1570")
                && info[0].contains("issue #42"),
            "{recipe}: INFO line must name PR #1570 and issue #42: {}",
            info[0]
        );
        assert_no_markers(&format!("{recipe} stderr"), &stderr);
        assert_no_markers(&format!("{recipe} stdout"), &run.stdout());
        // The untrusted body is never evaluated.
        assert!(
            !run.repo.join("pwned").exists(),
            "{recipe}: body was executed"
        );
        assert!(!run.tmp.path().join("pwned").exists());
        // gh runs inside the preflight repo, never with -R.
        let calls = run.gh.calls();
        assert!(called(&calls, "pr view 1570 "), "{recipe}: calls {calls:?}");
        assert!(
            called(&calls, "issue view 42 "),
            "{recipe}: calls {calls:?}"
        );
        assert!(
            calls
                .iter()
                .all(|c| !c.split(' ').any(|a| a == "-R" || a == "--repo")),
            "{recipe}: gh must never get -R/--repo: {calls:?}"
        );
        let repo = run.repo.to_string_lossy().into_owned();
        assert!(
            run.gh.cwds().iter().all(|c| *c == repo),
            "{recipe}: gh must run in the preflight repo {repo}; cwds {:?}",
            run.gh.cwds()
        );
    });
}

// ---- (a″) task_description key absent behaves like empty -----------------

#[test]
fn a2_absent_task_key_with_pr_number_derives_the_same_task() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        context: serde_json::json!({ "pr_url": "x" }),
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert!(run.out.status.success(), "{recipe}: {}", run.stderr());
        assert_eq!(
            run.task().as_deref().map(|t| t.trim_end_matches('\n')),
            Some(expected_with_issue().as_str()),
            "{recipe}"
        );
    });
}

// ---- (a′) PR only, no linked issue -----------------------------------------

#[test]
fn a1_pr_without_link_takes_pr_text_only_and_follows_nothing() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        pr: pr_json(PR_TITLE, PR_BODY_NO_LINK),
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert!(run.out.status.success(), "{recipe}: {}", run.stderr());
        assert_eq!(
            run.task().as_deref().map(|t| t.trim_end_matches('\n')),
            Some(expected_pr_only(PR_BODY_NO_LINK).as_str()),
            "{recipe}: owner/repo#N and GH-N are not links"
        );
        let calls = run.gh.calls();
        assert!(!called(&calls, "issue view"), "{recipe}: {calls:?}");
        assert!(!called(&calls, "api"), "{recipe}: {calls:?}");
        let stderr = run.stderr();
        let info = info_lines(&stderr);
        assert_eq!(info.len(), 1, "{recipe}: {stderr}");
        assert!(!info[0].contains("issue #"), "{recipe}: {}", info[0]);
    });
}

// ---- (b) gh pr view fails --------------------------------------------------

#[test]
fn b_unreadable_pr_fails_the_step_and_names_the_pr() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        gh_env: &[("GH_PR_FAIL", "1")],
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert_eq!(run.out.status.code(), Some(1), "{recipe}: must exit 1");
        assert!(run.task_seen.is_none(), "{recipe}: no round may run");
        let stderr = run.stderr();
        assert!(
            stderr.contains("ERROR: auto-drive:") && stderr.contains("1570"),
            "{recipe}: error must name PR 1570; got:\n{stderr}"
        );
        assert!(
            stderr.contains("pass task_description"),
            "{recipe}: {stderr}"
        );
    });
}

// ---- (b′) linked issue unreadable ----------------------------------------

#[test]
fn b1_unreadable_linked_issue_fails_without_falling_back_to_pr_text() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        gh_env: &[("GH_ISSUE_FAIL", "1")],
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert!(!run.out.status.success(), "{recipe}: must fail");
        assert!(run.task_seen.is_none(), "{recipe}: no round may run");
        let stderr = run.stderr();
        assert!(
            stderr.contains("#42"),
            "{recipe}: must name issue #42:\n{stderr}"
        );
        assert_no_markers(&format!("{recipe} stderr"), &stderr);
    });
}

// ---- (b″) empty PR title and body -----------------------------------------

#[test]
fn b2_pr_with_empty_title_and_body_fails() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        pr: pr_json("", ""),
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert!(!run.out.status.success(), "{recipe}: must fail");
        assert!(run.task_seen.is_none(), "{recipe}: no round may run");
    });
}

// ---- (b‴-fork) cross-repository PRs are refused ---------------------------

#[test]
fn b3_fork_pr_or_unknown_origin_is_refused() {
    if !jq_available() {
        return;
    }
    let fork = serde_json::json!({ "title": PR_TITLE, "body": PR_BODY, "isCrossRepository": true })
        .to_string();
    let missing = serde_json::json!({ "title": PR_TITLE, "body": PR_BODY }).to_string();
    let null = serde_json::json!({ "title": PR_TITLE, "body": PR_BODY, "isCrossRepository": null })
        .to_string();
    for pr in [fork, missing, null] {
        let o = StepOpts {
            pr: pr.clone(),
            ..Default::default()
        };
        each_recipe(&o, |recipe, run| {
            assert!(!run.out.status.success(), "{recipe}: {pr} must be refused");
            assert!(run.task_seen.is_none(), "{recipe}: no round may run");
            assert_no_markers(&format!("{recipe} stderr"), &run.stderr());
            assert!(!called(&run.gh.calls(), "issue view"), "{recipe}");
        });
    }
}

// ---- (b‴-assoc) linked issue by an outsider, or unknown author -----------

#[test]
fn b4_linked_issue_by_non_member_or_unknown_author_is_refused() {
    if !jq_available() {
        return;
    }
    for env in [
        &[("GH_ISSUE_ASSOC", "NONE")][..],
        &[("GH_ISSUE_ASSOC", "CONTRIBUTOR")][..],
        &[("GH_API_FAIL", "1")][..],
    ] {
        let o = StepOpts {
            gh_env: env,
            ..Default::default()
        };
        each_recipe(&o, |recipe, run| {
            assert!(!run.out.status.success(), "{recipe} {env:?}: must fail");
            assert!(run.task_seen.is_none(), "{recipe}: no round may run");
            let calls = run.gh.calls();
            assert!(
                called(&calls, "api "),
                "{recipe}: author check missing: {calls:?}"
            );
            assert!(
                !called(&calls, "issue view"),
                "{recipe}: issue text must not be read before the author check: {calls:?}"
            );
            assert!(run.stderr().contains("#42"), "{recipe}: {}", run.stderr());
            assert_no_markers(&format!("{recipe} stderr"), &run.stderr());
        });
    }
}

#[test]
fn b5_owner_and_collaborator_issues_are_followed() {
    if !jq_available() {
        return;
    }
    for assoc in ["OWNER", "MEMBER", "COLLABORATOR"] {
        let env = [("GH_ISSUE_ASSOC", assoc)];
        let o = StepOpts {
            gh_env: &env,
            ..Default::default()
        };
        each_recipe(&o, |recipe, run| {
            assert!(
                run.out.status.success(),
                "{recipe} {assoc}: {}",
                run.stderr()
            );
            assert!(
                run.task().unwrap_or_default().contains(ISSUE_BODY),
                "{recipe}"
            );
        });
    }
}

// ---- (c) no operator PR: today's refusals, no derivation ------------------

#[test]
fn c_empty_task_without_operator_pr_is_refused_with_exit_2() {
    if !jq_available() {
        return;
    }
    // The preflight still has PR 1570 (guessed from the launch checkout).
    let o = StepOpts {
        vars: &[],
        loop_kind: Loop::Real,
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert_eq!(run.out.status.code(), Some(2), "{recipe}: {}", run.stderr());
        assert!(
            run.stderr().contains("missing or empty"),
            "{recipe}: {}",
            run.stderr()
        );
        let calls = run.gh.calls();
        assert!(
            !calls
                .iter()
                .any(|c| c.starts_with("pr view") && c.contains("title")),
            "{recipe}: a guessed PR must never supply a task: {calls:?}"
        );
        assert!(!called(&calls, "issue view"), "{recipe}: {calls:?}");
    });
}

#[test]
fn c1_absent_task_without_operator_pr_still_exits_1_unavailable() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        context: serde_json::json!({ "pr_url": "x" }),
        vars: &[],
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert_eq!(run.out.status.code(), Some(1), "{recipe}: {}", run.stderr());
        assert!(
            run.stderr().contains("task_description unavailable"),
            "{recipe}"
        );
        assert!(run.task_seen.is_none(), "{recipe}");
        assert!(run.gh.calls().is_empty(), "{recipe}: {:?}", run.gh.calls());
    });
}

// ---- (d) PR text never reaches argv ---------------------------------------

#[test]
fn d_derived_text_reaches_the_loop_only_through_the_task_file() {
    if !jq_available() {
        return;
    }
    each_recipe(&StepOpts::default(), |recipe, run| {
        assert!(run.out.status.success(), "{recipe}: {}", run.stderr());
        // Not vacuous: the task really was derived and handed over.
        assert!(
            run.task().unwrap_or_default().contains(PR_TITLE),
            "{recipe}: the derived task must reach the loop"
        );
        let argv = run.loop_argv.clone().expect("loop argv recorded");
        for a in &argv {
            assert_no_markers(&format!("{recipe} loop argv element"), a);
        }
        assert!(
            argv.iter().any(|a| a == "--task-file"),
            "{recipe}: {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.starts_with("task_description=")),
            "{recipe}: the task must not be passed as --context: {argv:?}"
        );
        for c in run.gh.calls() {
            assert_no_markers(&format!("{recipe} gh argv"), &c);
        }
    });
}

// ---- (e) an operator task wins ---------------------------------------------

#[test]
fn e_operator_task_wins_byte_for_byte_and_calls_no_gh() {
    if !jq_available() {
        return;
    }
    let task = "  operator task: do exactly this\n$(not run) \u{20ac}\n";
    for vars in [&[("pr_number", "1570")][..], &[][..]] {
        let o = StepOpts {
            context: serde_json::json!({ "task_description": task }),
            vars,
            ..Default::default()
        };
        each_recipe(&o, |recipe, run| {
            assert!(run.out.status.success(), "{recipe}: {}", run.stderr());
            assert_eq!(
                run.task_seen.as_deref(),
                Some(task.as_bytes()),
                "{recipe}: operator task must be used unchanged"
            );
            assert!(run.gh.calls().is_empty(), "{recipe}: {:?}", run.gh.calls());
            assert!(info_lines(&run.stderr()).is_empty(), "{recipe}");
        });
    }
}

#[test]
fn e1_whitespace_only_task_counts_as_provided() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        context: serde_json::json!({ "task_description": " \n" }),
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert!(run.out.status.success(), "{recipe}: {}", run.stderr());
        assert_eq!(run.task_seen.as_deref(), Some(&b" \n"[..]), "{recipe}");
        assert!(run.gh.calls().is_empty(), "{recipe}");
    });
}

// ---- (f) operator PR must equal the preflight PR --------------------------

#[test]
fn f_operator_pr_that_differs_from_preflight_never_derives() {
    if !jq_available() {
        return;
    }
    for vars in [
        // autodrive_pr_input outranks pr_number.
        &[("autodrive_pr_input", "99"), ("pr_number", "1570")][..],
        // Set but empty still outranks pr_number (composer run with only
        // existing_branch).
        &[("autodrive_pr_input", ""), ("pr_number", "1570")][..],
        // Strings are compared, never normalised.
        &[("pr_number", "01570")][..],
        &[("pr_number", "1570 ")][..],
    ] {
        let o = StepOpts {
            vars,
            loop_kind: Loop::Real,
            ..Default::default()
        };
        each_recipe(&o, |recipe, run| {
            assert_eq!(
                run.out.status.code(),
                Some(2),
                "{recipe} {vars:?}: {}",
                run.stderr()
            );
            assert!(
                run.stderr().contains("missing or empty"),
                "{recipe} {vars:?}"
            );
            assert!(!called(&run.gh.calls(), "pr view"), "{recipe} {vars:?}");
        });
    }
}

#[test]
fn f1_autodrive_pr_input_equal_to_preflight_derives() {
    if !jq_available() {
        return;
    }
    let o = StepOpts {
        vars: &[("autodrive_pr_input", "1570")],
        ..Default::default()
    };
    each_recipe(&o, |recipe, run| {
        assert!(run.out.status.success(), "{recipe}: {}", run.stderr());
        assert_eq!(
            run.task().as_deref().map(|t| t.trim_end_matches('\n')),
            Some(expected_with_issue().as_str()),
            "{recipe}"
        );
    });
}

// --------------------------------------------------------------------------
// Unit tests: autodrive_task_from_pr called directly
// --------------------------------------------------------------------------

struct HelperRun {
    status: std::process::ExitStatus,
    stderr: String,
    stdout: String,
    out_file: PathBuf,
    out_dir: PathBuf,
    private_tmp: PathBuf,
    repo: PathBuf,
    gh: Gh,
    _tmp: tempfile::TempDir,
}

impl HelperRun {
    fn task(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.out_file).ok()
    }
}

fn run_helper(
    pr_arg: &str,
    pr: &str,
    issue: &str,
    gh_env: &[(&str, &str)],
    existing_out: Option<&str>,
) -> HelperRun {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let out_dir = root.join("out");
    let repo = root.join("repo");
    let private_tmp = root.join("tmp");
    for d in [&out_dir, &repo, &private_tmp] {
        std::fs::create_dir_all(d).unwrap();
    }
    let out_file = out_dir.join("task.txt");
    if let Some(e) = existing_out {
        std::fs::write(&out_file, e).unwrap();
    }
    let mut gh = Gh::new(&root, pr, issue);
    for (k, v) in gh_env {
        gh = gh.set(k, v);
    }
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(". \"$1\" || exit 97; autodrive_task_from_pr \"$2\" \"$3\" \"$4\"")
        .arg("bash")
        .arg(tools_src().join("autodrive_task_from_pr.sh"))
        .arg(pr_arg)
        .arg(&out_file)
        .arg(&repo)
        .env_clear();
    gh.apply(&mut cmd);
    cmd.env("HOME", &root)
        .env("TMPDIR", &private_tmp)
        .current_dir(&root);
    let out = cmd.output().expect("spawn bash");
    HelperRun {
        status: out.status,
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        out_file,
        out_dir,
        private_tmp,
        repo,
        gh,
        _tmp: tmp,
    }
}

fn helper(pr_body: &str) -> HelperRun {
    run_helper(
        "1570",
        &pr_json(PR_TITLE, pr_body),
        &issue_json(ISSUE_TITLE, ISSUE_BODY),
        &[],
        None,
    )
}

fn dir_names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn unit_helper_is_defined_in_its_own_tool() {
    let text = std::fs::read_to_string(tools_src().join("autodrive_task_from_pr.sh")).unwrap();
    assert!(
        text.contains("autodrive_task_from_pr()"),
        "autodrive_task_from_pr.sh must define autodrive_task_from_pr"
    );
}

#[test]
fn unit_success_writes_exact_text_private_and_leaves_no_temp_files() {
    if !jq_available() {
        return;
    }
    let r = helper(PR_BODY);
    assert!(r.status.success(), "stderr:\n{}", r.stderr);
    let task = String::from_utf8(r.task().expect("output written")).unwrap();
    assert_eq!(task.trim_end_matches('\n'), expected_with_issue());
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&r.out_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "task file must be 0600, got {mode:o}");
    assert_eq!(dir_names(&r.out_dir), vec!["task.txt".to_string()]);
    assert!(
        dir_names(&r.private_tmp).is_empty(),
        "TMPDIR left: {:?}",
        dir_names(&r.private_tmp)
    );
    assert_eq!(
        r.stderr.lines().filter(|l| l.starts_with("INFO:")).count(),
        1,
        "{}",
        r.stderr
    );
    assert_no_markers("helper stderr", &r.stderr);
    assert_no_markers("helper stdout", &r.stdout);
    let repo = r.repo.to_string_lossy().into_owned();
    assert!(r.gh.cwds().iter().all(|c| *c == repo), "{:?}", r.gh.cwds());
}

#[test]
fn unit_bad_pr_numbers_are_rejected_before_any_gh_call() {
    if !jq_available() {
        return;
    }
    for bad in [
        "",
        "12a",
        "-1",
        "--repo",
        "12345678901",
        "1570 ",
        " 1570",
        "1e3",
    ] {
        let r = run_helper(
            bad,
            &pr_json(PR_TITLE, PR_BODY),
            &issue_json(ISSUE_TITLE, ISSUE_BODY),
            &[],
            None,
        );
        assert!(!r.status.success(), "{bad:?} must be rejected");
        assert!(r.gh.calls().is_empty(), "{bad:?}: {:?}", r.gh.calls());
        assert!(r.task().is_none(), "{bad:?}: no output");
    }
}

#[test]
fn unit_link_words_followed_case_insensitively() {
    if !jq_available() {
        return;
    }
    for body in [
        "Closes #42",
        "closes #42",
        "FIXES #42",
        "Resolves #42",
        "rEsOlVeS #42",
        "Refs #42",
        "refs  #42",
        "text before\nfixes\t#42 after",
        "(Fixes #42)",
    ] {
        let r = helper(body);
        assert!(r.status.success(), "{body:?}: {}", r.stderr);
        assert!(
            called(&r.gh.calls(), "issue view 42"),
            "{body:?}: {:?}",
            r.gh.calls()
        );
    }
}

#[test]
fn unit_non_links_are_not_followed() {
    if !jq_available() {
        return;
    }
    for body in [
        "see #42",
        "Fix #42",
        "Closes: #42",
        "Fixes#42",
        "Fixed #42",
        "Closed #42",
        "Resolved #42",
        "Ref #42",
        "Fixes owner/repo#42",
        "Fixes https://github.com/o/r/issues/42",
        "Fixes GH-42",
        "Fixes #--repo",
        "prefixes #42",
        "Fixes #12345678901",
    ] {
        let r = helper(body);
        assert!(r.status.success(), "{body:?}: {}", r.stderr);
        let calls = r.gh.calls();
        assert!(!called(&calls, "issue view"), "{body:?}: {calls:?}");
        assert!(!called(&calls, "api"), "{body:?}: {calls:?}");
        let task = String::from_utf8(r.task().unwrap()).unwrap();
        assert_eq!(
            task.trim_end_matches('\n'),
            expected_pr_only(body),
            "{body:?}"
        );
    }
}

#[test]
fn unit_first_link_wins() {
    if !jq_available() {
        return;
    }
    let r = helper("Refs #42\nCloses #43");
    assert!(r.status.success(), "{}", r.stderr);
    let calls = r.gh.calls();
    assert!(called(&calls, "issue view 42"), "{calls:?}");
    assert!(!called(&calls, "issue view 43"), "{calls:?}");
}

#[test]
fn unit_nul_bytes_are_removed_and_never_break_link_search() {
    if !jq_available() {
        return;
    }
    let r = helper("abc\u{0000}def");
    assert!(r.status.success(), "{}", r.stderr);
    let task = r.task().unwrap();
    assert!(!task.contains(&0u8), "NUL bytes must be removed");
    assert!(String::from_utf8_lossy(&task).contains("abcdef"));
    assert!(!called(&r.gh.calls(), "issue view"));
    // A NUL before the link must not hide it or turn grep binary.
    let r = helper("x\u{0000}y\nFixes #42");
    assert!(r.status.success(), "{}", r.stderr);
    assert!(called(&r.gh.calls(), "issue view 42"), "{:?}", r.gh.calls());
}

#[test]
fn unit_null_body_is_allowed_when_title_is_present() {
    if !jq_available() {
        return;
    }
    let pr = serde_json::json!({ "title": PR_TITLE, "body": null, "isCrossRepository": false })
        .to_string();
    let r = run_helper("1570", &pr, &issue_json(ISSUE_TITLE, ISSUE_BODY), &[], None);
    assert!(r.status.success(), "{}", r.stderr);
    let task = String::from_utf8(r.task().unwrap()).unwrap();
    assert!(task.starts_with(&format!("PR #1570: {PR_TITLE}")), "{task}");
    assert!(
        !task.contains("null"),
        "a null body must not print as 'null': {task}"
    );
}

#[test]
fn unit_wrong_json_types_are_refused() {
    if !jq_available() {
        return;
    }
    for pr in [
        r#"{"title":{},"body":"b","isCrossRepository":false}"#,
        r#"{"title":"t","body":["b"],"isCrossRepository":false}"#,
        r#"{"title":"t","body":"b","isCrossRepository":"false"}"#,
        "not json",
        "",
    ] {
        let r = run_helper("1570", pr, &issue_json(ISSUE_TITLE, ISSUE_BODY), &[], None);
        assert!(!r.status.success(), "{pr:?} must be refused");
        assert!(r.task().is_none(), "{pr:?}");
    }
    let r = run_helper(
        "1570",
        &pr_json(PR_TITLE, PR_BODY),
        r#"{"title":{},"body":"b"}"#,
        &[],
        None,
    );
    assert!(
        !r.status.success(),
        "an issue with an object title must be refused"
    );
}

#[test]
fn unit_empty_linked_issue_fails() {
    if !jq_available() {
        return;
    }
    let r = run_helper(
        "1570",
        &pr_json(PR_TITLE, PR_BODY),
        &issue_json("", ""),
        &[],
        None,
    );
    assert!(!r.status.success());
    assert!(r.stderr.contains("#42"), "{}", r.stderr);
}

#[test]
fn unit_failure_leaves_existing_output_unchanged_and_no_temp_files() {
    if !jq_available() {
        return;
    }
    let cases: [&[(&str, &str)]; 4] = [
        &[("GH_PR_FAIL", "1")],
        &[("GH_ISSUE_FAIL", "1")],
        &[("GH_API_FAIL", "1")],
        &[("GH_ISSUE_ASSOC", "NONE")],
    ];
    for env in cases {
        let r = run_helper(
            "1570",
            &pr_json(PR_TITLE, PR_BODY),
            &issue_json(ISSUE_TITLE, ISSUE_BODY),
            env,
            Some("previous content\n"),
        );
        assert!(!r.status.success(), "{env:?} must fail");
        assert_eq!(
            std::fs::read_to_string(&r.out_file).unwrap(),
            "previous content\n",
            "{env:?}: a failure must leave the output unchanged"
        );
        assert_eq!(
            dir_names(&r.out_dir),
            vec!["task.txt".to_string()],
            "{env:?}"
        );
        assert!(
            dir_names(&r.private_tmp).is_empty(),
            "{env:?}: {:?}",
            dir_names(&r.private_tmp)
        );
        assert!(
            r.stderr.starts_with("ERROR: auto-drive:") || r.stderr.contains("\nERROR: auto-drive:"),
            "{env:?}: {}",
            r.stderr
        );
        assert_no_markers("helper stderr", &r.stderr);
    }
}

#[test]
fn unit_output_symlink_is_replaced_not_written_through() {
    if !jq_available() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let victim = tmp.path().join("victim");
    std::fs::write(&victim, "untouched").unwrap();
    // Build a run whose output path is a symlink to `victim`.
    let r = {
        let root = tmp.path().canonicalize().unwrap();
        let out_dir = root.join("out");
        let repo = root.join("repo");
        let private_tmp = root.join("tmp");
        for d in [&out_dir, &repo, &private_tmp] {
            std::fs::create_dir_all(d).unwrap();
        }
        let out_file = out_dir.join("task.txt");
        std::os::unix::fs::symlink(&victim, &out_file).unwrap();
        let gh = Gh::new(
            &root,
            &pr_json(PR_TITLE, PR_BODY),
            &issue_json(ISSUE_TITLE, ISSUE_BODY),
        );
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(". \"$1\" || exit 97; autodrive_task_from_pr \"$2\" \"$3\" \"$4\"")
            .arg("bash")
            .arg(tools_src().join("autodrive_task_from_pr.sh"))
            .arg("1570")
            .arg(&out_file)
            .arg(&repo)
            .env_clear();
        gh.apply(&mut cmd);
        cmd.env("HOME", &root).env("TMPDIR", &private_tmp);
        (cmd.output().unwrap(), out_file)
    };
    assert!(
        r.0.status.success(),
        "{}",
        String::from_utf8_lossy(&r.0.stderr)
    );
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "untouched");
    assert!(
        !std::fs::symlink_metadata(&r.1)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

// --------------------------------------------------------------------------
// Docs and recipe text
// --------------------------------------------------------------------------

#[test]
fn docs_say_task_description_is_optional_with_pr_number() {
    let a = std::fs::read_to_string(
        repo_root().join("amplifier-bundle/skills/auto-drive-to-merge/SKILL.md"),
    )
    .unwrap();
    let b = std::fs::read_to_string(
        repo_root().join("docs/claude/skills/auto-drive-to-merge/SKILL.md"),
    )
    .unwrap();
    assert_eq!(a, b, "the two SKILL.md copies must stay byte-identical");
    let reference =
        std::fs::read_to_string(repo_root().join("docs/reference/auto-drive-to-merge.md")).unwrap();
    assert!(
        reference.contains("autodrive_task_from_pr"),
        "the reference doc must describe autodrive_task_from_pr"
    );
}

#[test]
fn loop_steps_load_the_state_helper_only_on_the_derivation_path() {
    for (recipe, step, _) in RECIPES {
        let cmd = step_command(recipe, step);
        assert!(
            cmd.contains("autodrive_task_from_pr \"$PR\" \"$TASK_IN\" \"$REPO\""),
            "{recipe}: step-02 must call autodrive_task_from_pr with the preflight PR and repo"
        );
        assert!(
            cmd.contains("write_context_value task_description"),
            "{recipe}: the #1570 read stays"
        );
        let derive = cmd.find("autodrive_task_from_pr").unwrap();
        let run_loop = cmd.find("bash \"$LOOP_HELPER\"").unwrap();
        assert!(
            derive < run_loop,
            "{recipe}: derivation must precede the loop"
        );
    }
}
