//! default-workflow 4.0.0 run whole, through the real binary and runner, with
//! PR 1's harness: workflow-prep, step-04, the build loop and the report.
//!
//! The repository is named by its GitHub URL and git reaches the bare origin
//! through `insteadOf`, so nothing reaches the network. A wrapper in front of
//! the harness's gh answers what a build adds (the identity pre-flight, the
//! issue read, the claim lookup, `pr list --head`, `pr create` and the PR
//! text); any other call goes to the harness's gh, and a call neither knows is
//! logged to `unexpected.log`, which every test asserts is empty. Nothing
//! falls through to a real gh.
//!
//! The turn's agent follows the live reading: with no pull request it opens
//! one with the task's own title and body, then marks it ready, then ends
//! `ready`; a failed previous action or a reading it does not expect hands
//! off, naming it. It ends on state, never on a count of turns.
//!
//! Assertions are on the runner's step results, the stand-in's call log, the
//! remote's refs and files on disk, never on wording.

#![cfg(target_os = "linux")]

use crate::issue_3129_goal_loop::support::{Harness, PR_BRANCH, describe, git};
use crate::support::exec_stub::write_executable;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

const WRAPPER: &str = r##"#!/usr/bin/env bash
G="$GH_STATE"
jqf() { local f=""; while [ $# -gt 0 ]; do case "$1" in --jq|-q) f="$2"; shift 2 ;; *) shift ;; esac; done
  if [ -n "$f" ]; then jq -r "$f"; else cat; fi; }
log() { printf '%s\n' "$*" >> "$G/calls.log"; }
case "${1:-} ${2:-}" in
  "api graphql")
    log "$@"
    echo '{"data":{"viewer":{"login":"alice"},"repository":{"nameWithOwner":"o/r","viewerPermission":"WRITE"}}}' ;;
  "api repos/{owner}/{repo}/issues/463"|"api repos/o/r/issues/463")
    log "$@"; shift 2
    echo '{"state":"open","html_url":"https://github.com/o/r/issues/463"}' | jqf "$@" ;;
  "issue view")
    log "$@"
    echo '{"state":"OPEN","url":"https://github.com/o/r/issues/463","number":463}' | jqf "$@" ;;
  "pr list")
    log "$@"
    case " $* " in
      *" --head "*)
        if [ -e "$G/created" ]; then
          jq -cn --arg b "$GH_PR_BRANCH" '[{number:7,url:"https://github.com/o/r/pull/7",state:"OPEN",isDraft:true,headRefName:$b,
            headRefOid:"",baseRefName:"main",isCrossRepository:false,title:"t",body:"b",createdAt:"2026-11-23T00:00:00Z"}]'
        else echo '[]'; fi ;;
      *" --repo "*" --state open "*) cat "$G/claims.json" 2>/dev/null || echo '[]' ;;
      *) echo '[]' ;;
    esac ;;
  "pr create")
    log "$@"
    prev=""
    for a in "$@"; do
      case "$a" in --title=*) printf '%s' "${a#--title=}" > "$G/live-title" ;; esac
      case "$prev" in --title) printf '%s' "$a" > "$G/live-title" ;; --body-file) cp -- "$a" "$G/create-body"; cp -- "$a" "$G/live-body" ;; esac
      prev="$a"
    done
    : > "$G/created"; echo "https://github.com/o/r/pull/7" ;;
  "pr edit")
    log "$@"
    prev=""
    for a in "$@"; do
      case "$a" in --title=*) printf '%s' "${a#--title=}" > "$G/live-title" ;; esac
      case "$prev" in --body-file) cp -- "$a" "$G/live-body" ;; esac
      prev="$a"
    done ;;
  "pr view")
    case " $* " in
      *" title,body "*) log "$@"
        jq -cn --rawfile t "$G/live-title" --rawfile b "$G/live-body" '{title:$t, body:$b}' ;;
      *) exec "$G/../bin/gh-harness" "$@" ;;
    esac ;;
  *)
    err="$(mktemp "$G/err.XXXXXX")"
    "$G/../bin/gh-harness" "$@" 2>"$err"; rc=$?
    grep -q 'stand-in gh: unexpected' "$err" && printf '%s\n' "$*" >> "$G/unexpected.log"
    cat "$err" >&2; rm -f -- "$err"
    exit "$rc" ;;
esac
"##;

