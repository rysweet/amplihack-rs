//! TDD (red) contracts for the PR-scope primary-key fix (default-workflow
//! finalize false-failure, issue #1017).
//!
//! `workflow_pr_scope.sh` currently AND-joins every scope discriminator
//! (`--issue`, `--head-sha`, `--expected-pr-title-prefix`, ...) on top of the
//! head/base identity. That converts a reliable lookup into one that fails
//! closed: the recipe's OWN unique-by-branch PR (e.g. #1015) is rejected with
//! `no_scoped_pr` whenever the PR body references the UPSTREAM issue instead of
//! the branch's LOCAL tracking issue, or whenever the remote head advanced past
//! the captured `--head-sha`. The publish step then falls through to
//! `gh pr create`, collides ("a pull request for branch already exists"), and
//! hard-fails the whole recipe.
//!
//! These tests specify the required behavior BEFORE implementation:
//!   * `(headRefName, baseRefName, same-repo, non-cross-repo)` is the PRIMARY
//!     key. A single primary candidate is adopted even when `--issue` /
//!     `--head-sha` / `--expected-pr-title-prefix` do NOT match.
//!   * discriminators act only as TIE-BREAKERS when >1 candidate shares the
//!     same head+base; if they zero out but >=2 OPEN candidates remain, the
//!     loud `multiple_scoped_prs` failure is preserved.
//!   * a PR on a DIFFERENT head branch is NEVER matched (safety invariant).
//!   * `workflow_publish_pr.sh` is collision-tolerant: when `gh pr create`
//!     fails but an OPEN PR now exists for the branch, publish adopts it as
//!     `existing-open-pr` / success (exit 0) instead of `FAILED_PR_CREATE`.
//!
//! All harnesses inject a stub `gh` on PATH (mirroring
//! `issue_815_804_local_tracking_scope_args_test.rs` and
//! `workflow_publish_resilience.rs`) so no real GitHub token is required.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // bins/amplihack -> bins
    p.pop(); // bins -> workspace root
    p
}

