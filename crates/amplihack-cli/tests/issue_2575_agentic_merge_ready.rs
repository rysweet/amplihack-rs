//! Issue #2575: the merge depends only on GitHub's own conditions and the
//! captured authorisation.
//!
//! Contract: docs/reference/autodrive-goal-loop.md.
//!
//! - The merge depends only on the captured authorisation, the head the turn
//!   read, GitHub's checks and GitHub accepting `--match-head-commit`
//!   (`autodrive_merge`, and since #3129 the goal loop's merge action,
//!   `autodrive_pr_goal.sh act merge`, which replaced the finalize `merge`
//!   step).
//! - `amplihack orch helper autodrive authorization` reports the captured
//!   permission.
//!
//! #3129 retired the merge-ready round's word and record tools, the merge
//! loop's step-02 and step-02a and the driver they ran; their tests went with
//! them.
//!
//! Every process runs under `isolated("bash")` with HOME, AMPLIHACK_HOME (a
//! copied bundle), CLAUDE_CONFIG_DIR and the XDG directories in a temporary
//! folder, and with fake `gh` and `amplihack` first on PATH. Nothing reaches
//! GitHub or an agent, and nothing under the real `~/.amplihack` or
//! `~/.claude` is named. tests/gadugi/* is not run here.

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::support::auto_drive_light::{
    jq_available, private_dir, private_file, repo_root, show, stdout,
};
use crate::support::exec_stub::write_executable;

const PR: &str = "2575";
const SHA_A: &str = concat!("15a7f6f47c1e2d0b9a8f", "7e6d5c4b3a2918070605");
const SHA_B: &str = concat!("aaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaaaaaaa");
const RUN_ID: &str = "a3f9c2e1-7b44-4d0e-9a51-2c6f0e8d1b37";
const PROVENANCE: &str = concat!(
    "trusted-launch-bundle:sha256:",
    "0123456789abcdef0123456789abcdef",
    "0123456789abcdef0123456789abcdef"
);

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// A fake `gh` driven by files in `$FAKE_STATE`:
/// `state` (OPEN), `title`, `body`, `server_head`, `required` and `all`
/// (pass | fail | pending | none), `merge_rc`, `rc79_on` (view | checks |
/// merge). Every call is logged to `gh.log`, one argv per line joined by
/// U+001F; the merge body read from stdin goes to `merge-body`.
///
/// #2575 B2/M1 additions: `merge_state` (the `mergeStateStatus` read before a
/// merge request, default CLEAN), `merge_state_after` (the value read once a
/// merge request has been made), `check_name` (the check's name, default
/// `ci`), `base_ref`, `head_ref` and `cross` (`isCrossRepository`).
/// #2663: `merge_rc_after` is the exit status of a merge request that GitHub
/// did carry out (the PR reads MERGED, yet gh reports an error).
const FAKE_GH: &str = r#"#!/usr/bin/env bash
set -u
S="$FAKE_STATE"
( IFS=$'\037'; printf '%s\n' "$*" ) >> "$S/gh.log"
[ -f "$S/rc79_on" ] && [ "$(cat "$S/rc79_on")" = "${2:-}" ] && { echo "HTTP 401: Bad credentials (fixture terminal refusal)" >&2; exit 79; }
opt() { local want="$1" prev="" a; shift; for a in "$@"; do [ "$prev" = "$want" ] && { printf '%s' "$a"; return 0; }; prev="$a"; done; return 1; }
# #1888: `gh api repos/{owner}/{repo} --jq '[...]|@tsv'` answers the three
# allowed-method booleans (allow_merge_commit, allow_squash_merge,
# allow_rebase_merge) from `allow_merge`, `allow_squash`, `allow_rebase`
# (each default true). `api_rc` makes the call fail with that status, writing
# `api_err` verbatim to standard error when it exists and a fixed line
# otherwise; `api_out` replaces the answer verbatim. Keyed on $1: for `gh api`
# the second word is the path, so `rc79_on` never matches it.
if [ "${1:-}" = api ]; then
  if [ -f "$S/api_rc" ]; then
    if [ -f "$S/api_err" ]; then cat "$S/api_err" >&2; else echo "fixture: gh api failed" >&2; fi
    exit "$(cat "$S/api_rc")"
  fi
  [ -f "$S/api_out" ] && { cat "$S/api_out"; exit 0; }
  printf '%s\t%s\t%s\n' "$(cat "$S/allow_merge" 2>/dev/null || echo true)" \
    "$(cat "$S/allow_squash" 2>/dev/null || echo true)" "$(cat "$S/allow_rebase" 2>/dev/null || echo true)"
  exit 0
fi
case "${1:-} ${2:-}" in
  "pr view")
    ms="$(cat "$S/merge_state" 2>/dev/null || echo CLEAN)"
    [ -f "$S/merge-body" ] && [ -f "$S/merge_state_after" ] && ms="$(cat "$S/merge_state_after")"
    obj="$(jq -cn --arg state "$(cat "$S/state" 2>/dev/null || echo OPEN)" \
      --arg title "$(cat "$S/title" 2>/dev/null || echo 'Retire the parsed record')" \
      --rawfile body "$S/body" --arg head "$(cat "$S/server_head")" --arg ms "$ms" \
      --arg base "$(cat "$S/base_ref" 2>/dev/null || echo main)" \
      --arg headref "$(cat "$S/head_ref" 2>/dev/null || echo feat/issue-2575)" \
      --argjson cross "$(cat "$S/cross" 2>/dev/null || echo false)" \
      '{number:2575,state:$state,title:$title,body:$body,headRefOid:$head,isDraft:false,mergeStateStatus:$ms,baseRefName:$base,headRefName:$headref,isCrossRepository:$cross}')"
    if q="$(opt --jq "$@")" || q="$(opt -q "$@")"; then printf '%s' "$obj" | jq -r "$q"
    elif f="$(opt --json "$@")"; then printf '%s' "$obj" | jq -c --arg f "$f" '. as $o | reduce ($f | split(","))[] as $k ({}; .[$k] = $o[$k])'
    else printf '%s\n' "$obj"; fi
    exit 0 ;;
  "pr checks")
    req=0; for a in "$@"; do [ "$a" = --required ] && req=1; done
    if [ "$req" = 1 ]; then mode="$(cat "$S/required" 2>/dev/null || echo pass)"; else mode="$(cat "$S/all" 2>/dev/null || echo pass)"; fi
    case "$mode" in
      none)
        if [ "$req" = 1 ]; then echo "no required checks reported on the 'feat/issue-2575' branch" >&2
        else echo "no checks reported on the 'feat/issue-2575' branch" >&2; fi
        exit 1 ;;
      pass) b=pass; rc=0 ;;
      fail) b=fail; rc=1 ;;
      *) b=pending; rc=8 ;;
    esac
    n="$(cat "$S/check_name" 2>/dev/null || echo ci)"
    if opt --json "$@" >/dev/null; then jq -cn --arg b "$b" --arg n "$n" '[{name:$n,bucket:$b,state:($b | ascii_upcase)}]'
    else printf '%s\t%s\t1m\thttps://example.invalid/ci\n' "$n" "$b"; fi
    exit "$rc" ;;
  "pr merge")
    cat > "$S/merge-body"
    sha="$(opt --match-head-commit "$@")" || sha=""
    if [ "$sha" != "$(cat "$S/server_head")" ]; then
      echo "GraphQL: Head branch was modified. Review and try the merge again. (mergePullRequest)" >&2; exit 1
    fi
    [ -f "$S/merge_rc" ] && { echo "fixture: merge refused by branch protection" >&2; exit "$(cat "$S/merge_rc")"; }
    printf MERGED > "$S/state"
    exit "$(cat "$S/merge_rc_after" 2>/dev/null || echo 0)" ;;
