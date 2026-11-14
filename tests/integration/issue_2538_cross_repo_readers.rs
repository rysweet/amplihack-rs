//! Issue #2538: every reader of `isCrossRepository` accepts a pull request as
//! same-repository only when jq finds `.isCrossRepository == false`.
//!
//! Readers covered here, each run for real with a fake `gh` first on `PATH`:
//!
//! - `workflow_pr_scope.sh`, discovery mode (the primary-key `select`);
//! - `workflow-terminal-state.yaml`, the PR metadata fields, with
//!   `WORKFLOW_PR_SCOPE_HELPER` stubbed to hand the PR JSON over verbatim so
//!   the terminal-state reader is tested on its own, not behind the scope
//!   filter;
//! - `workflow_pr_ready.sh`, the identity check before `gh pr ready`;
//! - `workflow_gh_retry.sh`, the REST fallback that builds the field from
//!   `head.repo.full_name` and `base.repo.full_name`: it may emit `false`
//!   only when both names are present and equal.
//!
//! The shared table: `true`, a missing field, `null`, the string `"false"`,
//! empty output and invalid JSON are refused; `false` is accepted.
//! `workflow_final_status.sh` and `workflow_issue_claim_check.sh` are
//! covered by `tests/issue_1596_final_status_branch_pr_lookup.sh` and
//! `tests/issue_1448_review_is_not_a_duplicate.sh`; `light-eligibility` and
//! `autodrive_run_pr.sh` by `cli_integration`'s
//! `issue_2538_cross_repo_refusal`.
//!
//! Every child runs with a temporary `HOME` and `TMPDIR`, no GitHub tokens
//! and no network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value as JsonValue, json};
use serde_yaml::Value;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p
}