fn helper_path(name: &str) -> PathBuf {
    workspace_root().join("amplifier-bundle/tools").join(name)
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

fn git(repo: &Path, args: &[&str]) {
    let status = amplihack_git::command()
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

/// The PR-scope helper only needs `command -v gh` to succeed plus a JSON array
/// on stdout for `gh pr list`. This stub emits the canned candidate list from
/// `$STUB_PR_LIST_JSON` and is a no-op for anything else.
fn write_list_stub_gh(bin_dir: &Path, list_json: &Path) {
    fs::create_dir_all(bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    fs::write(
        &gh,
        format!(
            "#!/usr/bin/env bash\nset -uo pipefail\nsub=\"${{1:-}} ${{2:-}}\"\ncase \"$sub\" in\n  \"pr list\") cat {list:?} ;;\n  *) exit 0 ;;\nesac\n",
            list = list_json.display().to_string()
        ),
    )
    .expect("write gh stub");
    make_executable(&gh);
}

// Scope failures now persist evidence through the built binary. Keep that
// process and every helper away from the operator's configuration.
fn private_bash(dir: &Path, bin_dir: &Path) -> Command {
    let home = dir.join("private-home");
    let runtime = dir.join("private-runtime");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&runtime).unwrap();
    let binary_dir = Path::new(env!("CARGO_BIN_EXE_amplihack")).parent().unwrap();
    let mut cmd = Command::new("bash");
    cmd.env_clear()
        .env(
            "PATH",
            format!(
                "{}:{}:/usr/bin:/bin",
                bin_dir.display(),
                binary_dir.display()
            ),
        )
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("GH_CONFIG_DIR", home.join("gh"))
        .env("AMPLIHACK_RUNTIME_ROOT", &runtime)
        .env("TMPDIR", &runtime)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

/// Run `workflow_pr_scope.sh` with the stub `gh` on PATH. Returns (stdout,
/// success). No git repo is required: `--repo/--head/--base/--head-sha` are all
/// passed explicitly.
fn run_scope(dir: &Path, bin_dir: &Path, args: &[&str]) -> (String, bool) {
    run_scope_env(dir, bin_dir, args, &[])
}

/// `run_scope` with extra environment variables for the helper.
fn run_scope_env(
    dir: &Path,
    bin_dir: &Path,
    args: &[&str],
    envs: &[(&str, &str)],
) -> (String, bool) {
    let output = private_bash(dir, bin_dir)
        .arg(helper_path("workflow_pr_scope.sh"))
        .args(args)
        .current_dir(dir)
        .envs(envs.iter().copied())
        .output()
        .expect("run workflow_pr_scope.sh");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.success(),
    )
}

/// The stale local head-sha the recipe captured before the remote branch
/// advanced. Deliberately different from every candidate's `headRefOid`.
const STALE_HEAD_SHA: &str = "2222222222222222222222222222222222222222";

// ---------------------------------------------------------------------------
// Case (a): #1015 adoption despite stale --issue + --head-sha + title-prefix.
// ---------------------------------------------------------------------------

#[test]
fn adopts_own_unique_branch_pr_despite_stale_issue_head_sha_and_title_prefix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");

    // The #1015 shape: OPEN, unique by (head,base), title does NOT start with
    // the "Update" prefix passed below (the publish step's default until
    // #1968, which dropped it), body references only the UPSTREAM issue
    // (#4610), and headRefOid differs from the recipe's captured --head-sha.
    let list = tmp.path().join("candidates.json");
    fs::write(
        &list,
        r#"[
          {
            "number": 1015,
            "title": "Verus Phase-1 proof scaffolding",
            "body": "Closes #4610\nRelates to #4610\nUpstream tracking: #4610",
            "state": "OPEN",
            "createdAt": "2024-01-01T00:00:00Z",
            "mergedAt": null,
            "url": "https://github.com/owner/repo/pull/1015",
            "headRefName": "feat/issue-1014-verus-phase1",
            "baseRefName": "main",
            "headRefOid": "1111111111111111111111111111111111111111",
            "headRepositoryOwner": {"login": "owner"},
            "headRepository": {"name": "repo"},
            "isCrossRepository": false,
            "isDraft": true
          }
        ]"#,
    )
    .expect("write candidates");
    write_list_stub_gh(&bin_dir, &list);

    let (stdout, ok) = run_scope(
        tmp.path(),
        &bin_dir,
        &[
            "--repo",
            "owner/repo",
            "--head",
            "feat/issue-1014-verus-phase1",
            "--base",
            "main",
            "--issue",
            "1014",
            "--work-item",
            "1014",
            "--expected-pr-title-prefix",
            "Update",
            "--head-sha",
            STALE_HEAD_SHA,
        ],
    );

    assert!(
        ok,
        "scope must exit 0 when a single primary (head,base,same-repo) candidate exists; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"ok\":true") && stdout.contains("\"scoped\":true"),
        "the recipe's own unique-by-branch PR must be adopted (ok:true) even when --issue/--head-sha/--expected-pr-title-prefix do not match; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"number\":1015"),
        "the adopted PR must be #1015, not a re-derived candidate; stdout:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// Case (b): regression — body references a DIFFERENT upstream issue than the
// branch tracking issue, AND the remote head-sha differs. Still adopted.
// ---------------------------------------------------------------------------