esac
exit 0
"#;

/// A fake `amplihack`: `orch helper autodrive authorization` answers from
/// `auth_out` / `auth_rc`; `extract-json` / `extract-field` are jq-based.
/// Every call is logged to `amplihack.log`.
const FAKE_AMPLIHACK: &str = r#"#!/usr/bin/env bash
set -u
S="$FAKE_STATE"
( IFS=$'\037'; printf '%s\n' "$*" ) >> "$S/amplihack.log"
case "${1:-} ${2:-}" in
esac
case "${1:-} ${2:-} ${3:-} ${4:-}" in
  "orch helper autodrive authorization")
    [ -f "$S/auth_out" ] && cat "$S/auth_out"
    exit "$(cat "$S/auth_rc" 2>/dev/null || echo 0)" ;;
esac
if [ "${1:-} ${2:-}" = "orch helper" ]; then
  in="$(cat)"
  if [ "${3:-}" = "extract-json" ]; then
    f=""; while [ $# -gt 0 ]; do [ "$1" = "--require-field" ] && f="${2:-}"; shift; done
    printf '%s\n' "$in" | jq -R -c --arg f "$f" 'fromjson? | select(type == "object" and has($f))' 2>/dev/null | tail -n 1
    exit 0
  fi
  if [ "${3:-}" = "extract-field" ]; then
    f=""; d=""
    while [ $# -gt 0 ]; do case "$1" in --field) f="${2:-}" ;; --default) d="${2:-}" ;; esac; shift; done
    v=""
    [ -n "$in" ] && v="$(printf '%s' "$in" | jq -r --arg f "$f" 'if .[$f] == null then empty else .[$f] end | if type == "string" then . else tojson end' 2>/dev/null)"
    if [ -n "$v" ]; then printf '%s' "$v"; else printf '%s' "$d"; fi
    exit 0
  fi
fi
exit 0
"#;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Fx {
    /// The fixture root, in the effective group so that what the tests make
    /// in it is not refused as foreign-group evidence (issue #2931).
    env: crate::isolated_amplihack::PrivateEnv,
    root: PathBuf,
    ah: PathBuf,
    bin: PathBuf,
    fake: PathBuf,
    state: PathBuf,
    repo: PathBuf,
}

fn run_ok(cmd: &mut Command, what: &str) {
    let out = cmd.output().unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(out.status.success(), "{what}: {}", show(&out));
}

impl Fx {
    fn new() -> Option<Self> {
        if !jq_available() {
            eprintln!("SKIP: jq is not installed; the fakes and tools need it");
            return None;
        }
        let env =
            crate::isolated_amplihack::private_env_in(crate::support::scratch_base::scratch_base());
        let root = env.root().to_path_buf();
        let p = |n: &str| root.join(n);
        let (home, ah, bin, fake, state, repo) = (
            env.home().to_path_buf(),
            p("amplihack-home"),
            p("bin"),
            p("fake-state"),
            p("state"),
            p("repo"),
        );
        for d in [&home, &bin, &fake, &repo, &ah.join("amplifier-bundle")] {
            std::fs::create_dir_all(d).unwrap();
        }
        for d in [
            "claude-config",
            "xdg-state",
            "xdg-config",
            "xdg-cache",
            "xdg-data",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        private_dir(&state);
        // AMPLIHACK_HOME is a copy of the launch bundle's tools and recipes.
        for sub in ["tools", "recipes"] {
            run_ok(
                Command::new("cp")
                    .arg("-R")
                    .arg(repo_root().join("amplifier-bundle").join(sub))
                    .arg(ah.join("amplifier-bundle")),
                "copy bundle",
            );
        }
        run_ok(
            Command::new("chmod").arg("-R").arg("go-w").arg(&ah),
            "chmod bundle",
        );
        write_executable(&bin.join("gh"), FAKE_GH, 0o755);
        write_executable(&bin.join("amplihack"), FAKE_AMPLIHACK, 0o755);
        std::fs::write(
            fake.join("body"),
            "Description text.\n\n## Merge readiness\nVerdict: MERGE_READY\n",
        )
        .unwrap();
        std::fs::write(fake.join("server_head"), SHA_A).unwrap();
        Some(Fx {
            env,
            root,
            ah,
            bin,
            fake,
            state,
            repo,
        })
    }

    fn set(&self, name: &str, value: &str) {
        std::fs::write(self.fake.join(name), value).unwrap();
    }

    fn tools(&self) -> PathBuf {
        self.ah.join("amplifier-bundle/tools")
    }

    /// An isolated bash with every home-like directory in the fixture.
    fn bash(&self) -> Command {
        let path = format!(
            "{}:{}",
            self.bin.display(),
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
        );
        // `HOME`, `TMPDIR` and `CLAUDE_CONFIG_DIR` come from the private
        // environment, so a script's temporary files stay in the fixture.
        let mut c = self.env.command("bash");
        c.env("PATH", path)
            .env("AMPLIHACK_HOME", &self.ah)
            .env("AMPLIHACK_BIN", self.bin.join("amplihack"))
            .env("XDG_STATE_HOME", self.root.join("xdg-state"))
            .env("XDG_CONFIG_HOME", self.root.join("xdg-config"))
            .env("XDG_CACHE_HOME", self.root.join("xdg-cache"))
            .env("XDG_DATA_HOME", self.root.join("xdg-data"))
            .env("FAKE_STATE", &self.fake)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GH_TOKEN")
            .env_remove("GITHUB_TOKEN")
            .current_dir(&self.root);
        c
    }

    fn merge_tool(&self, pr: &str, sha: &str) -> Output {
        self.merge_tool_method(pr, sha, "")
    }

    /// `autodrive_merge <pr> <sha> <merge_method>`; an empty method is what a
    /// caller that was given none passes.
    fn merge_tool_method(&self, pr: &str, sha: &str, method: &str) -> Output {
        let mut c = self.bash();
        c.arg("-c")
            .arg(
                "set -u\n. \"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge.sh\" || exit 90\n\
                 autodrive_merge \"$1\" \"$2\" \"$3\"",
            )
            .arg("bash")
            .args([pr, sha, method]);
        feed(c, b"")
    }

    /// The `gh api` calls made so far.
    fn api_calls(&self) -> Vec<Vec<String>> {
        self.gh_calls()
            .into_iter()
            .filter(|c| c.first().map(String::as_str) == Some("api"))
            .collect()
    }

    fn log(&self, name: &str) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.fake.join(name))
            .unwrap_or_default()
            .lines()
            .map(|l| l.split('\u{1f}').map(str::to_string).collect())
            .collect()
    }

    fn gh_calls(&self) -> Vec<Vec<String>> {
        self.log("gh.log")
    }

    fn merge_calls(&self) -> Vec<Vec<String>> {
        self.gh_calls()
            .into_iter()
            .filter(|c| {
                c.first().map(String::as_str) == Some("pr")
                    && c.get(1).map(String::as_str) == Some("merge")
            })
            .collect()
    }

    /// A git repository at `self.repo` with one commit; returns its HEAD.
    fn git_repo(&self) -> String {
        let git = |args: &[&str]| crate::support::quick_change_repo::git(&self.repo, args);
        git(&["init", "-q"]);
        std::fs::write(self.repo.join("README.md"), "fixture\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "-q", "-m", "init"]);
        git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    fn commit(&self, message: &str) -> String {
        let git = |args: &[&str]| crate::support::quick_change_repo::git(&self.repo, args);
        git(&["commit", "-q", "--allow-empty", "-m", message]);
        git(&["rev-parse", "HEAD"]).trim().to_string()
    }
}

fn feed(mut c: Command, stdin: &[u8]) -> Output {
    use std::io::Write;
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().expect("spawn bash");
    child.stdin.take().unwrap().write_all(stdin).ok();
    child.wait_with_output().expect("wait for bash")
}

/// The last stdout line that is a JSON object holding `field`.
fn last_json(o: &Output, field: &str) -> Value {
    stdout(o)
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v.get(field).is_some())
        .unwrap_or_else(|| panic!("no JSON line with `{field}`:\n{}", show(o)))
}

// ===========================================================================
// autodrive_merge: GitHub's conditions only
// ===========================================================================

fn assert_no_merge_attempt(fx: &Fx) {
    assert!(
        fx.merge_calls().is_empty(),
        "no merge may be attempted: {:?}",
        fx.gh_calls()
    );
}

#[test]
fn merge_goes_ahead_on_matching_head_and_green_checks() {
    let Some(fx) = Fx::new() else { return };
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let v = last_json(&out, "result");
    assert_eq!(v["result"], json!("merged"));
    assert_eq!(v["pr"], json!(2575));
    assert_eq!(v["expected_head"], json!(SHA_A));
    let merges = fx.merge_calls();
    assert_eq!(merges.len(), 1, "one merge request, no retries: {merges:?}");
    let m = &merges[0];
    let after = |flag: &str| {
        m.iter()
            .position(|a| a == flag)
            .and_then(|i| m.get(i + 1))
            .cloned()
    };
    assert!(m.contains(&PR.to_string()), "{m:?}");
    assert!(m.contains(&"--squash".to_string()), "{m:?}");
    assert_eq!(after("--match-head-commit").as_deref(), Some(SHA_A));
    assert_eq!(
        after("--body-file").as_deref(),
        Some("-"),
        "body on stdin (#2056)"
    );
    assert_eq!(
        after("--subject").as_deref(),
        Some("Retire the parsed record (#2575)"),
        "subject from the trusted merge-message helper (#2056)"
    );
    let body = std::fs::read_to_string(fx.fake.join("merge-body")).unwrap();
    assert!(body.starts_with("Description text."), "{body:?}");
    assert!(
        !body.contains("Merge readiness"),
        "the report is cut: {body:?}"
    );
    // The PR state is read again after the merge.
    let last = fx.gh_calls().last().cloned().unwrap();
    assert_eq!(&last[..2], ["pr", "view"], "{last:?}");
}

#[test]
fn moved_head_is_refused_then_handoff() {
    let Some(fx) = Fx::new() else { return };
    fx.set("server_head", SHA_B);
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(4), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("refused"));
    assert_eq!(fx.merge_calls().len(), 1, "GitHub decides; no retry");
}

