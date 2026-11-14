//! Issue #2333 fixture: a fake `gh` that refuses GraphQL the way GitHub's
//! secondary limit does and answers REST from fixture files, plus a small PR
//! repository whose `origin` is a local bare repository, so the head's
//! `git ls-remote origin refs/pull/7/head` path is real git, not a fake.
//!
//! Nothing here reaches GitHub, reads a credential file or names the real
//! `~/.claude` or `~/.amplihack`. The rate-limit reset epoch is written by the
//! fake itself (now + 600, or a fixed absolute value) and logged to
//! `$FAKE_LOG/reset-epoch`, so no test reads the clock.
//!
//! Modes, read on every call from files in `$FAKE_FIX` (so one fixture can
//! change mode between calls without new environment):
//!
//! | file        | values                                  | default |
//! |-------------|-----------------------------------------|---------|
//! | `gql`       | `ok`, `refuse` (secondary limit), `fail` (HTTP 502), `auth` (HTTP 401) | `ok` |
//! | `rest`      | `ok`, `refuse` (secondary limit), `fail` (HTTP 404), `auth` (HTTP 401) | `ok` |
//! | `push-during-checks` | a `gql-pr.json` that replaces it after `pr checks` answers | absent |
//! | `ub`        | `202`, `422-noop`, `422-moved`, `refuse` | `202`  |
//! | `reset-abs` | an absolute epoch for `gh api rate_limit` | now + 600 |
//! | `extra`     | text appended to every refusal's stderr  | empty   |
//!
//! Branch protection (`branches/*/protection/*`) answers 404 unless
//! `rest-protection.json` exists; `rules/branches/*` answers `[]` and
//! `branches/*` `{"protected":false}` unless their fixtures say otherwise.
//!
//! GraphQL calls are `pr view`, `pr checks`, `pr list`, `repo view` and
//! `api graphql`. Every other `api` call is REST. `api rate_limit` always
//! answers, as GitHub's does under a limit.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

use crate::support::auto_drive_light::{private_dir, show, stdout};

pub const BRANCH: &str = "quick/demo";

pub const FAKE_GH: &str = r#"#!/usr/bin/env bash
set -u
log="${FAKE_LOG:?}"; fix="${FAKE_FIX:?}"
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "$log/gh.log"
printf 'GH_REPO=%s GH_PROMPT_DISABLED=%s GH_PAGER=%s\n' "${GH_REPO:-}" "${GH_PROMPT_DISABLED:-}" "${GH_PAGER:-}" >> "$log/gh-env.log"
mode() { cat "$fix/$1" 2>/dev/null || printf '%s' "$2"; }
extra="$(cat "$fix/extra" 2>/dev/null || true)"
args=("$@"); jqf=""; slurp=0; method=GET
for ((i=0; i<${#args[@]}; i++)); do
  case "${args[$i]}" in
    --jq|-q) jqf="${args[$((i+1))]:-}" ;;
    --jq=*) jqf="${args[$i]#--jq=}" ;;
    --slurp) slurp=1 ;;
    -X|--method) method="${args[$((i+1))]:-GET}" ;;
  esac
done
emit_file() {
  local body; body="$(cat "$fix/$1")"
  [ "$slurp" = 1 ] && body="[$body]"
  if [ -n "$jqf" ]; then printf '%s' "$body" | jq -r "$jqf"; else printf '%s\n' "$body"; fi
}
refuse() {
  printf '%s: You have exceeded a secondary rate limit. Please wait a few minutes before you try again.%s\n' "$1" "${extra:+ $extra}" >&2
  exit 1
}
graphql() {
  case "$(mode gql ok)" in
    refuse) refuse "GraphQL" ;;
    fail) echo "HTTP 502: Bad Gateway (https://api.github.com/graphql)" >&2; exit 1 ;;
    auth) echo "HTTP 401: Bad credentials (https://api.github.com/graphql)" >&2; exit 1 ;;
  esac
}
case "${1:-} ${2:-}" in
  "pr view") graphql; emit_file gql-pr.json; exit 0 ;;
  "pr checks") graphql; emit_file gql-checks.json
    # push-during-checks: a push lands while gh reads the checks.
    [ ! -f "$fix/push-during-checks" ] || cp -- "$fix/push-during-checks" "$fix/gql-pr.json"
    exit 0 ;;
  "pr list") graphql; emit_file gql-pr-list.json; exit 0 ;;
  "repo view") graphql; printf '{"nameWithOwner":"o/r","owner":{"login":"o"},"name":"r"}\n'; exit 0 ;;
  "api graphql") graphql; emit_file gql-threads.json; exit 0 ;;
  "api rate_limit")
    e="$(cat "$fix/reset-abs" 2>/dev/null || echo $(( $(date +%s) + 600 )))"
    printf '%s' "$e" > "$log/reset-epoch"
    body="{\"resources\":{\"core\":{\"reset\":$e},\"graphql\":{\"reset\":$e}},\"rate\":{\"reset\":$e}}"
    if [ -n "$jqf" ]; then printf '%s' "$body" | jq -r "$jqf"; else printf '%s\n' "$body"; fi
    exit 0 ;;