#[test]
fn adopts_pr_whose_body_references_a_different_upstream_issue() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");

    let list = tmp.path().join("candidates.json");
    fs::write(
        &list,
        r#"[
          {
            "number": 4242,
            "title": "Update workflow recipes with 3 changed files (#4610)",
            "body": "Closes #4610\nThis addresses upstream issue #4610 only.",
            "state": "OPEN",
            "createdAt": "2024-02-02T00:00:00Z",
            "mergedAt": null,
            "url": "https://github.com/owner/repo/pull/4242",
            "headRefName": "feat/issue-1014-tracking",
            "baseRefName": "main",
            "headRefOid": "1111111111111111111111111111111111111111",
            "headRepositoryOwner": {"login": "owner"},
            "headRepository": {"name": "repo"},
            "isCrossRepository": false,
            "isDraft": true
          }
        ]"#,
    )
    .expect("write candidates");
    write_list_stub_gh(&bin_dir, &list);

    let (stdout, ok) = run_scope(
        tmp.path(),
        &bin_dir,
        &[
            "--repo",
            "owner/repo",
            "--head",
            "feat/issue-1014-tracking",
            "--base",
            "main",
            // Local tracking issue 1014 appears NOWHERE in the PR text.
            "--issue",
            "1014",
            "--work-item",
            "1014",
            // Remote head advanced past the captured sha.
            "--head-sha",
            STALE_HEAD_SHA,
        ],
    );

    assert!(
        ok,
        "scope must adopt the sole primary candidate even when the branch's local tracking issue (#1014) is absent from PR text and the head-sha is stale; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"ok\":true") && stdout.contains("\"number\":4242"),
        "the #1015-class PR (upstream-only issue reference + stale head) must remain adopted; stdout:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// Case (d): NEGATIVE safety invariant — a PR on a DIFFERENT head branch must
// NEVER be matched, no matter what discriminators are supplied.
// ---------------------------------------------------------------------------

#[test]
fn never_matches_a_pr_on_a_different_head_branch() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");

    // Candidate is OPEN, same base/repo, and its body even contains the issue
    // token — but it lives on an UNRELATED head branch.
    let list = tmp.path().join("candidates.json");
    fs::write(
        &list,
        r#"[
          {
            "number": 9001,
            "title": "Update something (#1014)",
            "body": "Closes #1014\nissue-1014",
            "state": "OPEN",
            "createdAt": "2024-03-03T00:00:00Z",
            "mergedAt": null,
            "url": "https://github.com/owner/repo/pull/9001",
            "headRefName": "some-other-unrelated-branch",
            "baseRefName": "main",
            "headRefOid": "1111111111111111111111111111111111111111",
            "headRepositoryOwner": {"login": "owner"},
            "headRepository": {"name": "repo"},
            "isCrossRepository": false,
            "isDraft": true
          }
        ]"#,
    )
    .expect("write candidates");
    write_list_stub_gh(&bin_dir, &list);

    let (stdout, ok) = run_scope(
        tmp.path(),
        &bin_dir,
        &[
            "--repo",
            "owner/repo",
            "--head",
            "feat/issue-1014-verus-phase1",
            "--base",
            "main",
            "--issue",
            "1014",
        ],
    );

    assert!(
        !ok,
        "a PR on a different head branch must never be adopted; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"reason\":\"no_scoped_pr\""),
        "different-head candidate must yield no_scoped_pr, never a match; stdout:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// Case (e): >=2 OPEN candidates on the same head+base, discriminators zero out
// -> the loud `multiple_scoped_prs` failure is preserved (genuine ambiguity).
// ---------------------------------------------------------------------------