#[test]
fn github_refusal_for_protection_or_reviews_is_refused() {
    let Some(fx) = Fx::new() else { return };
    fx.set("merge_rc", "1");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(4), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("refused"));
}

#[test]
fn failing_checks_prevent_merge_attempt() {
    // A pending required check is also not green since #3129; see
    // issue_2575_wait_sync_success.rs.
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "fail");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    assert_eq!(
        last_json(&out, "result")["result"],
        json!("checks-not-green")
    );
    assert_no_merge_attempt(&fx);
}

#[test]
fn no_required_checks_falls_back_to_all_checks() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "none");
    fx.set("all", "pass");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("merged"));
    let checks: Vec<_> = fx
        .gh_calls()
        .into_iter()
        .filter(|c| c.get(1).map(String::as_str) == Some("checks"))
        .collect();
    assert!(
        checks
            .iter()
            .any(|c| !c.contains(&"--required".to_string())),
        "the fallback reads all checks: {checks:?}"
    );
}

#[test]
fn failing_checks_after_the_fallback_prevent_merge_attempt() {
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "none");
    fx.set("all", "fail");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(3), "{}", show(&out));
    assert_no_merge_attempt(&fx);
}

#[test]
fn no_checks_at_all_merges() {
    // GitHub does not require checks: a PR with none reported is not refused
    // for that reason, and GitHub's own branch protection decides the merge.
    let Some(fx) = Fx::new() else { return };
    fx.set("required", "none");
    fx.set("all", "none");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(last_json(&out, "result")["result"], json!("merged"));
    let merges = fx.merge_calls();
    assert_eq!(merges.len(), 1, "{merges:?}");
    assert!(
        merges[0].contains(&"--match-head-commit".to_string()),
        "{merges:?}"
    );
}

#[test]
fn a_pr_that_is_not_open_is_not_merged() {
    // A PR merged at another head is not this run's merge (#2663); one merged
    // at the expected head is covered in issue_2575_wait_sync_success.rs.
    let Some(fx) = Fx::new() else { return };
    fx.set("server_head", SHA_B);
    for state in ["CLOSED", "MERGED"] {
        fx.set("state", state);
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(5), "{state}: {}", show(&out));
        assert_eq!(last_json(&out, "result")["result"], json!("not-open"));
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn gh_exit_79_passes_through_merge_tool() {
    let Some(fx) = Fx::new() else { return };
    for at in ["view", "checks", "merge"] {
        let _ = std::fs::remove_file(fx.fake.join("gh.log"));
        fx.set("state", "OPEN");
        fx.set("rc79_on", at);
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(79), "79 at {at}: {}", show(&out));
        let calls = fx.gh_calls();
        let idx = calls
            .iter()
            .position(|c| c.get(1).map(String::as_str) == Some(at))
            .expect("the refusing call was made");
        assert_eq!(
            idx + 1,
            calls.len(),
            "nothing runs after a 79 at {at}: {calls:?}"
        );
    }
}

#[test]
fn merge_tool_rejects_bad_arguments_without_calling_gh() {
    let Some(fx) = Fx::new() else { return };
    let upper = SHA_A.to_uppercase();
    for (pr, sha) in [
        ("", SHA_A),
        ("0", SHA_A),
        ("-1", SHA_A),
        ("12a", SHA_A),
        ("--repo=x", SHA_A),
        (PR, ""),
        (PR, "abc"),
        (PR, upper.as_str()),
        (PR, "HEAD"),
    ] {
        let out = fx.merge_tool(pr, sha);
        assert_eq!(out.status.code(), Some(2), "{pr:?} {sha:?}: {}", show(&out));
    }
    assert!(fx.gh_calls().is_empty(), "{:?}", fx.gh_calls());
}