/// The turn's agent, from the live reading.
const POLICY: &str = r#"
r() { jq -r "$1" "$TURN/reading.json"; }
own="$(r '.goal.prs.own // empty')"
prev="$(r '.previous.outcome // "none"')"
handoff() {
  jq -cn --arg d "$1" '{action:"handoff",args:{owner:"the operator who started this run",decision:$d,
    text:("The build could not go on: " + $d + ".")},outcomes:[],why:("I could not take the next step: " + $d + ".")}'
}
case "$prev" in
  none|continued|opened|already-open|text-differs|marked-ready|already-ready) ;;
  *) handoff "the previous action ended ${prev}"; return 0 ;;
esac
if [ -z "$own" ]; then
  jq -cn --rawfile t "$STUB_LOG/title.txt" --rawfile b "$STUB_LOG/body.txt" \
    '{action:"open-pr",args:{title:$t,body:$b},outcomes:[],why:"The branch is pushed and has no pull request, so I open it with the task text."}'
elif [ "$prev" = marked-ready ] || [ "$prev" = already-ready ]; then
  echo '{"action":"ready","args":{"text":"The change is pushed and its pull request is open and ready for review."},"outcomes":[],"why":"The pull request is ready, so the build is done."}'
else
  echo '{"action":"mark-ready","args":{},"outcomes":[],"why":"The pull request is open with the task text, so I mark it ready."}'
fi
"#;

const WAIT_POLICY: &str = r#"
case "$(jq -r '.previous.outcome // "none"' "$TURN/reading.json")" in
  marked-ready|already-ready)
    echo '{"action":"wait","args":{},"outcomes":[],"why":"Wait for the checks."}'
    return 0 ;;
  checks-complete)
    echo '{"action":"ready","args":{"text":"The checks finished."},"outcomes":[],"why":"The checks finished."}'
    return 0 ;;
esac
"#;

const TITLE: &str = "Build the widget";
const BODY: &str = "Builds the widget.\n\nFixes #463\n";

struct Run {
    h: Harness,
    out: Output,
    result: Value,
}

impl Run {
    fn step(&self, id: &str) -> Vec<&Value> {
        self.result["step_results"]
            .as_array()
            .unwrap_or_else(|| panic!("no step_results: {}", describe(&self.out)))
            .iter()
            .filter(|s| s["step_id"] == id)
            .collect()
    }

    fn status(&self, id: &str) -> String {
        let s = self.step(id);
        assert_eq!(s.len(), 1, "step {id} once: {}", self.result);
        s[0]["status"].as_str().unwrap_or_default().to_lowercase()
    }

    fn output(&self, id: &str) -> String {
        self.step(id)
            .first()
            .and_then(|s| s["output"].as_str())
            .unwrap_or_default()
            .trim()
            .to_owned()
    }

    fn position(&self, id: &str) -> usize {
        self.result["step_results"]
            .as_array()
            .unwrap()
            .iter()
            .position(|s| s["step_id"] == id)
            .unwrap_or_else(|| panic!("no step {id}: {}", self.result))
    }

    fn unexpected(&self) -> String {
        fs::read_to_string(self.h.gh.join("unexpected.log")).unwrap_or_default()
    }
}

/// Every folder under `root` whose name starts with `build.`, at any depth.
fn build_state_folders(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if e.file_name().to_string_lossy().starts_with("build.") {
                    found.push(p.clone());
                }
                if !p.ends_with(".git") && !p.ends_with("origin.git") {
                    stack.push(p);
                }
            }
        }
    }
    found
}

/// Sets up the harness for a build: the GitHub-named origin, the wrapper, the
/// policy and the task's PR text. `origin_url` names the repository.
fn prepare(origin_url: &str) -> Harness {
    let h = Harness::new();
    let origin = h.origin.to_str().unwrap().to_owned();
    git(&h.repo, &["config", "remote.origin.url", origin_url]);
    git(
        &h.repo,
        &["config", &format!("url.{origin}.insteadOf"), origin_url],
    );
    // The launch checkout stands on main; the run works on the PR branch in
    // its own worktree.
    git(&h.repo, &["checkout", "-q", "main"]);
    fs::rename(h.bin.join("gh"), h.bin.join("gh-harness")).unwrap();
    write_executable(&h.bin.join("gh"), WRAPPER, 0o755);
    fs::write(&h.policy, POLICY).unwrap();
    fs::write(h.base.join("title.txt"), TITLE).unwrap();
    fs::write(h.base.join("body.txt"), BODY).unwrap();
    h
}