#[test]
fn preserves_multiple_scoped_prs_when_two_open_candidates_remain() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");

    // Two OPEN PRs share the same head+base+repo (a GitHub anomaly). Neither
    // body carries the local issue token, both head-shas are stale, and neither
    // title starts with "Update" — so every discriminator zeroes out. With >=2
    // OPEN candidates remaining, the resolver must fail loudly, not guess.
    let list = tmp.path().join("candidates.json");
    fs::write(
        &list,
        r#"[
          {
            "number": 5001,
            "title": "Verus phase-1 A",
            "body": "Closes #4610",
            "state": "OPEN",
            "createdAt": "2024-04-04T00:00:00Z",
            "mergedAt": null,
            "url": "https://github.com/owner/repo/pull/5001",
            "headRefName": "feat/issue-1014-verus-phase1",
            "baseRefName": "main",
            "headRefOid": "1111111111111111111111111111111111111111",
            "headRepositoryOwner": {"login": "owner"},
            "headRepository": {"name": "repo"},
            "isCrossRepository": false,
            "isDraft": true
          },
          {
            "number": 5002,
            "title": "Verus phase-1 B",
            "body": "Closes #4610",
            "state": "OPEN",
            "createdAt": "2024-04-05T00:00:00Z",
            "mergedAt": null,
            "url": "https://github.com/owner/repo/pull/5002",
            "headRefName": "feat/issue-1014-verus-phase1",
            "baseRefName": "main",
            "headRefOid": "3333333333333333333333333333333333333333",
            "headRepositoryOwner": {"login": "owner"},
            "headRepository": {"name": "repo"},
            "isCrossRepository": false,
            "isDraft": true
          }
        ]"#,
    )
    .expect("write candidates");
    write_list_stub_gh(&bin_dir, &list);

    let (stdout, ok) = run_scope(
        tmp.path(),
        &bin_dir,
        &[
            "--repo",
            "owner/repo",
            "--head",
            "feat/issue-1014-verus-phase1",
            "--base",
            "main",
            "--issue",
            "1014",
            "--expected-pr-title-prefix",
            "Update",
            "--head-sha",
            STALE_HEAD_SHA,
        ],
    );

    assert!(
        !ok,
        "two OPEN candidates sharing head+base is a genuine ambiguity and must fail; stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("\"reason\":\"multiple_scoped_prs\""),
        "the loud multiple_scoped_prs failure must be preserved for >=2 OPEN candidates; stdout:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// Case (c): create-collision recovery in workflow_publish_pr.sh. The first
// scoped lookup finds nothing (TOCTOU), `gh pr create` collides, and a
// re-query then finds the OPEN PR -> publish must succeed as existing-open-pr.
// ---------------------------------------------------------------------------

/// A stateful stub `gh` for the publish create-collision path:
///   * `pr list` call #1 -> `[]` (nothing found yet)
///   * `pr create`       -> fails with the branch-already-exists collision
///   * `pr list` call #2 -> the now-visible OPEN PR (adopted via re-lookup)
///   * anything else     -> no-op success (labels, view, etc.)
fn write_collision_stub_gh(bin_dir: &Path, state_dir: &Path, open_pr_json: &Path) {
    fs::create_dir_all(bin_dir).expect("create bin dir");
    fs::create_dir_all(state_dir).expect("create state dir");
    let gh = bin_dir.join("gh");
    fs::write(
        &gh,
        format!(
            r#"#!/usr/bin/env bash
set -uo pipefail
state_dir={state:?}
open_pr={open:?}
sub="${{1:-}} ${{2:-}}"
case "$sub" in
  "pr list")
    count_file="$state_dir/list_count"
    n=0
    [ -f "$count_file" ] && n="$(cat "$count_file")"
    printf '%s' "$((n + 1))" > "$count_file"
    if [ "$n" -eq 0 ]; then
      printf '[]\n'
    else
      cat "$open_pr"
    fi
    ;;
  "pr create")
    echo "a pull request for branch already exists" >&2
    exit 1
    ;;
  "pr view")
    cat "$open_pr"
    ;;
  *)
    exit 0
    ;;
esac
"#,
            state = state_dir.display().to_string(),
            open = open_pr_json.display().to_string()
        ),
    )
    .expect("write collision gh stub");
    make_executable(&gh);
}