#[test]
fn merge_tool_refuses_a_symlinked_merge_message_helper() {
    let Some(fx) = Fx::new() else { return };
    let helper = fx.tools().join("autodrive_merge_message.sh");
    let moved = fx.root.join("elsewhere-merge-message.sh");
    std::fs::rename(&helper, &moved).unwrap();
    symlink(&moved, &helper).unwrap();
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    assert_no_merge_attempt(&fx);
}

// ===========================================================================
// #1888: the merge method is one the repository allows
// ===========================================================================

/// The flags of the single merge request, asserting that exactly one was made,
/// that the head is pinned and that no bypass flag is present.
fn merge_flags(fx: &Fx) -> Vec<String> {
    let merges = fx.merge_calls();
    assert_eq!(merges.len(), 1, "one merge request: {merges:?}");
    let m = merges[0].clone();
    let at = m.iter().position(|a| a == "--match-head-commit");
    assert_eq!(
        at.and_then(|i| m.get(i + 1)).map(String::as_str),
        Some(SHA_A),
        "every method pins the head: {m:?}"
    );
    for forbidden in ["--admin", "--auto", "--bypass"] {
        assert!(!m.contains(&forbidden.to_string()), "{forbidden}: {m:?}");
    }
    m
}

fn method_flags(m: &[String]) -> Vec<&str> {
    m.iter()
        .map(String::as_str)
        .filter(|a| matches!(*a, "--squash" | "--merge" | "--rebase"))
        .collect()
}

/// Only the repository settings differ from the defaults (all allowed).
fn repo_allows(fx: &Fx, merge: bool, squash: bool, rebase: bool) {
    fx.set("allow_merge", &merge.to_string());
    fx.set("allow_squash", &squash.to_string());
    fx.set("allow_rebase", &rebase.to_string());
}

/// Exit 4, `refused`, a reason that starts as the hand-off expects, one JSON
/// line, and no merge request.
fn assert_unsettled(fx: &Fx, out: &Output) -> Value {
    assert_eq!(out.status.code(), Some(4), "{}", show(out));
    let json_lines: Vec<Value> = stdout(out)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l:?}")))
        .collect();
    assert_eq!(json_lines.len(), 1, "one JSON line: {}", show(out));
    let v = json_lines[0].clone();
    assert_eq!(v["result"], json!("refused"), "{v}");
    let why = v["reason"].as_str().unwrap_or_default();
    assert!(
        why.starts_with("the merge method could not be settled:"),
        "{why:?}"
    );
    assert_no_merge_attempt(fx);
    v
}

#[test]
fn merge_only_repository_merges_with_a_merge_commit() {
    let Some(fx) = Fx::new() else { return };
    repo_allows(&fx, true, false, false);
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let m = merge_flags(&fx);
    assert_eq!(method_flags(&m), ["--merge"], "{m:?}");
    assert!(m.contains(&"--subject".to_string()), "{m:?}");
    assert!(m.contains(&"--body-file".to_string()), "{m:?}");
    let body = std::fs::read_to_string(fx.fake.join("merge-body")).unwrap();
    assert!(body.starts_with("Description text."), "{body:?}");
    let v = last_json(&out, "result");
    assert_eq!(v["result"], json!("merged"));
    assert_eq!(v["merge_method"], json!("merge"), "{v}");
    assert!(
        !v["merge_method_reason"].as_str().unwrap_or("").is_empty(),
        "the reason is recorded: {v}"
    );
}

#[test]
fn squash_allowed_repository_squashes_when_no_method_is_given() {
    let Some(fx) = Fx::new() else { return };
    // All three allowed (the fixture default): squash stays the default.
    let out = fx.merge_tool_method(PR, SHA_A, "");
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let m = merge_flags(&fx);
    assert_eq!(method_flags(&m), ["--squash"], "{m:?}");
    assert!(m.contains(&"--subject".to_string()), "{m:?}");
    let v = last_json(&out, "result");
    assert_eq!(v["merge_method"], json!("squash"), "{v}");
    assert!(!v["merge_method_reason"].as_str().unwrap_or("").is_empty());
    assert_eq!(fx.api_calls().len(), 1, "the repository is read once");
}

#[test]
fn default_order_is_squash_then_merge_then_rebase() {
    let Some(fx) = Fx::new() else { return };
    repo_allows(&fx, true, false, true);
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(method_flags(&merge_flags(&fx)), ["--merge"]);
}

#[test]
fn rebase_only_repository_rebases_without_a_message() {
    let Some(fx) = Fx::new() else { return };
    repo_allows(&fx, false, false, true);
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let m = merge_flags(&fx);
    assert_eq!(method_flags(&m), ["--rebase"], "{m:?}");
    assert!(!m.contains(&"--subject".to_string()), "no subject: {m:?}");
    assert!(!m.contains(&"--body-file".to_string()), "no body: {m:?}");
    assert_eq!(last_json(&out, "result")["merge_method"], json!("rebase"));
}

#[test]
fn an_explicit_method_wins_where_squash_is_also_allowed() {
    let Some(fx) = Fx::new() else { return };
    let out = fx.merge_tool_method(PR, SHA_A, "merge");
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(method_flags(&merge_flags(&fx)), ["--merge"]);
    let v = last_json(&out, "result");
    assert_eq!(v["merge_method"], json!("merge"), "{v}");
    let why = v["merge_method_reason"].as_str().unwrap_or("");
    assert!(
        why.contains("merge"),
        "the reason names the method: {why:?}"
    );
}

#[test]
fn every_explicit_method_pins_the_head() {
    for method in ["merge", "squash", "rebase"] {
        let Some(fx) = Fx::new() else { return };
        let out = fx.merge_tool_method(PR, SHA_A, method);
        assert_eq!(out.status.code(), Some(0), "{method}: {}", show(&out));
        let m = merge_flags(&fx);
        assert_eq!(method_flags(&m), [format!("--{method}")], "{m:?}");
    }
}

#[test]
fn a_moved_head_is_still_refused_by_github_for_every_method() {
    for method in ["merge", "squash", "rebase"] {
        let Some(fx) = Fx::new() else { return };
        fx.set("server_head", SHA_B);
        let out = fx.merge_tool_method(PR, SHA_A, method);
        assert_eq!(out.status.code(), Some(4), "{method}: {}", show(&out));
        assert_eq!(last_json(&out, "result")["result"], json!("refused"));
        assert_eq!(fx.merge_calls().len(), 1, "{method}");
    }
}

#[test]
fn an_explicit_method_the_repository_disallows_is_never_used() {
    // The spec case, and its siblings: no other method takes its place.
    for (method, allow) in [
        ("rebase", (true, true, false)),
        ("squash", (true, false, true)),
        ("merge", (false, true, true)),
    ] {
        let Some(fx) = Fx::new() else { return };
        repo_allows(&fx, allow.0, allow.1, allow.2);
        let out = fx.merge_tool_method(PR, SHA_A, method);
        let v = assert_unsettled(&fx, &out);
        let why = v["reason"].as_str().unwrap();
        assert!(why.contains(method), "the reason names {method}: {why:?}");
    }
}

#[test]
fn a_repository_that_allows_no_method_is_handed_off_not_merged() {
    let Some(fx) = Fx::new() else { return };
    repo_allows(&fx, false, false, false);
    let out = fx.merge_tool(PR, SHA_A);
    assert_unsettled(&fx, &out);
}