esac
[ "${1:-}" = api ] || { echo "fake gh: unsupported call: $*" >&2; exit 64; }
path=""
for a in "${args[@]:1}"; do case "$a" in repos/*|/repos/*) path="${a#/}"; break ;; esac; done
case "$path" in
  */update-branch)
    printf '%s %s\n' "$method" "$*" >> "$log/update-branch.log"
    case "$(mode ub 202)" in
      202) printf '{"message":"Updating pull request branch.","url":"https://github.com/o/r/pull/7"}\n'; exit 0 ;;
      422-noop) echo 'HTTP 422: There are no new commits on the base branch. (https://api.github.com/repos/o/r/pulls/7/update-branch)' >&2; exit 1 ;;
      422-moved) echo "HTTP 422: expected head sha didn't match current head ref. (https://api.github.com/repos/o/r/pulls/7/update-branch)" >&2; exit 1 ;;
      refuse) refuse "HTTP 403" ;;
    esac ;;
esac
case "$(mode rest ok)" in
  refuse) refuse "HTTP 403" ;;
  fail) echo "HTTP 404: Not Found (https://api.github.com/$path)" >&2; exit 1 ;;
  auth) echo "HTTP 401: Bad credentials (https://api.github.com/$path)" >&2; exit 1 ;;
esac
case "$path" in
  */merge|*/merges) echo "fake gh: a merge was attempted" >> "$log/merge-attempt"; exit 1 ;;
  repos/*/*/pulls/7/reviews*) emit_file rest-reviews.json ;;
  repos/*/*/branches/*/protection/*)
    [ -f "$fix/rest-protection.json" ] || { echo "HTTP 404: Not Found (https://api.github.com/$path)" >&2; exit 1; }
    emit_file rest-protection.json ;;
  repos/*/*/rules/branches/*) [ -f "$fix/rest-rules.json" ] || printf '[]' > "$fix/rest-rules.json"; emit_file rest-rules.json ;;
  repos/*/*/branches/*) [ -f "$fix/rest-branch.json" ] || printf '{"protected":false}' > "$fix/rest-branch.json"; emit_file rest-branch.json ;;
  repos/*/*/commits/*/check-runs*) emit_file rest-check-runs.json ;;
  repos/*/*/commits/*/status*) emit_file rest-status.json ;;
  repos/*/*/pulls/7) emit_file rest-pr.json ;;
  repos/*/*/pulls|repos/*/*/pulls\?*) emit_file rest-pulls.json ;;
  repos/*/*) printf '{"full_name":"o/r","name":"r","owner":{"login":"o"}}\n' ;;
  *) echo "HTTP 404: Not Found (https://api.github.com/$path)" >&2; exit 1 ;;
esac
exit 0
"#;

/// A PR worktree on `quick/demo` with a bare `origin`, the fake `gh` and a
/// link to the real `amplihack` first on `PATH`, and the fixture files.
pub struct GhRepo {
    pub tmp: tempfile::TempDir,
    pub repo: PathBuf,
    pub origin: PathBuf,
    pub bin: PathBuf,
    pub log: PathBuf,
    pub fix: PathBuf,
    pub home: PathBuf,
}

impl GhRepo {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir_in(crate::support::scratch_base::scratch_base()).unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let (repo, origin, bin, log, fix, home) = (
            root.join("pr"),
            root.join("origin.git"),
            root.join("fake-bin"),
            root.join("fake-log"),
            root.join("fake-fix"),
            root.join("home"),
        );
        for d in [&bin, &log, &fix, &home] {
            private_dir(d);
        }
        install(&bin, &log);
        let g = Self {
            tmp,
            repo,
            origin,
            bin,
            log,
            fix,
            home,
        };
        git_in(&root, &["init", "-q", "--bare", g.origin.to_str().unwrap()]);
        git_in(
            &root,
            &["init", "-q", "-b", BRANCH, g.repo.to_str().unwrap()],
        );
        git_in(&g.repo, &["commit", "--allow-empty", "-qm", "pr head"]);
        git_in(
            &g.repo,
            &["remote", "add", "origin", g.origin.to_str().unwrap()],
        );
        git_in(&g.repo, &["push", "-q", "origin", "HEAD:refs/heads/main"]);
        g.write_fixtures(&g.head());
        g
    }

    pub fn head(&self) -> String {
        git_in(&self.repo, &["rev-parse", "HEAD"])
    }

    /// Publishes the head as `refs/pull/7/head` on origin, as GitHub does.
    pub fn publish_pull_ref(&self) {
        git_in(
            &self.repo,
            &["push", "-q", "origin", "HEAD:refs/pull/7/head"],
        );
    }

    pub fn mode(&self, file: &str, value: &str) {
        std::fs::write(self.fix.join(file), value).unwrap();
    }

    pub fn fixture(&self, file: &str, value: &Value) {
        std::fs::write(self.fix.join(file), value.to_string()).unwrap();
    }

    /// The GraphQL and REST answers describe the same PR, so a REST read and
    /// a GraphQL read must map to equal output (D4).
    pub fn write_fixtures(&self, head: &str) {
        write_fixtures(&self.fix, head);
    }

    pub fn path(&self) -> std::ffi::OsString {
        std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )),
        )
        .unwrap()
    }

    pub fn cmd(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut c = crate::isolated_amplihack::isolated(program);
        c.current_dir(&self.repo)
            .env("PATH", self.path())
            .env("HOME", &self.home)
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env("FAKE_LOG", &self.log)
            .env("FAKE_FIX", &self.fix)
            .env("GIT_CONFIG_NOSYSTEM", "1");
        c
    }

    /// `amplihack orch helper autodrive-gh-read <args>`.
    pub fn read(&self, args: &[&str]) -> Output {
        self.cmd(crate::support::amplihack_bin::amplihack_bin())
            .args(["orch", "helper", "autodrive-gh-read"])
            .args(args)
            .output()
            .unwrap()
    }

    pub fn update_branch(&self, args: &[&str]) -> Output {
        self.cmd(crate::support::amplihack_bin::amplihack_bin())
            .args(["orch", "helper", "autodrive-gh-update-branch"])
            .args(args)
            .output()
            .unwrap()
    }

    pub fn calls(&self) -> Vec<String> {
        lines(&self.log.join("gh.log"))
    }

    pub fn reset_epoch(&self) -> Option<i64> {
        std::fs::read_to_string(self.log.join("reset-epoch"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
    }
}

/// Writes the fake `gh` and an `amplihack` link to the binary under test.
pub fn install(bin: &Path, log: &Path) {
    crate::support::exec_stub::write_executable(&bin.join("gh"), FAKE_GH, 0o755);
    let link = bin.join("amplihack");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(crate::support::amplihack_bin::amplihack_bin(), &link).unwrap();
    let _ = std::fs::write(log.join("gh.log"), "");
}

pub fn write_fixtures(fix: &Path, head: &str) {
    let w = |n: &str, v: Value| std::fs::write(fix.join(n), v.to_string()).unwrap();
    w(
        "gql-pr.json",
        json!({
            "number": 7, "state": "OPEN", "isDraft": false, "headRefOid": head,
            "headRefName": BRANCH, "baseRefName": "main", "mergeStateStatus": "BLOCKED",
            "mergeable": "MERGEABLE", "mergedAt": null, "reviewDecision": "APPROVED",
            "url": "https://github.com/o/r/pull/7"
        }),
    );
    w(
        "gql-pr-list.json",
        json!([{"number": 7, "headRefName": BRANCH}]),
    );
    w(
        "gql-checks.json",
        json!([
            {"name": "build", "bucket": "pass", "state": "SUCCESS"},
            {"name": "lint", "bucket": "pending", "state": "IN_PROGRESS"},
            {"name": "e2e", "bucket": "pending", "state": "QUEUED"},
            {"name": "docs", "bucket": "skipping", "state": "SKIPPED"},
            {"name": "legacy/ci", "bucket": "fail", "state": "FAILURE"}
        ]),
    );
    w(
        "gql-threads.json",
        json!({"data": {"repository": {"pullRequest": {"reviewThreads": {
            "pageInfo": {"hasNextPage": false, "endCursor": null},
            "nodes": [{"isResolved": false}, {"isResolved": true}]
        }}}}}),
    );
    w(
        "rest-pr.json",
        json!({
            "number": 7, "state": "open", "draft": false, "merged_at": null,
            "head": {"sha": head, "ref": BRANCH}, "base": {"ref": "main"},
            "mergeable": true, "mergeable_state": "blocked"
        }),
    );
    w(
        "rest-pulls.json",
        json!([{"number": 7, "head": {"ref": BRANCH, "sha": head}}]),
    );
    w(
        "rest-check-runs.json",
        json!({"total_count": 4, "check_runs": [
            {"name": "build", "status": "completed", "conclusion": "success"},
            {"name": "lint", "status": "in_progress", "conclusion": null},
            {"name": "e2e", "status": "queued", "conclusion": null},
            {"name": "docs", "status": "completed", "conclusion": "skipped"}
        ]}),
    );
    w(
        "rest-status.json",
        json!({"state": "failure", "statuses": [{"context": "legacy/ci", "state": "failure"}]}),
    );
    w(
        "rest-reviews.json",
        json!([
            {"user": {"login": "alice"}, "state": "COMMENTED"},
            {"user": {"login": "alice"}, "state": "APPROVED"},
            {"user": {"login": "bob"}, "state": "CHANGES_REQUESTED"},
            {"user": {"login": "bob"}, "state": "DISMISSED"},
            {"user": {"login": "carol"}, "state": "COMMENTED"}
        ]),
    );
}

pub fn git_in(dir: &Path, args: &[&str]) -> String {
    let o = amplihack_git::command_in(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(o.status.success(), "git {args:?}: {}", show(&o));
    stdout(&o).trim().to_string()
}

pub fn lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The single JSON line the helper prints.
pub fn one_json(o: &Output) -> Value {
    let out = stdout(o);
    let mut it = out.lines().filter(|l| !l.trim().is_empty());
    let first = it
        .next()
        .unwrap_or_else(|| panic!("helper printed nothing: {}", show(o)));
    assert!(it.next().is_none(), "exactly one JSON line: {}", show(o));
    serde_json::from_str(first).unwrap_or_else(|e| panic!("not JSON ({e}): {}", show(o)))
}

/// `v` without `source`, for the GraphQL/REST equality check.
pub fn without_source(v: &Value) -> Value {
    let mut v = v.clone();
    v.as_object_mut().unwrap().remove("source");
    v
}

// ── IV3: the previous round's `.wait` sidecar ───────────────────────────────

impl GhRepo {
    /// Writes a previous-round sidecar under the fixture's state directory,
    /// as merge-evidence step-01 does, and returns its path. `body` is
    /// written verbatim so a test can write a malformed one.
    pub fn prior_wait_raw(&self, body: &str) -> PathBuf {
        let state = self.tmp.path().join("state");
        private_dir(&state);
        let p = state.join("merge-ready-round-1.json.wait");
        std::fs::write(&p, body).unwrap();
        p
    }

    /// A valid sidecar recording `UNKNOWN` at `head`.
    pub fn prior_wait(&self, head: &str) -> PathBuf {
        self.prior_wait_raw(&unknown_sidecar(head))
    }

    /// Calls made to `git` through the logging wrapper from [`Self::fake_git`].
    pub fn git_calls(&self) -> Vec<String> {
        lines(&self.log.join("git.log"))
    }

    /// Puts a `git` wrapper first on `PATH`. It logs every call to
    /// `$FAKE_LOG/git.log` (arguments joined by 0x1f) and runs the real git,
    /// except `remote get-url origin`, which prints `origin_url` so the
    /// fixture's local bare origin can stand for a GitHub repository without
    /// any network access.
    pub fn fake_git(&self, origin_url: &str) {
        std::fs::write(self.fix.join("origin-url"), origin_url).unwrap();
        let real = real_git();
        let script = format!(
            r#"#!/usr/bin/env bash
set -u
( IFS=$'\x1f'; printf '%s\n' "$*" ) >> "${{FAKE_LOG:?}}/git.log"
prev2=""; prev1=""
for a in "$@"; do
  if [ "$prev2 $prev1 $a" = "remote get-url origin" ]; then
    cat "${{FAKE_FIX:?}}/origin-url"; printf '\n'; exit 0
  fi
  prev2="$prev1"; prev1="$a"
done
exec '{real}' "$@"
"#,
            real = real.display()
        );
        crate::support::exec_stub::write_executable(&self.bin.join("git"), &script, 0o755);
        let _ = std::fs::write(self.log.join("git.log"), "");
    }
}

/// The JSON a valid `UNKNOWN` sidecar holds.
pub fn unknown_sidecar(head: &str) -> String {
    json!({"wait": format!("mergeability@{head}"), "head_sha": head, "merge_state": "UNKNOWN"})
        .to_string()
}

/// The first `git` on the inherited `PATH`, resolved before the fixture's
/// own `bin` is put in front of it.
fn real_git() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH")
}