#[test]
fn publish_adopts_existing_open_pr_on_create_collision_instead_of_failing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).expect("create repo");

    // Real git repo on a feature branch with a committed diff over origin/main.
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Workflow Test"]);
    fs::write(repo.join("README.md"), "base\n").expect("write readme");
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "base"]);
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/owner/repo.git",
        ],
    );
    git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
    git(&repo, &["switch", "-c", "feat/issue-1014-collide"]);
    fs::write(repo.join("feature.txt"), "feature\n").expect("write feature");
    git(&repo, &["add", "feature.txt"]);
    git(&repo, &["commit", "-m", "feature work"]);

    // The OPEN PR that becomes visible on the re-query. Its title is NOT
    // "Update"-prefixed and its body references only the upstream issue, so it
    // is adopted purely via the (head,base,same-repo) primary key.
    let open_pr = tmp.path().join("open_pr.json");
    fs::write(
        &open_pr,
        r#"[
          {
            "number": 1015,
            "title": "Verus Phase-1 proof scaffolding",
            "body": "Closes #4610",
            "state": "OPEN",
            "createdAt": "2024-01-01T00:00:00Z",
            "mergedAt": null,
            "url": "https://github.com/owner/repo/pull/1015",
            "headRefName": "feat/issue-1014-collide",
            "baseRefName": "main",
            "headRefOid": "1111111111111111111111111111111111111111",
            "headRepositoryOwner": {"login": "owner"},
            "headRepository": {"name": "repo"},
            "isCrossRepository": false,
            "isDraft": true
          }
        ]"#,
    )
    .expect("write open pr");

    let bin_dir = tmp.path().join("bin");
    let state_dir = tmp.path().join("gh-state");
    write_collision_stub_gh(&bin_dir, &state_dir, &open_pr);

    let output = private_bash(tmp.path(), &bin_dir)
        .arg(helper_path("workflow_publish_pr.sh"))
        .current_dir(&repo)
        .env("REMOTE_HOST_TYPE", "github")
        .env("ISSUE_NUMBER", "1014")
        .env(
            "WORKFLOW_RUNTIME_ARTIFACT_HELPER",
            helper_path("workflow_runtime_artifacts.sh"),
        )
        .output()
        .expect("run workflow_publish_pr.sh");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "a create collision whose branch already has an OPEN PR must end in SUCCESS, not a recipe failure\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("\"legacy_state\":\"existing-open-pr\"")
            && stdout.contains("\"terminal_status\":\"success\""),
        "publish must adopt the already-open PR as existing-open-pr success on create collision\nstdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("FAILED_PR_CREATE"),
        "create collision recovered by an existing OPEN PR must NOT report FAILED_PR_CREATE\nstdout:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// Issue #2784: `--any-base-open`. With base_branch empty, the publish step
// resolves --base from origin/HEAD; an OPEN same-repository PR from --head into
// another base must be found instead of missed (which led to a duplicate
// `gh pr create`). The option is opt-in, read only from argv, and applies only
// in discovery mode when the resolved base has no OPEN candidate.
// ---------------------------------------------------------------------------

const ISSUE_2784_HEAD: &str = "feat/issue-611-ui";

/// A candidate from `ISSUE_2784_HEAD` with its own number, state, base and
/// head repository owner.
fn candidate_2784(number: u64, state: &str, base: &str, owner: &str) -> serde_json::Value {
    serde_json::json!({
        "number": number,
        "title": "feat: ui platform (#611)",
        "body": "Fixes #611",
        "state": state,
        "createdAt": "2026-11-17T00:00:00Z",
        "mergedAt": if state == "MERGED" { Some("2026-11-17T01:00:00Z") } else { None },
        "url": format!("https://github.com/owner/repo/pull/{number}"),
        "headRefName": ISSUE_2784_HEAD,
        "baseRefName": base,
        "headRefOid": "bd6a8e8c00000000000000000000000000000000",
        "headRepositoryOwner": {"login": owner},
        "headRepository": {"name": "repo"},
        "isCrossRepository": false,
        "isDraft": true
    })
}

/// Run the scope helper over `candidates` with `--base main` plus `extra`.
fn scope_2784(
    candidates: &[serde_json::Value],
    extra: &[&str],
    envs: &[(&str, &str)],
) -> (String, bool) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");
    let list = tmp.path().join("candidates.json");
    fs::write(
        &list,
        serde_json::Value::Array(candidates.to_vec()).to_string(),
    )
    .expect("write candidates");
    write_list_stub_gh(&bin_dir, &list);
    let mut args = vec![
        "--repo",
        "owner/repo",
        "--head",
        ISSUE_2784_HEAD,
        "--base",
        "main",
        "--issue",
        "611",
    ];
    args.extend_from_slice(extra);
    run_scope_env(tmp.path(), &bin_dir, &args, envs)
}