#[test]
fn an_unreadable_repository_is_handed_off_with_the_exit_status() {
    let Some(fx) = Fx::new() else { return };
    fx.set("api_rc", "1");
    let out = fx.merge_tool(PR, SHA_A);
    let v = assert_unsettled(&fx, &out);
    assert!(
        v["reason"].as_str().unwrap().contains("exit 1"),
        "names the status: {v}"
    );
    // An explicit method does not skip the read: the repository decides.
    let out = fx.merge_tool_method(PR, SHA_A, "merge");
    assert_unsettled(&fx, &out);
}

/// #1888 review: a failed repository read says why, not only that it failed.
/// The first line of gh's error that is not blank follows the exit status in
/// the reason; gh's whole error still reaches the step's log.
#[test]
fn an_unreadable_repository_reason_carries_the_first_line_of_gh_error() {
    let Some(fx) = Fx::new() else { return };
    fx.set("api_rc", "1");
    fx.set(
        "api_err",
        "\n   \nHTTP 403: Resource not accessible by integration\nsecond line of detail\n",
    );
    let out = fx.merge_tool(PR, SHA_A);
    let v = assert_unsettled(&fx, &out);
    let why = v["reason"].as_str().unwrap();
    assert!(
        why.contains("(gh exit 1: HTTP 403: Resource not accessible by integration)"),
        "the status, then gh's first line: {why:?}"
    );
    assert!(!why.contains("second line"), "only the first line: {why:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("HTTP 403") && err.contains("second line of detail"),
        "gh's error still reaches the log: {err:?}"
    );
}

/// The gh error is text this tool did not write: it is made printable ASCII
/// and cut at 200 bytes before the reason repeats it, so no escape sequence,
/// control character or long page of output reaches the finalisation
/// statement or the hand-off comment. A read with no error text keeps the
/// status alone.
#[test]
fn a_gh_error_in_the_reason_is_cut_to_200_bytes_and_printable() {
    let Some(fx) = Fx::new() else { return };
    fx.set("api_rc", "1");
    let noisy = format!(
        "\u{1b}[31mHTTP 500\u{7} caf\u{e9} {}\r\nsecond line\n",
        "x".repeat(500)
    );
    fx.set("api_err", &noisy);
    let out = fx.merge_tool(PR, SHA_A);
    let v = assert_unsettled(&fx, &out);
    let why = v["reason"].as_str().unwrap();
    assert!(
        why.chars().all(|c| c.is_ascii() && !c.is_control()),
        "{why:?}"
    );
    let shown = why
        .split_once("(gh exit 1: ")
        .map(|(_, rest)| rest.strip_suffix(')').unwrap_or(rest))
        .unwrap_or_else(|| panic!("the status, then gh's text: {why:?}"));
    assert!(
        shown.len() <= 200,
        "cut at 200 bytes: {} {shown:?}",
        shown.len()
    );
    assert!(shown.contains("HTTP 500"), "{shown:?}");
    assert!(
        shown.ends_with('x'),
        "the long line is cut, not dropped: {shown:?}"
    );
    assert!(!why.contains("second line"), "{why:?}");

    let Some(fx) = Fx::new() else { return };
    fx.set("api_rc", "1");
    fx.set("api_err", "");
    let out = fx.merge_tool(PR, SHA_A);
    let v = assert_unsettled(&fx, &out);
    assert!(
        v["reason"].as_str().unwrap().ends_with("(gh exit 1)"),
        "no text, no colon: {v}"
    );
}

#[test]
fn an_unparseable_repository_reading_is_handed_off() {
    for raw in [
        "",
        "\n",
        "true\ttrue\n",
        "true\ttrue\ttrue\ttrue\n",
        "yes\tno\tmaybe\n",
        "null\tnull\tnull\n",
        "<html>not json</html>\n",
        "True\tTrue\tTrue\n",
    ] {
        let Some(fx) = Fx::new() else { return };
        fx.set("api_out", raw);
        for method in ["", "squash"] {
            let out = fx.merge_tool_method(PR, SHA_A, method);
            assert_unsettled(&fx, &out);
        }
    }
}

#[test]
fn gh_exit_79_on_the_repository_read_passes_through() {
    let Some(fx) = Fx::new() else { return };
    fx.set("api_rc", "79");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(79), "{}", show(&out));
    assert_no_merge_attempt(&fx);
    let calls = fx.gh_calls();
    assert_eq!(
        calls.last().and_then(|c| c.first()).map(String::as_str),
        Some("api"),
        "nothing runs after a 79: {calls:?}"
    );
}

#[test]
fn hostile_method_text_is_refused_and_never_reaches_argv() {
    // A path no shell of this test ever writes: it exists only if the method
    // text was executed rather than matched.
    let marker_dir = tempfile::tempdir().expect("temporary folder");
    let marker = marker_dir.path().join("ran");
    let hostile = [
        format!("merge\"; touch {} #", marker.display()),
        format!("$(touch {})", marker.display()),
        format!("`touch {}`", marker.display()),
        "merge\nsquash".to_string(),
        "merge ".to_string(),
        " merge".to_string(),
        "Merge".to_string(),
        "SQUASH".to_string(),
        "--admin".to_string(),
        "merge --admin".to_string(),
        "auto".to_string(),
        "'; echo x".to_string(),
    ];
    for method in hostile {
        let Some(fx) = Fx::new() else { return };
        let out = fx.merge_tool_method(PR, SHA_A, &method);
        assert_eq!(out.status.code(), Some(4), "{method:?}: {}", show(&out));
        let v = assert_unsettled(&fx, &out);
        assert_eq!(v["pr"], json!(2575));
        assert_eq!(v["expected_head"], json!(SHA_A));
        assert!(
            !fx.gh_calls()
                .iter()
                .flatten()
                .any(|a| a.contains("--admin")),
            "{method:?}"
        );
        assert!(
            fx.api_calls().is_empty(),
            "{method:?}: refused before the repository read: {:?}",
            fx.gh_calls()
        );
    }
    assert!(!marker.exists(), "the method text was executed");
}

#[test]
fn an_echoed_method_is_cut_to_32_characters_and_printable() {
    let Some(fx) = Fx::new() else { return };
    let long = "a".repeat(200);
    let out = fx.merge_tool_method(PR, SHA_A, &long);
    let v = assert_unsettled(&fx, &out);
    let why = v["reason"].as_str().unwrap();
    assert!(!why.contains(&"a".repeat(33)), "cut at 32: {why:?}");
    assert!(why.contains("aaaa"), "the cut text is still named: {why:?}");
    // A control character or non-ASCII byte is replaced, not echoed.
    let out = fx.merge_tool_method(PR, SHA_A, "me\u{7}rg\u{e9}\u{1b}[31m");
    let v = assert_unsettled(&fx, &out);
    let why = v["reason"].as_str().unwrap();
    assert!(
        why.chars().all(|c| !c.is_control() && c.is_ascii()),
        "{why:?}"
    );
}