/// Runs default-workflow on the PR branch with the task's own PR lines.
fn workflow(h: Harness, extra: &[&str]) -> Run {
    let task = format!("task_description=Build the widget.\n\nPR title: {TITLE}\n\n{BODY}");
    let repo = format!("repo_path={}", h.repo.display());
    let branch = format!("existing_branch={PR_BRANCH}");
    let mut args: Vec<&str> = vec![
        "recipe",
        "run",
        "default-workflow",
        "-c",
        &task,
        "-c",
        &repo,
        "-c",
        &branch,
    ];
    for kv in extra {
        args.push("-c");
        args.push(kv);
    }
    let w = h.repo.to_str().unwrap().to_owned();
    args.extend_from_slice(&["--format", "json", "-w", &w, "--no-detach"]);
    // build-goal-setup makes the state folder under the run folder.
    let glob = format!("{}/work/runs/*/build-goal/build.*", h.base.display());
    let out = h
        .command(&args)
        .env("STUB_STATE_GLOB", glob)
        .output()
        .expect("amplihack recipe run");
    let result: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    Run { h, out, result }
}

fn worktree_of(run: &Run) -> PathBuf {
    let setup: Value = serde_json::from_str(&run.output("build-goal-setup"))
        .unwrap_or_else(|e| panic!("build-goal-setup output: {e}: {}", run.result));
    PathBuf::from(setup["worktree"].as_str().unwrap_or_default())
}

#[test]
fn checks_watch_interval_reaches_the_build_loop_from_startup_context() {
    let h = prepare("https://github.com/o/r.git");
    h.policy(&format!("{WAIT_POLICY}\n{POLICY}"));
    // The check watch reads one line and completes without blocking.
    let release = h.gh.join("release.fifo");
    fs::remove_file(&release).unwrap();
    fs::write(&release, "\n").unwrap();

    let run = workflow(h, &["checks_watch_interval=120"]);
    assert!(run.out.status.success(), "{}", describe(&run.out));
    assert_eq!(run.unexpected(), "", "{}", describe(&run.out));
    assert_eq!(run.status("build-loop"), "completed", "{}", run.result);
    assert_eq!(run.h.watch_calls(), ["pr checks 7 --watch --interval 120"]);
}

#[test]
fn the_worktree_is_kept_and_the_report_runs_once_after_the_loop() {
    let run = workflow(prepare("https://github.com/o/r.git"), &[]);
    assert!(run.out.status.success(), "{}", describe(&run.out));
    assert_eq!(run.unexpected(), "", "{}", describe(&run.out));
    assert_eq!(run.status("build-loop"), "completed", "{}", run.result);
    assert_eq!(run.status("build-report"), "completed", "{}", run.result);
    assert!(run.position("build-report") > run.position("build-loop"));
    assert!(
        run.h.calls().iter().any(|c| c.starts_with("pr ready 7")),
        "{:?}\n{}",
        run.h.calls(),
        describe(&run.out)
    );
    let wt = worktree_of(&run);
    assert!(
        wt.is_dir(),
        "the worktree outlives the run: {}",
        wt.display()
    );
    let report: Value = serde_json::from_str(&run.output("build-report")).unwrap();
    assert_eq!(report["pr_number"], "7", "{report}");
    assert_eq!(report["outcomes"], json!([]), "{report}");
}

/// The state folder build-goal-setup made for this run.
fn state_of(run: &Run) -> PathBuf {
    let setup: Value = serde_json::from_str(&run.output("build-goal-setup"))
        .unwrap_or_else(|e| panic!("build-goal-setup output: {e}: {}", run.result));
    PathBuf::from(setup["state_dir"].as_str().unwrap_or_default())
}

/// C5: a bookkeeping step that fails after the work is pushed and the pull
/// request is open never fails the run. The test's own bundle copy holds a
/// stand-in for the run script whose `report` fails; every other subcommand
/// is the real one.
#[test]
fn a_report_that_fails_after_the_pr_opened_leaves_the_run_successful() {
    let h = prepare("https://github.com/o/r.git");
    let tools = h.bundle_root.join("amplifier-bundle/tools");
    fs::rename(
        tools.join("autodrive_build_run.sh"),
        tools.join("autodrive_build_run_real.sh"),
    )
    .unwrap();
    write_executable(
        &tools.join("autodrive_build_run.sh"),
        "#!/usr/bin/env bash\n\
         if [ \"${1:-}\" = report ]; then echo \"ERROR: the report stand-in fails\" >&2; exit 1; fi\n\
         exec bash \"$(dirname \"$0\")/autodrive_build_run_real.sh\" \"$@\"\n",
        0o755,
    );
    let run = workflow(h, &[]);
    assert!(run.out.status.success(), "{}", describe(&run.out));
    assert_eq!(run.unexpected(), "", "{}", describe(&run.out));
    assert_eq!(run.status("build-report"), "failed", "{}", run.result);
    let calls = run.h.calls();
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("pr create")).count(),
        1,
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("pr ready 7")),
        "{calls:?}"
    );
}