fn scope_json(stdout: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("scope helper must print one JSON object ({e}):\n{stdout}"))
}

#[test]
fn any_base_open_adopts_the_single_open_pr_on_another_base() {
    let (stdout, ok) = scope_2784(
        &[candidate_2784(615, "OPEN", "dev", "owner")],
        &["--any-base-open"],
        &[],
    );
    assert!(ok, "the open PR into dev must be found; stdout:\n{stdout}");
    let found = scope_json(&stdout);
    assert_eq!(found["ok"], true, "{found}");
    assert_eq!(found["scoped"], true, "{found}");
    assert_eq!(found["number"], 615, "{found}");
    assert_eq!(found["baseRefName"], "dev", "{found}");
}

#[test]
fn owner_formats_preserve_same_repository_selection_on_each_base() {
    for owner in [
        serde_json::json!({"login":"owner"}),
        serde_json::json!({"name":"owner"}),
        serde_json::json!("owner"),
    ] {
        for base in ["main", "dev"] {
            let mut candidate = candidate_2784(615, "OPEN", base, "owner");
            candidate["headRepositoryOwner"] = owner.clone();
            let (stdout, ok) = scope_2784(&[candidate], &["--any-base-open"], &[]);
            assert!(ok, "owner {owner} into {base}: {stdout}");
            assert_eq!(scope_json(&stdout)["number"], 615, "{stdout}");
            assert_eq!(scope_json(&stdout)["baseRefName"], base, "{stdout}");
        }
    }
}

#[test]
fn incomplete_owner_metadata_returns_scope_diagnostics_on_each_base() {
    for owner in [
        serde_json::json!({}),
        serde_json::Value::Null,
        serde_json::json!([]),
        serde_json::json!(42),
        serde_json::json!({"login":42}),
    ] {
        for base in ["main", "dev"] {
            let mut candidate = candidate_2784(615, "OPEN", base, "owner");
            candidate["headRepositoryOwner"] = owner.clone();
            let (stdout, ok) = scope_2784(&[candidate], &["--any-base-open"], &[]);
            assert!(!ok, "incomplete owner {owner} into {base}: {stdout}");
            let result = scope_json(&stdout);
            assert_eq!(result["reason"], "no_scoped_pr", "{stdout}");
            assert!(result["diagnostic_report"].is_string(), "{stdout}");
        }
    }
}

#[test]
fn without_any_base_open_an_open_pr_on_another_base_is_not_matched() {
    let (stdout, ok) = scope_2784(&[candidate_2784(615, "OPEN", "dev", "owner")], &[], &[]);
    assert!(
        !ok,
        "other callers keep the base-scoped lookup; stdout:\n{stdout}"
    );
    assert_eq!(scope_json(&stdout)["reason"], "no_scoped_pr", "{stdout}");
}

#[test]
fn any_base_open_never_adopts_a_pr_whose_head_is_in_another_repository() {
    // isCrossRepository is false but the head repository is other/repo: both
    // same-repository conditions must hold.
    let (stdout, ok) = scope_2784(
        &[candidate_2784(615, "OPEN", "dev", "other")],
        &["--any-base-open"],
        &[],
    );
    assert!(
        !ok,
        "a PR headed in other/repo must not match; stdout:\n{stdout}"
    );
    assert_eq!(scope_json(&stdout)["reason"], "no_scoped_pr", "{stdout}");
}

#[test]
fn any_base_open_never_adopts_a_cross_repository_pr_with_matching_repository_metadata() {
    let mut candidate = candidate_2784(615, "OPEN", "dev", "owner");
    let (stdout, ok) = scope_2784(std::slice::from_ref(&candidate), &["--any-base-open"], &[]);
    assert!(ok, "the unchanged candidate must be eligible; {stdout}");
    assert_eq!(scope_json(&stdout)["number"], 615, "{stdout}");

    // Keep the owner and repository eligible so only this predicate excludes it.
    candidate["isCrossRepository"] = true.into();
    let (stdout, ok) = scope_2784(&[candidate], &["--any-base-open"], &[]);
    assert!(
        !ok,
        "a cross-repository PR must not match even with matching repository metadata; stdout:\n{stdout}"
    );
    assert_eq!(scope_json(&stdout)["reason"], "no_scoped_pr", "{stdout}");
    assert_eq!(scope_json(&stdout)["ok"], false, "{stdout}");
}