fn tool(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

fn jq_available() -> bool {
    Command::new("jq")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The readers under test are jq programs, so a host without jq cannot pass.
fn require_jq() {
    assert!(
        jq_available(),
        "`jq` must be on PATH to run the #2538 reader tests"
    );
}

fn write_exec(path: &Path, body: &str) {
    crate::exec_stub::write_executable(path, body, 0o755);
}

fn show(o: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// One refused case: the label and how the PR object is changed. `Raw`
/// replaces the whole `gh` answer.
enum Case {
    Set(JsonValue),
    Remove,
    Raw(&'static str),
}

fn refused_cases() -> Vec<(&'static str, Case)> {
    vec![
        ("true", Case::Set(json!(true))),
        ("missing field", Case::Remove),
        ("null", Case::Set(JsonValue::Null)),
        ("string \"false\"", Case::Set(json!("false"))),
        ("empty output", Case::Raw("")),
        ("invalid JSON", Case::Raw("{this is not json")),
    ]
}

/// Applies `case` to `pr`; `wrap` puts an object in an array for list calls.
fn answer(pr: &JsonValue, case: &Case, wrap: bool) -> String {
    let mut obj = pr.clone();
    match case {
        Case::Set(v) => obj["isCrossRepository"] = v.clone(),
        Case::Remove => {
            obj.as_object_mut().unwrap().remove("isCrossRepository");
        }
        Case::Raw(r) => return (*r).to_string(),
    }
    if wrap {
        json!([obj]).to_string()
    } else {
        obj.to_string()
    }
}

/// A private sandbox: `bin`, `home`, `tmp` and a `gh.log`.
struct Sandbox {
    tmp: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        for d in ["bin", "home", "tmp"] {
            fs::create_dir_all(tmp.path().join(d)).unwrap();
        }
        Sandbox { tmp }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    fn bin(&self) -> PathBuf {
        self.p("bin")
    }

    fn gh_log(&self) -> String {
        fs::read_to_string(self.p("gh.log")).unwrap_or_default()
    }

    /// A command with this sandbox's HOME, TMPDIR and fakes first on PATH,
    /// and no tokens, profile root or inherited recipe variables.
    fn command(&self, program: &str, cwd: &Path) -> Command {
        let mut c = Command::new(program);
        for (key, _) in std::env::vars_os() {
            let k = key.to_string_lossy();
            if k.starts_with("RECIPE_VAR_")
                || k.starts_with("GH_")
                || k.starts_with("GITHUB_")
                || k.starts_with("WORKFLOW_")
                || k.starts_with("PR_")
            {
                c.env_remove(&key);
            }
        }
        for key in [
            "AMPLIHACK_CLAUDE_PROFILE_ROOT",
            "ISSUE_NUMBER",
            "EXPECTED_PR_TITLE_PREFIX",
            "RECIPE_STARTED_AT",
            "TASK_STARTED_AT",
            "BRANCH_NAME",
            "REPO_PATH",
            "NO_MERGE",
        ] {
            c.env_remove(key);
        }
        let path = std::env::join_paths(std::iter::once(self.bin()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        c.current_dir(cwd)
            .env("PATH", path)
            .env("HOME", self.p("home"))
            .env("TMPDIR", self.p("tmp"))
            .env("XDG_CONFIG_HOME", self.p("home/.config"))
            .env("GH_CONFIG_DIR", self.p("home/.config/gh"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GH_RETRY_TMPDIR", self.p("tmp"))
            .env("FAKE_LOG", self.p("gh.log"));
        crate::isolated_amplihack::no_git_prompt(&mut c);
        c
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let o = self
            .command("git", dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(o.status.success(), "git {args:?}\n{}", show(&o));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// A repository on `feature`, one commit ahead of `main`, with origin
    /// `https://github.com/owner/repo.git` and `origin/main`/`origin/HEAD`.
    /// Returns the repository path and HEAD.
    fn repo(&self) -> (PathBuf, String) {
        let repo = self.p("repo");
        fs::create_dir_all(&repo).unwrap();
        self.git(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("README.md"), "base\n").unwrap();
        self.git(&repo, &["add", "README.md"]);
        self.git(&repo, &["commit", "-q", "-m", "base"]);
        self.git(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repo.git",
            ],
        );
        let main = self.git(&repo, &["rev-parse", "main"]);
        self.git(&repo, &["update-ref", "refs/remotes/origin/main", &main]);
        self.git(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        self.git(&repo, &["checkout", "-q", "-b", "feature"]);
        fs::write(repo.join("feature.txt"), "feature\n").unwrap();
        self.git(&repo, &["add", "feature.txt"]);
        self.git(&repo, &["commit", "-q", "-m", "feature"]);
        let head = self.git(&repo, &["rev-parse", "HEAD"]);
        (repo, head)
    }
}

/// A same-repository PR #7 on `feature` -> `main` at `head`.
fn pr_object(head: &str, state: &str, draft: bool) -> JsonValue {
    json!({
        "number": 7,
        "title": "feat: demo",
        "body": "",
        "state": state,
        "isDraft": draft,
        "createdAt": "2026-01-01T00:00:00Z",
        "mergedAt": if state == "MERGED" { json!("2026-01-02T00:00:00Z") } else { json!("") },
        "url": "https://github.com/owner/repo/pull/7",
        "headRefName": "feature",
        "baseRefName": "main",
        "headRefOid": head,
        "headRepositoryOwner": {"login": "owner"},
        "headRepository": {"name": "repo"},
        "isCrossRepository": false,
        "statusCheckRollup": [],
        "mergeable": "MERGEABLE",
        "reviews": []
    })
}

/// A `gh` that logs argv and answers `pr list` with `$FAKE_GH_PR_LIST`,
/// `pr view` with `$FAKE_GH_PR_VIEW`, `api` with `$FAKE_GH_API`, verbatim.
const FAKE_GH: &str = r#"#!/usr/bin/env bash
printf '%s\n' "$*" >> "${FAKE_LOG:?}"
case "${1:-} ${2:-}" in
  "auth status") exit 0 ;;
  "pr list") printf '%s' "${FAKE_GH_PR_LIST-[]}" ;;
  "pr view") printf '%s' "${FAKE_GH_PR_VIEW-}" ;;
  "pr ready"|"pr comment") : ;;
  *) if [ "${1:-}" = api ]; then printf '%s' "${FAKE_GH_API-{\}}"; fi ;;
esac
exit 0
"#;

// --- (c) workflow_pr_scope.sh, discovery mode -----------------------------------

fn scope(list: &str, head: &str) -> (Output, Sandbox) {
    let sb = Sandbox::new();
    write_exec(&sb.bin().join("gh"), FAKE_GH);
    let cwd = sb.p("tmp");
    let o = sb
        .command("bash", &cwd)
        .arg(tool("workflow_pr_scope.sh"))
        .args([
            "--repo",
            "owner/repo",
            "--head",
            "feature",
            "--base",
            "main",
            "--head-sha",
            head,
        ])
        .env("FAKE_GH_PR_LIST", list)
        .output()
        .expect("run workflow_pr_scope.sh");
    (o, sb)
}

fn scoped_number(o: &Output) -> Option<i64> {
    let out = String::from_utf8_lossy(&o.stdout);
    out.lines()
        .rev()
        .find_map(|l| serde_json::from_str::<JsonValue>(l).ok())
        .and_then(|j| j["number"].as_i64())
}

#[test]
fn pr_scope_discovery_refuses_every_value_but_exact_false() {
    require_jq();
    let head = "1111111111111111111111111111111111111111";
    let pr = pr_object(head, "OPEN", true);
    for (label, case) in refused_cases() {
        let (o, _sb) = scope(&answer(&pr, &case, true), head);
        assert!(
            !(o.status.success() && scoped_number(&o) == Some(7)),
            "isCrossRepository {label}: PR 7 must not be selected as the scoped PR\n{}",
            show(&o)
        );
    }
}

#[test]
fn pr_scope_discovery_accepts_exact_false() {
    require_jq();
    let head = "1111111111111111111111111111111111111111";
    let (o, sb) = scope(&json!([pr_object(head, "OPEN", true)]).to_string(), head);
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(scoped_number(&o), Some(7), "{}", show(&o));
    assert!(sb.gh_log().contains("pr list"), "the helper must ask gh");
}

// --- (d) workflow-terminal-state.yaml, PR metadata fields ------------------------

/// A scope helper stub that prints `$STUB_PR_JSON` verbatim and succeeds, so
/// the terminal-state reader sees exactly the bytes under test.
const SCOPE_STUB: &str = "#!/usr/bin/env bash\nprintf '%s' \"${STUB_PR_JSON-}\"\nexit 0\n";

fn terminal_state_command() -> String {
    let path = workspace_root().join("amplifier-bundle/recipes/workflow-terminal-state.yaml");
    let text = fs::read_to_string(&path).unwrap();
    let recipe: Value = serde_yaml::from_str(&text).unwrap();
    recipe["steps"]
        .as_sequence()
        .expect("steps")
        .iter()
        .filter_map(|s| s.get("command").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs the terminal-state probe for PR 7 on `feature`. `pr_json` builds the
/// scope helper's answer from the repository's HEAD.
fn terminal(pr_json: impl FnOnce(&str) -> String) -> (Output, JsonValue) {
    let sb = Sandbox::new();
    write_exec(&sb.bin().join("gh"), FAKE_GH);
    let stub = sb.p("scope-stub.sh");
    write_exec(&stub, SCOPE_STUB);
    let (repo, head) = sb.repo();
    let o = sb
        .command("bash", &repo)
        .arg("-c")
        .arg(terminal_state_command())
        .env("REPO_PATH", &repo)
        .env("AMPLIHACK_HOME", workspace_root())
        .env("RECIPE_VAR_repo_path", &repo)
        .env("RECIPE_VAR_branch_name", "feature")
        .env("RECIPE_VAR_base_ref", "main")
        .env("RECIPE_VAR_pr_number", "")
        .env("RECIPE_VAR_pr_url", "https://github.com/owner/repo/pull/7")
        .env("RECIPE_VAR_issue_number", "")
        .env("RECIPE_VAR_goal_already_met", "false")
        .env("RECIPE_VAR_worktree_setup", "")
        .env("WORKFLOW_PR_SCOPE_HELPER", &stub)
        .env(
            "WORKFLOW_GITHUB_REPO_HELPER",
            tool("workflow_github_repo.sh"),
        )
        .env("STUB_PR_JSON", pr_json(&head))
        .output()
        .expect("run workflow-terminal-state");
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    let j = out
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with('{'))
        .and_then(|l| serde_json::from_str(l).ok())
        .unwrap_or(JsonValue::Null);
    (o, j)
}

/// PR 7, merged, at the repository's own HEAD, changed by `case`.
fn terminal_with_case(case: &Case) -> (Output, JsonValue) {
    terminal(|head| answer(&pr_object(head, "MERGED", false), case, false))
}

#[test]
fn terminal_state_refuses_every_value_but_exact_false() {
    require_jq();
    // A merged PR is terminal success only when it is this repository's PR.
    for (label, case) in refused_cases() {
        let (o, j) = terminal_with_case(&case);
        assert!(
            !(o.status.success() && j["terminal_success"] == "true"),
            "isCrossRepository {label}: a PR not proven same-repository is never terminal success\n{}",
            show(&o)
        );
        assert_ne!(
            j["terminal_state"],
            "MERGED",
            "isCrossRepository {label}\n{}",
            show(&o)
        );
    }
}

#[test]
fn terminal_state_accepts_exact_false() {
    require_jq();
    let (o, j) = terminal_with_case(&Case::Set(json!(false)));
    assert!(o.status.success(), "{}", show(&o));
    assert_eq!(j["terminal_success"], "true", "{}", show(&o));
    assert_eq!(j["terminal_state"], "MERGED", "{}", show(&o));
}

// --- (e) workflow_pr_ready.sh ------------------------------------------------------

/// A scope helper stub naming PR 7.
const READY_SCOPE_STUB: &str = "#!/usr/bin/env bash\nprintf '%s\\n' '{\"url\":\"https://github.com/owner/repo/pull/7\",\"number\":7}'\nexit 0\n";

fn ready(case: &Case) -> (Output, Sandbox) {
    let sb = Sandbox::new();
    write_exec(&sb.bin().join("gh"), FAKE_GH);
    let stub = sb.p("scope-stub.sh");
    write_exec(&stub, READY_SCOPE_STUB);
    let (repo, head) = sb.repo();
    let view = answer(&pr_object(&head, "OPEN", true), case, false);
    let o = sb
        .command("bash", &repo)
        .arg(tool("workflow_pr_ready.sh"))
        .env("RECIPE_VAR_pr_url", "https://github.com/owner/repo/pull/7")
        .env("WORKFLOW_PR_SCOPE_HELPER", &stub)
        .env("FAKE_GH_PR_VIEW", &view)
        .output()
        .expect("run workflow_pr_ready.sh");
    (o, sb)
}

fn ready_mutated(sb: &Sandbox) -> bool {
    sb.gh_log().lines().any(|l| l.starts_with("pr ready"))
}

#[test]
fn pr_ready_refuses_every_value_but_exact_false() {
    require_jq();
    for (label, case) in refused_cases() {
        let (o, sb) = ready(&case);
        assert!(
            !o.status.success(),
            "isCrossRepository {label}: the readiness check must fail\n{}",
            show(&o)
        );
        assert!(
            !ready_mutated(&sb),
            "isCrossRepository {label}: gh pr ready must not be called\n{}\ngh log:\n{}",
            show(&o),
            sb.gh_log()
        );
    }
}

#[test]
fn pr_ready_accepts_exact_false() {
    require_jq();
    let (o, sb) = ready(&Case::Set(json!(false)));
    assert!(o.status.success(), "{}\ngh log:\n{}", show(&o), sb.gh_log());
    assert!(
        ready_mutated(&sb),
        "a same-repository draft PR is marked ready\ngh log:\n{}",
        sb.gh_log()
    );
}

// --- REST fallback producer (workflow_gh_retry.sh) ------------------------------

fn rest(api: &JsonValue, func: &str, arg: &str) -> Output {
    let sb = Sandbox::new();
    write_exec(&sb.bin().join("gh"), FAKE_GH);
    let cwd = sb.p("tmp");
    let script = r#". "$1" && "$2" owner/repo "$3""#;
    sb.command("bash", &cwd)
        .arg("-c")
        .arg(script)
        .arg("bash")
        .arg(tool("workflow_gh_retry.sh"))
        .arg(func)
        .arg(arg)
        .env("FAKE_GH_API", api.to_string())
        .output()
        .expect("run workflow_gh_retry.sh")
}

fn rest_pr(head_repo: JsonValue, base_repo: JsonValue) -> JsonValue {
    json!({
        "number": 7,
        "state": "open",
        "html_url": "https://github.com/owner/repo/pull/7",
        "head": {"ref": "feature", "sha": "1111111111111111111111111111111111111111", "repo": head_repo},
        "base": {"ref": "main", "repo": base_repo}
    })
}

fn rest_cross_repo(pr: &JsonValue) -> (JsonValue, JsonValue) {
    let view = rest(pr, "gh_pr_view_rest", "7");
    assert!(view.status.success(), "{}", show(&view));
    let v: JsonValue = serde_json::from_slice(&view.stdout)
        .unwrap_or_else(|e| panic!("view JSON ({e})\n{}", show(&view)));
    let list = rest(&json!([pr]), "gh_pr_exists_rest", "feature");
    assert!(list.status.success(), "{}", show(&list));
    let l: JsonValue = serde_json::from_slice(&list.stdout)
        .unwrap_or_else(|e| panic!("list JSON ({e})\n{}", show(&list)));
    (
        v["isCrossRepository"].clone(),
        l[0]["isCrossRepository"].clone(),
    )
}

#[test]
fn rest_fallback_never_claims_same_repository_without_both_names() {
    require_jq();
    let owner = json!({"login": "owner"});
    let cases = [
        (
            "both full_name missing",
            rest_pr(
                json!({"name": "repo", "owner": owner}),
                json!({"name": "repo"}),
            ),
        ),
        (
            "head.repo null (deleted fork)",
            rest_pr(JsonValue::Null, json!({"full_name": "owner/repo"})),
        ),
        ("both repos null", rest_pr(JsonValue::Null, JsonValue::Null)),
        (
            "different names",
            rest_pr(
                json!({"full_name": "forker/repo", "name": "repo", "owner": {"login": "forker"}}),
                json!({"full_name": "owner/repo"}),
            ),
        ),
    ];
    for (label, pr) in cases {
        let (view, list) = rest_cross_repo(&pr);
        assert_eq!(view, json!(true), "gh_pr_view_rest, {label}");
        assert_eq!(list, json!(true), "gh_pr_exists_rest, {label}");
    }
}

#[test]
fn rest_fallback_reports_false_for_equal_names() {
    require_jq();
    let pr = rest_pr(
        json!({"full_name": "owner/repo", "name": "repo", "owner": {"login": "owner"}}),
        json!({"full_name": "owner/repo"}),
    );
    let (view, list) = rest_cross_repo(&pr);
    assert_eq!(view, json!(false));
    assert_eq!(list, json!(false));
}