/// The seven v3 phase recipes, as the Azure DevOps route runs them.
const V3: [&str; 7] = [
    "workflow-design",
    "workflow-tdd",
    "workflow-refactor-review",
    "workflow-precommit-test",
    "workflow-publish",
    "workflow-pr-review",
    "workflow-finalize",
];

/// D2: an Azure DevOps origin takes the seven v3 phase steps and never starts
/// the build loop, which reads and writes through gh only. In the test's own
/// bundle copy each v3 phase recipe is a one-step stand-in that records its
/// name, so the route is seen without running the phases.
#[test]
fn an_azure_devops_origin_takes_the_v3_route_and_never_starts_the_loop() {
    let h = prepare("https://dev.azure.com/org/proj/_git/r");
    let recipes = h.bundle_root.join("amplifier-bundle/recipes");
    for name in V3 {
        fs::write(
            recipes.join(format!("{name}.yaml")),
            format!(
                "name: \"{name}\"\ndescription: \"test stand-in\"\nversion: \"1.0.0\"\nsteps:\n  \
                 - id: \"stand-in\"\n    type: \"bash\"\n    command: |\n      \
                 printf '%s\\n' {name} >> \"$STUB_LOG/v3-steps.log\"\n    output: \"stand_in\"\n"
            ),
        )
        .unwrap();
    }
    let run = workflow(h, &[]);
    assert!(run.out.status.success(), "{}", describe(&run.out));
    for id in ["build-goal-setup", "build-loop", "build-report"] {
        assert_eq!(run.status(id), "skipped", "{id}: {}", run.result);
    }
    let ran: Vec<String> = fs::read_to_string(run.h.base.join("v3-steps.log"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(ran, V3, "{}", run.result);
    assert!(
        build_state_folders(&run.h.base).is_empty(),
        "no build state folder: {:?}",
        build_state_folders(&run.h.base)
    );
    assert!(
        !run.h.calls().iter().any(|c| c.starts_with("pr create")),
        "{:?}",
        run.h.calls()
    );
}

/// C6c: a branch that is only behind a base without strict rules gets no base
/// merge, and the run marks its pull request ready.
#[test]
fn a_branch_only_behind_a_non_strict_base_gets_no_base_merge() {
    let h = prepare("https://github.com/o/r.git");
    h.advance_base(false);
    let head = h.origin_head(PR_BRANCH);
    let run = workflow(h, &[]);
    assert!(run.out.status.success(), "{}", describe(&run.out));
    assert_eq!(run.unexpected(), "", "{}", describe(&run.out));
    assert!(
        run.h.merge_commits_on(PR_BRANCH).is_empty(),
        "no merge commit on the branch"
    );
    assert_eq!(
        run.h.origin_head(PR_BRANCH),
        head,
        "the branch is where it was"
    );
    assert!(
        run.h.calls().iter().any(|c| c.starts_with("pr ready 7")),
        "{:?}",
        run.h.calls()
    );
}

/// #3110 at run level: an open pull request on another branch that closes
/// the task's issue reaches the turn's reading as information, and the run
/// goes on to open its own.
#[test]
fn another_branchs_pr_on_the_issue_is_information_and_the_run_opens_its_own() {
    let h = prepare("https://github.com/o/r.git");
    fs::write(
        h.gh.join("claims.json"),
        json!([{"number": 480, "url": "https://github.com/o/r/pull/480",
            "title": "Runner content follow-ups", "state": "OPEN", "mergedAt": null,
            "headRefName": "feat/issues-463-464-runner-content-follow-ups", "isCrossRepository": false,
            "closingIssuesReferences": [{"number": 463}], "baseRefName": "main",
            "body": "Fixes #463"}])
        .to_string(),
    )
    .unwrap();
    let run = workflow(h, &[]);
    assert!(run.out.status.success(), "{}", describe(&run.out));
    assert_eq!(run.unexpected(), "", "{}", describe(&run.out));
    let reading: Value = serde_json::from_str(
        &fs::read_to_string(state_of(&run).join("turns/001/reading.json")).unwrap(),
    )
    .unwrap();
    let issue = &reading["goal"]["issue"];
    assert_eq!(issue["number"], 463, "{issue}");
    let claims = issue["claims"]
        .as_array()
        .unwrap_or_else(|| panic!("{issue}"));
    assert!(
        claims
            .iter()
            .any(|c| c["number"] == 480 && c["relation"] == "other-branch"),
        "{issue}"
    );
    let calls = run.h.calls();
    assert_eq!(
        calls.iter().filter(|c| c.starts_with("pr create")).count(),
        1,
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.starts_with("pr ready 7")),
        "{calls:?}"
    );
}