#[test]
fn the_repository_read_is_a_read_of_the_repository_and_nothing_else() {
    let Some(fx) = Fx::new() else { return };
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    let api = fx.api_calls();
    assert_eq!(api.len(), 1, "{api:?}");
    let line = api[0].join(" ");
    assert!(line.contains("repos/{owner}/{repo}"), "{line}");
    assert!(!line.contains("/merge"), "no merge through the API: {line}");
    for flag in ["--method", "-X", "--input", "-f", "-F", "--field"] {
        assert!(!api[0].contains(&flag.to_string()), "{flag}: {line}");
    }
    // Between the message helper and the merge: after the last checks call,
    // before the merge request.
    let calls = fx.gh_calls();
    let pos = |k: &str, n: &str| {
        calls.iter().rposition(|c| {
            c.first().map(String::as_str) == Some(k) && c.get(1).map(String::as_str) == Some(n)
        })
    };
    let checks = pos("pr", "checks").unwrap();
    let merge = pos("pr", "merge").unwrap();
    let api_at = calls
        .iter()
        .position(|c| c.first().map(String::as_str) == Some("api"))
        .unwrap();
    assert!(checks < api_at && api_at < merge, "{calls:?}");
}

#[test]
fn paths_that_never_reach_a_merge_make_no_repository_call() {
    // Not open, failing checks, pending checks and DIRTY end before the read.
    let cases: [(&str, &str); 4] = [
        ("state", "CLOSED"),
        ("required", "fail"),
        ("required", "pending"),
        ("merge_state", "DIRTY"),
    ];
    for (name, value) in cases {
        let Some(fx) = Fx::new() else { return };
        fx.set(name, value);
        let out = fx.merge_tool_method(PR, SHA_A, "merge");
        assert_ne!(out.status.code(), Some(0), "{name}={value}: {}", show(&out));
        assert!(
            fx.api_calls().is_empty(),
            "{name}={value}: {:?}",
            fx.gh_calls()
        );
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn the_method_is_recorded_when_github_refuses_the_merge() {
    let Some(fx) = Fx::new() else { return };
    repo_allows(&fx, true, false, false);
    fx.set("merge_rc", "1");
    let out = fx.merge_tool(PR, SHA_A);
    assert_eq!(out.status.code(), Some(4), "{}", show(&out));
    let v = last_json(&out, "result");
    assert_eq!(v["result"], json!("refused"));
    assert_eq!(v["merge_method"], json!("merge"), "{v}");
    assert!(!v["merge_method_reason"].as_str().unwrap_or("").is_empty());
    assert_eq!(method_flags(&merge_flags(&fx)), ["--merge"]);
}

#[test]
fn a_fourth_argument_is_a_usage_error_without_calling_gh() {
    let Some(fx) = Fx::new() else { return };
    let mut c = fx.bash();
    c.arg("-c")
        .arg(
            "set -u\n. \"$AMPLIHACK_HOME/amplifier-bundle/tools/autodrive_merge.sh\" || exit 90\n\
             autodrive_merge \"$1\" \"$2\" merge extra",
        )
        .arg("bash")
        .args([PR, SHA_A]);
    let out = feed(c, b"");
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    assert!(fx.gh_calls().is_empty(), "{:?}", fx.gh_calls());
}

#[test]
fn the_message_helper_still_gates_every_method() {
    // Exit 6 stays reachable whichever method the repository settles on.
    for (m, s, r) in [
        (true, true, true),
        (true, false, false),
        (false, false, true),
    ] {
        let Some(fx) = Fx::new() else { return };
        repo_allows(&fx, m, s, r);
        fx.set("title", "");
        let out = fx.merge_tool(PR, SHA_A);
        assert_eq!(out.status.code(), Some(6), "{}", show(&out));
        assert!(
            fx.api_calls().is_empty(),
            "no read before the message: {:?}",
            fx.gh_calls()
        );
        assert_no_merge_attempt(&fx);
    }
}

// ===========================================================================
// The goal loop's merge action: authorisation, the head this turn read, then
// GitHub
//
// #3129 replaced the finalize `merge` step with the goal loop's merge action,
// `autodrive_pr_goal.sh act merge <turn_dir>`. It merges only when the head
// the agent named (`args.head` in decision.json) is the head this turn read
// (`pr.head` in goal.json), and only under the captured authorisation; then
// autodrive_merge.sh asks GitHub with --match-head-commit. The outcome goes to
// the turn's result.json and the merge tool's result, bound to the run, to
// merge-result.json. Exit 79 and 75 pass up; any other failure is an outcome
// for the next turn, never a hand-off (merge-result.json always says
// `"handoff": "false"`).
// ===========================================================================

fn merge_action_fixture() -> Option<(Fx, String)> {
    let fx = Fx::new()?;
    let head = fx.git_repo();
    fx.set("server_head", &head);
    fx.set(
        "auth_out",
        &json!({"allows_merge": true, "run_id": RUN_ID, "pr_selected": true}).to_string(),
    );
    Some((fx, head))
}

/// One turn's merge action: what the tool did, and the turn folder it wrote.
struct MergeTurn {
    out: Output,
    dir: PathBuf,
}

impl MergeTurn {
    /// The turn's `result.json`: the outcome the next turn's reading carries.
    fn result(&self) -> Value {
        let text = std::fs::read_to_string(self.dir.join("result.json"))
            .unwrap_or_else(|e| panic!("no result.json ({e}): {}", show(&self.out)));
        serde_json::from_str(text.trim()).expect("result.json is one JSON object")
    }

    fn outcome(&self) -> Value {
        self.result()["outcome"].clone()
    }
}

/// `merge-result.json` in the state folder: the merge tool's result line,
/// bound to the run.
fn merge_record(fx: &Fx) -> Value {
    let text = std::fs::read_to_string(fx.state.join("merge-result.json"))
        .expect("the merge action writes merge-result.json");
    serde_json::from_str(text.trim()).expect("one JSON object")
}

/// `act merge` in a new turn folder, where the agent named `named` as the
/// head (`None`: no `args.head`) and the turn read `read` as the PR's head
/// (`None`: no `pr.head`), with extra `RECIPE_VAR_*` values from the run's
/// context.
fn merge_action(
    fx: &Fx,
    named: Option<&str>,
    read: Option<&str>,
    vars: &[(&str, &str)],
) -> MergeTurn {
    let turns = fx.state.join("turns");
    private_dir(&turns);
    let n = (1..)
        .find(|n| !turns.join(format!("{n:03}")).exists())
        .unwrap();
    let dir = turns.join(format!("{n:03}"));
    private_dir(&dir);
    let args = named.map_or_else(|| json!({}), |h| json!({ "head": h }));
    private_file(
        &dir.join("decision.json"),
        &json!({"action": "merge", "args": args}).to_string(),
    );
    let pr = read.map_or_else(|| json!({}), |h| json!({ "head": h }));
    private_file(
        &dir.join("goal.json"),
        &json!({"schema": 1, "summary": "the fixture's reading", "pr": pr}).to_string(),
    );
    let mut c = fx.bash();
    c.arg(fx.tools().join("autodrive_pr_goal.sh"))
        .args(["act", "merge"])
        .arg(&dir)
        .env("AUTODRIVE_GOAL_CONTRACT", "1")
        .env("AUTODRIVE_STATE_DIR", &fx.state)
        .env("AUTODRIVE_REPO", &fx.repo)
        .env("AUTODRIVE_WORKTREE", &fx.repo)
        .env("AUTODRIVE_LOOP_HOME", fx.tools())
        .env(
            "AUTODRIVE_INVOCATION_FILE",
            fx.state.join("invocation.json"),
        )
        .env("RECIPE_VAR_pr_number", PR)
        .current_dir(&fx.repo)
        .stdin(Stdio::null());
    for (k, v) in vars {
        c.env(format!("RECIPE_VAR_{k}"), v);
    }
    let out = c.output().expect("run the merge action");
    MergeTurn { out, dir }
}

/// The merge action at the head both the agent and the reading name.
fn merge_at(fx: &Fx, head: &str, vars: &[(&str, &str)]) -> MergeTurn {
    merge_action(fx, Some(head), Some(head), vars)
}

#[test]
fn merge_action_merges_on_captured_authorisation_read_head_and_green_checks() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(turn.outcome(), json!("merged"), "{}", turn.result());
    let v = merge_record(&fx);
    assert_eq!(v["result"], json!("merged"), "{v}");
    assert_eq!(v["expected_head"], json!(head));
    assert_eq!(v["handoff"], json!("false"));
    let m = &fx.merge_calls()[0];
    let i = m.iter().position(|a| a == "--match-head-commit").unwrap();
    assert_eq!(m[i + 1], head);
    let auth_calls = fx.log("amplihack.log");
    let auth = auth_calls
        .iter()
        .find(|c| {
            c.join(" ")
                .starts_with("orch helper autodrive authorization")
        })
        .expect("the captured invocation is consulted");
    assert!(
        auth.contains(&fx.state.join("invocation.json").display().to_string()),
        "{auth:?}"
    );
    assert!(auth.contains(&fx.state.display().to_string()), "{auth:?}");
}

#[test]
fn merge_refused_without_captured_authorization() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    // The helper fails (record missing, wrong directory): exit 2, no stdout.
    // The action reports it and merges nothing.
    std::fs::remove_file(fx.fake.join("auth_out")).unwrap();
    fx.set("auth_rc", "2");
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(
        turn.outcome(),
        json!("authorization-unreadable"),
        "{}",
        turn.result()
    );
    assert_no_merge_attempt(&fx);

    // Exit 0 with no object holding a boolean allows_merge grants nothing.
    // The action reads anything but a true allows_merge as no permission, so
    // the outcome is not-authorised.
    fx.set("auth_rc", "0");
    fx.set("auth_out", r#"{"allows_merge": "yes"}"#);
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(turn.outcome(), json!("not-authorised"), "{}", turn.result());
    assert_no_merge_attempt(&fx);

    // The captured invocation does not allow a merge.
    fx.set(
        "auth_out",
        &json!({"allows_merge": false, "run_id": RUN_ID, "pr_selected": true}).to_string(),
    );
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.outcome(), json!("not-authorised"), "{}", turn.result());
    let v = merge_record(&fx);
    assert_eq!(v["result"], json!("not-authorised"), "{v}");
    assert_eq!(v["handoff"], json!("false"), "{v}");
    assert_no_merge_attempt(&fx);
}

