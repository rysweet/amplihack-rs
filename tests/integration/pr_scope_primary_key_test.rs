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

/// Run `workflow_pr_scope.sh` with the stub `gh` on PATH. Returns (stdout,
/// success). No git repo is required: `--repo/--head/--base/--head-sha` are all
/// passed explicitly.
fn run_scope(dir: &Path, bin_dir: &Path, args: &[&str]) -> (String, bool) {
    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(helper_path("workflow_pr_scope.sh"))
        .args(args)
        .current_dir(dir)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
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
    // the default "Update" prefix, body references only the UPSTREAM issue
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

    let old_path = std::env::var("PATH").unwrap_or_default();
    let output = Command::new("bash")
        .arg(helper_path("workflow_publish_pr.sh"))
        .current_dir(&repo)
        .env("PATH", format!("{}:{old_path}", bin_dir.display()))
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