#[test]
fn any_base_open_never_adopts_a_pr_with_an_empty_base() {
    let mut candidate = candidate_2784(615, "OPEN", "dev", "owner");
    let (stdout, ok) = scope_2784(std::slice::from_ref(&candidate), &["--any-base-open"], &[]);
    assert!(ok, "the unchanged candidate must be eligible; {stdout}");
    assert_eq!(scope_json(&stdout)["baseRefName"], "dev", "{stdout}");

    // All identity and state fields remain eligible; only the base is missing.
    candidate["baseRefName"] = "".into();
    let (stdout, ok) = scope_2784(&[candidate], &["--any-base-open"], &[]);
    assert!(
        !ok,
        "a PR with an unknown base must not match; stdout:\n{stdout}"
    );
    assert_eq!(scope_json(&stdout)["reason"], "no_scoped_pr", "{stdout}");
    assert_eq!(scope_json(&stdout)["ok"], false, "{stdout}");
}

#[test]
fn any_base_open_is_read_from_arguments_not_the_environment() {
    // An inherited variable must not turn cross-base selection on for the
    // helper's other callers.
    let (stdout, ok) = scope_2784(
        &[candidate_2784(615, "OPEN", "dev", "owner")],
        &[],
        &[("ANY_BASE_OPEN", "true")],
    );
    assert!(
        !ok,
        "ANY_BASE_OPEN in the environment must have no effect; stdout:\n{stdout}"
    );
    assert_eq!(scope_json(&stdout)["reason"], "no_scoped_pr", "{stdout}");
}

#[test]
fn any_base_open_ignores_closed_and_merged_prs_on_other_bases() {
    let (stdout, ok) = scope_2784(
        &[
            candidate_2784(615, "CLOSED", "dev", "owner"),
            candidate_2784(616, "MERGED", "release", "owner"),
        ],
        &["--any-base-open"],
        &[],
    );
    assert!(
        !ok,
        "only an OPEN PR on another base is adopted; stdout:\n{stdout}"
    );
    assert_eq!(scope_json(&stdout)["reason"], "no_scoped_pr", "{stdout}");
}

#[test]
fn any_base_open_has_no_effect_on_an_explicitly_named_pr() {
    // Authoritative mode (--pr-number) keeps the base in the primary key. The
    // helper reads a named PR with `gh pr view`, so this stub answers that
    // with PR #615 into dev; the list stub has no answer for `pr view`.
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");
    let view = tmp.path().join("view.json");
    fs::write(
        &view,
        candidate_2784(615, "OPEN", "dev", "owner").to_string(),
    )
    .expect("write view");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    fs::write(
        &gh,
        format!(
            "#!/usr/bin/env bash\nset -uo pipefail\ncase \"${{1:-}} ${{2:-}}\" in\n  \"pr view\") cat {view:?} ;;\n  *) echo 'unexpected gh call' >&2; exit 96 ;;\nesac\n",
            view = view.display().to_string()
        ),
    )
    .expect("write gh stub");
    make_executable(&gh);
    let (stdout, ok) = run_scope(
        tmp.path(),
        &bin_dir,
        &[
            "--repo",
            "owner/repo",
            "--head",
            ISSUE_2784_HEAD,
            "--base",
            "main",
            "--issue",
            "611",
            "--any-base-open",
            "--pr-number",
            "615",
        ],
    );
    assert!(
        !ok,
        "a named PR into another base is still no match; stdout:\n{stdout}"
    );
    assert_eq!(scope_json(&stdout)["reason"], "no_scoped_pr", "{stdout}");
}