/// Exit 79 from the authorisation read stays terminal: it is a refusal that
/// passes up, never an outcome for another turn.
#[test]
fn the_authorization_helpers_exit_79_passes_up() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    std::fs::remove_file(fx.fake.join("auth_out")).unwrap();
    fx.set("auth_rc", "79");
    let turn = merge_at(&fx, &head, &[]);
    assert_no_merge_attempt(&fx);
    assert_eq!(turn.out.status.code(), Some(79), "{}", show(&turn.out));
}

#[test]
fn merge_refused_for_a_pr_other_than_the_selected_one() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    // The helper says the PR is not the one the operator selected, or (an
    // older helper) says nothing about the selection: never merged. The
    // action reports both as not-authorised.
    for auth in [
        json!({"allows_merge": true, "run_id": RUN_ID, "pr_selected": false}),
        json!({"allows_merge": true, "run_id": RUN_ID}),
    ] {
        fx.set("auth_out", &auth.to_string());
        let turn = merge_at(&fx, &head, &[]);
        assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
        assert_eq!(turn.outcome(), json!("not-authorised"), "{}", turn.result());
        let v = merge_record(&fx);
        assert_eq!(v["result"], json!("not-authorised"), "{v}");
        assert_no_merge_attempt(&fx);
    }
    let auth = fx
        .log("amplihack.log")
        .into_iter()
        .find(|c| {
            c.join(" ")
                .starts_with("orch helper autodrive authorization")
        })
        .expect("the captured invocation is consulted");
    let i = auth
        .iter()
        .position(|a| a == "--pr")
        .expect("--pr is passed");
    assert_eq!(auth[i + 1], PR);
}

#[test]
fn recipe_context_can_narrow_but_never_grant_merge_permission() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    for (no_merge, dry_run) in [
        ("true", "false"),
        ("false", "true"),
        ("", "false"),
        ("FALSE", "false"),
        ("false", ""),
    ] {
        let turn = merge_at(
            &fx,
            &head,
            &[("no_merge", no_merge), ("autodrive_dry_run", dry_run)],
        );
        assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
        assert_eq!(
            turn.outcome(),
            json!("not-authorised"),
            "no_merge={no_merge:?} dry_run={dry_run:?}"
        );
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn head_moved_after_the_reading_is_not_merged() {
    let Some((fx, assessed)) = merge_action_fixture() else {
        return;
    };
    let moved = fx.commit("committed after the assessment");
    assert_ne!(moved, assessed);
    fx.set("server_head", &moved);
    // The agent names the head it assessed; this turn's reading has moved on.
    let turn = merge_action(&fx, Some(&assessed), Some(&moved), &[]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(turn.outcome(), json!("head-mismatch"), "{}", turn.result());
    assert!(
        fx.gh_calls().is_empty(),
        "a moved head is refused without calling gh: {:?}",
        fx.gh_calls()
    );
}

#[test]
fn missing_or_malformed_assessed_head_never_merges() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    let upper = head.to_uppercase();
    // The head the agent names, then the head the turn read: missing, not a
    // SHA, or empty.
    for (named, read) in [
        (None, Some(head.as_str())),
        (Some("not-a-sha"), Some(head.as_str())),
        (Some(""), Some(head.as_str())),
        (Some(upper.as_str()), Some(head.as_str())),
        (Some(head.as_str()), None),
        (Some(head.as_str()), Some("not-a-sha")),
        (Some(head.as_str()), Some("")),
    ] {
        let turn = merge_action(&fx, named, read, &[]);
        assert_ne!(
            turn.outcome(),
            json!("merged"),
            "{named:?} {read:?}: {}",
            turn.result()
        );
        assert_no_merge_attempt(&fx);
    }
}

#[test]
fn failing_checks_at_the_merge_action_are_not_merged() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    fx.set("required", "fail");
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(turn.outcome(), json!("checks-not-green"));
    assert_eq!(merge_record(&fx)["result"], json!("checks-not-green"));
    assert_no_merge_attempt(&fx);
}

#[test]
fn github_refusal_at_the_merge_action_is_refused() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    fx.set("server_head", SHA_B); // moved on the server only
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(turn.outcome(), json!("refused"));
    assert_eq!(merge_record(&fx)["result"], json!("refused"));
}

#[test]
fn merge_action_passes_the_given_method_and_reports_it() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    let turn = merge_at(&fx, &head, &[("merge_method", "merge")]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(turn.outcome(), json!("merged"), "{}", turn.result());
    let v = merge_record(&fx);
    assert_eq!(v["result"], json!("merged"), "{v}");
    assert_eq!(v["merge_method"], json!("merge"), "{v}");
    assert!(!v["merge_method_reason"].as_str().unwrap_or("").is_empty());
    let m = &fx.merge_calls()[0];
    assert!(m.contains(&"--merge".to_string()), "{m:?}");
    assert!(!m.contains(&"--squash".to_string()), "{m:?}");
    let i = m.iter().position(|a| a == "--match-head-commit").unwrap();
    assert_eq!(m[i + 1], head);
}

#[test]
fn merge_action_without_a_method_follows_the_repository() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    repo_allows(&fx, true, false, false);
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(merge_record(&fx)["merge_method"], json!("merge"));
    assert!(fx.merge_calls()[0].contains(&"--merge".to_string()));
}

#[test]
fn merge_action_refuses_an_unsettled_method_with_what_was_found() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    repo_allows(&fx, true, true, false);
    let turn = merge_at(&fx, &head, &[("merge_method", "rebase")]);
    assert_eq!(turn.out.status.code(), Some(0), "{}", show(&turn.out));
    assert_eq!(turn.outcome(), json!("refused"), "{}", turn.result());
    let v = merge_record(&fx);
    assert_eq!(v["result"], json!("refused"), "{v}");
    assert!(
        v["reason"]
            .as_str()
            .unwrap_or("")
            .starts_with("the merge method could not be settled:"),
        "{v}"
    );
    assert_no_merge_attempt(&fx);
}

#[test]
fn gh_exit_79_passes_through_merge_step() {
    let Some((fx, head)) = merge_action_fixture() else {
        return;
    };
    fx.set("rc79_on", "checks");
    let turn = merge_at(&fx, &head, &[]);
    assert_eq!(turn.out.status.code(), Some(79), "{}", show(&turn.out));
    assert_no_merge_attempt(&fx);
}

// ===========================================================================
// `amplihack orch helper autodrive authorization` (real binary)
// ===========================================================================

fn write_invocation(dir: &Path, state_dir: &Path, no_merge: bool, dry_run: bool) -> PathBuf {
    private_dir(dir);
    let path = dir.join("invocation.json");
    let action = if no_merge || dry_run {
        "verify_readiness"
    } else {
        "merge"
    };
    private_file(
        &path,
        &json!({
            "schema_version": 1,
            "selected_pr": 2575,
            "selected_url": null,
            "selected_branch": null,
            "policy_provenance": PROVENANCE,
            "run_id": RUN_ID,
            "no_merge": no_merge,
            "dry_run": dry_run,
            "requested_action": action,
            "state_dir": state_dir,
        })
        .to_string(),
    );
    path
}

fn authorization_helper(fx: &Fx, invocation: &Path, directory: &Path) -> Output {
    authorization_helper_for(fx, invocation, directory, None)
}

fn authorization_helper_for(
    fx: &Fx,
    invocation: &Path,
    directory: &Path,
    pr: Option<&str>,
) -> Output {
    let mut c = fx
        .env
        .command(crate::support::amplihack_bin::amplihack_bin());
    c.args([
        "orch",
        "helper",
        "autodrive",
        "authorization",
        "--invocation",
    ])
    .arg(invocation)
    .arg("--directory")
    .arg(directory)
    .args(pr.map(|p| ["--pr", p]).into_iter().flatten())
    .env("AMPLIHACK_HOME", &fx.ah)
    .current_dir(&fx.root)
    .stdin(Stdio::null());
    c.output().expect("run amplihack authorization helper")
}

#[test]
fn authorization_helper_reports_captured_merge_permission() {
    let Some(fx) = Fx::new() else { return };
    for (no_merge, dry_run, allows) in [
        (false, false, true),
        (true, false, false),
        (false, true, false),
    ] {
        let inv = write_invocation(
            &fx.root.join(format!("inv-{no_merge}-{dry_run}")),
            &fx.state,
            no_merge,
            dry_run,
        );
        let out = authorization_helper(&fx, &inv, &fx.state);
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        assert_eq!(
            last_json(&out, "allows_merge"),
            // #3129: the answer also carries the captured flags behind
            // allows_merge.
            json!({
                "allows_merge": allows,
                "no_merge": no_merge,
                "dry_run": dry_run,
                "run_id": RUN_ID
            })
        );
    }
}

#[test]
fn authorization_helper_reports_whether_the_pr_was_selected() {
    let Some(fx) = Fx::new() else { return };
    // write_invocation selects PR 2575.
    let inv = write_invocation(&fx.root.join("inv"), &fx.state, false, false);
    for (pr, selected) in [("2575", true), ("2576", false)] {
        let out = authorization_helper_for(&fx, &inv, &fx.state, Some(pr));
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        assert_eq!(
            last_json(&out, "allows_merge"),
            json!({
                "allows_merge": true,
                "no_merge": false,
                "dry_run": false,
                "run_id": RUN_ID,
                "pr_selected": selected
            })
        );
    }
    // A selection by URL binds the merge too.
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&inv).unwrap()).unwrap();
    doc["selected_pr"] = Value::Null;
    doc["selected_url"] = json!("https://github.com/o/r/pull/2575");
    private_file(&inv, &doc.to_string());
    for (pr, selected) in [("2575", true), ("25", false)] {
        let out = authorization_helper_for(&fx, &inv, &fx.state, Some(pr));
        assert_eq!(out.status.code(), Some(0), "{}", show(&out));
        assert_eq!(
            last_json(&out, "pr_selected")["pr_selected"],
            json!(selected)
        );
    }
}

#[test]
fn authorization_helper_refuses_unbound_or_unreadable_invocations() {
    let Some(fx) = Fx::new() else { return };
    let inv = write_invocation(&fx.root.join("inv"), &fx.state, false, false);
    let other = fx.root.join("other-state");
    private_dir(&other);
    for (invocation, directory) in [
        (inv.clone(), other.clone()),
        (fx.root.join("inv/missing.json"), fx.state.clone()),
    ] {
        let out = authorization_helper(&fx, &invocation, &directory);
        assert_eq!(out.status.code(), Some(2), "{}", show(&out));
        assert!(
            stdout(&out).trim().is_empty(),
            "nothing on stdout: {}",
            show(&out)
        );
    }
    // A world-writable invocation is not private evidence and is never
    // repaired (#2546).
    std::fs::set_permissions(&inv, std::fs::Permissions::from_mode(0o666)).unwrap();
    let out = authorization_helper(&fx, &inv, &fx.state);
    assert_eq!(out.status.code(), Some(2), "{}", show(&out));
    // #2546: this user's group-readable file in a private folder is repaired
    // to 0600 and read, not refused.
    std::fs::set_permissions(&inv, std::fs::Permissions::from_mode(0o644)).unwrap();
    let out = authorization_helper(&fx, &inv, &fx.state);
    assert_eq!(out.status.code(), Some(0), "{}", show(&out));
    assert_eq!(
        std::fs::metadata(&inv).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

// #2575 B1, B2 and M1 (Crusty's review of PR #2637): pending checks wait,
// a missing Crusty record runs Crusty, the base sync, and success means
// merged. A child module, so it shares the fakes and fixtures above.
#[path = "issue_2575_wait_sync_success.rs"]
mod wait_sync_success;