/// One pull request in the shape of the REST `/pulls` payload, which the scope
/// helper reads when the GraphQL `gh pr list` call is rate-limited.
fn rest_pull_2784(number: u64, base: &str, head_full_name: &str) -> serde_json::Value {
    let (owner, name) = head_full_name
        .split_once('/')
        .expect("head_full_name is owner/name");
    serde_json::json!({
        "number": number,
        "title": "feat: ui platform (#611)",
        "body": "Fixes #611",
        "state": "open",
        "created_at": "2026-11-17T00:00:00Z",
        "merged_at": null,
        "html_url": format!("https://github.com/owner/repo/pull/{number}"),
        "head": {
            "ref": ISSUE_2784_HEAD,
            "sha": "bd6a8e8c00000000000000000000000000000000",
            "repo": {"full_name": head_full_name, "name": name, "owner": {"login": owner}}
        },
        "base": {"ref": base, "repo": {"full_name": "owner/repo"}}
    })
}

#[test]
fn any_base_open_adopts_through_the_rest_fallback_when_graphql_is_rate_limited() {
    // The factory hosts often exhaust the GraphQL budget. The helper then reads
    // the candidates from REST /pulls; the base, state and repository fields
    // mapped there must give the same result as the GraphQL list. A fork pull
    // request on the same base is in the payload and must not be counted.
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin_dir = tmp.path().join("bin");
    let pulls = tmp.path().join("pulls.json");
    let log = tmp.path().join("gh.log");
    fs::write(
        &pulls,
        serde_json::json!([
            rest_pull_2784(615, "dev", "owner/repo"),
            rest_pull_2784(700, "dev", "other/repo"),
        ])
        .to_string(),
    )
    .expect("write pulls");
    fs::create_dir_all(&bin_dir).expect("create bin dir");
    let gh = bin_dir.join("gh");
    let pulls_path =
        format!("repos/owner/repo/pulls?head=owner:{ISSUE_2784_HEAD}&state=all&per_page=100");
    fs::write(
        &gh,
        format!(
            "#!/usr/bin/env bash\nset -uo pipefail\nprintf 'call %s\\n' \"$*\" >> {log:?}\n\
             case \"${{1:-}} ${{2:-}}\" in\n\
             \x20 \"pr list\") echo 'GraphQL: API rate limit already exceeded for user ID 1.' >&2; exit 1 ;;\n\
             \x20 \"api rate_limit\") printf '{{\"resources\":{{\"graphql\":{{\"remaining\":0,\"reset\":4102444800}},\"core\":{{\"remaining\":4000,\"reset\":4102444800}}}}}}\\n' ;;\n\
             \x20 {pulls_call:?}) cat {pulls:?} ;;\n\
             \x20 *) echo 'unexpected gh call' >&2; exit 96 ;;\n\
             esac\n",
            log = log.display().to_string(),
            pulls_call = format!("api {pulls_path}"),
            pulls = pulls.display().to_string(),
        ),
    )
    .expect("write gh stub");
    make_executable(&gh);
    let (stdout, ok) = run_scope_env(
        tmp.path(),
        &bin_dir,
        &[
            "--repo",
            "owner/repo",
            "--head",
            ISSUE_2784_HEAD,
            "--base",
            "main",
            "--issue",
            "611",
            "--any-base-open",
        ],
        &[(
            "GH_RETRY_TMPDIR",
            tmp.path().to_str().expect("utf-8 tempdir"),
        )],
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        ok,
        "the open same-repository PR into dev must be found via REST; stdout:\n{stdout}\ngh calls:\n{calls}"
    );
    let found = scope_json(&stdout);
    assert_eq!(found["ok"], true, "{found}");
    assert_eq!(found["number"], 615, "{found}");
    assert_eq!(found["baseRefName"], "dev", "{found}");
    assert_eq!(found["isCrossRepository"], false, "{found}");
    assert!(
        calls.contains(&format!("call api {pulls_path}")),
        "the candidates must come from REST /pulls; gh calls:\n{calls}"
    );
}
