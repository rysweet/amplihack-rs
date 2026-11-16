use super::{private_state::State, secure_fs};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub const LIMIT: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub record_hash: String,
    pub sidecars: BTreeMap<String, Option<Vec<u8>>>,
}

pub fn blob_hash(dir: &Path, bytes: &[u8]) -> Result<String> {
    let mut child = amplihack_git::command_in(dir)
        .args(["hash-object", "--no-filters", "--stdin"])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("git stdin missing")?
        .write_all(bytes)?;
    let o = child.wait_with_output()?;
    ensure!(o.status.success(), "could not hash round evidence");
    let hash = String::from_utf8(o.stdout)?.trim().to_string();
    ensure!(
        [40, 64].contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid evidence hash"
    );
    Ok(hash)
}

pub fn review_stamp(tools: &Path, dir: &Path) -> Result<String> {
    let o = Command::new("bash")
        .arg(tools.join("autodrive_worktree_guard.sh"))
        .args(["index-stamp"])
        .arg(dir)
        .output()?;
    ensure!(o.status.success(), "cannot stamp trusted review directory");
    Ok(String::from_utf8(o.stdout)?)
}

pub fn capture(state: &State) -> Result<Snapshot> {
    let bytes = secure_fs::read(secure_fs::Scope::of(&state.state_dir), &state.record, LIMIT)?;
    let mut sidecars = BTreeMap::new();
    for ext in ["findings", "minor", "severity"] {
        let path = state.record.with_extension(format!("json.{ext}"));
        let value = match std::fs::symlink_metadata(&path) {
            Ok(_) => Some(secure_fs::read(
                secure_fs::Scope::of(&state.state_dir),
                &path,
                LIMIT,
            )?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        sidecars.insert(ext.to_string(), value);
    }
    ensure!(sidecars["findings"].is_some(), "missing findings sidecar");
    Ok(Snapshot {
        record_hash: blob_hash(&state.state_dir, &bytes)?,
        sidecars,
    })
}

pub fn validate(state: &State, expected: &Snapshot) -> Result<serde_json::Value> {
    let actual = capture(state)?;
    ensure!(
        actual.record_hash == expected.record_hash && actual.sidecars == expected.sidecars,
        "round record or sidecar evidence changed"
    );
    let bytes = secure_fs::read(secure_fs::Scope::of(&state.state_dir), &state.record, LIMIT)?;
    ensure!(
        bytes
            == secure_fs::read(
                secure_fs::Scope::of(&state.state_dir),
                &state.state_dir.join("crusty-latest.json"),
                LIMIT
            )?,
        "latest round copy differs"
    );
    ensure!(
        actual.sidecars["findings"].as_ref()
            == Some(&secure_fs::read(
                secure_fs::Scope::of(&state.state_dir),
                &state.state_dir.join("crusty-latest.json.findings"),
                LIMIT
            )?),
        "latest findings copy differs"
    );
    let manifest = String::from_utf8(secure_fs::read(
        secure_fs::Scope::of(&state.state_dir),
        &state.state_dir.join("crusty-records.tsv"),
        LIMIT,
    )?)?;
    let expected_row = format!(
        "{}\t{}\t{}",
        state.round_label,
        state
            .record
            .file_name()
            .context("missing record name")?
            .to_string_lossy(),
        actual.record_hash
    );
    ensure!(
        manifest.lines().rfind(|l| !l.trim().is_empty()) == Some(expected_row.as_str()),
        "manifest does not bind this round"
    );
    ensure!(
        review_stamp(&state.launch_tools, &state.review_dir)? == state.review_stamp,
        "trusted review directory changed"
    );
    let record: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        record["round_label"].as_str() == Some(&state.round_label),
        "record round identity mismatch"
    );
    let head = amplihack_git::command_in(&state.repo)
        .args(["rev-parse", "HEAD"])
        .output()?;
    ensure!(head.status.success(), "cannot read current PR head");
    let head = String::from_utf8(head.stdout)?;
    ensure!(
        record["head_sha"].as_str() == Some(head.trim()),
        "record does not describe current head"
    );
    if matches!(
        record["crusty_verdict"].as_str(),
        Some("CLEAN" | "MINOR_ONLY")
    ) {
        ensure!(
            record["reviewed_head_sha"].as_str() == Some(head.trim()),
            "completion must cover the current reviewed head"
        );
        ensure!(
            actual.sidecars["findings"]
                .as_ref()
                .is_some_and(|v| v.iter().all(u8::is_ascii_whitespace)),
            "completed record contains unresolved findings"
        );
        if record["crusty_verdict"] == "MINOR_ONLY" {
            ensure!(
                actual.sidecars["minor"]
                    .as_ref()
                    .is_some_and(|v| !v.is_empty())
                    && actual.sidecars["severity"].is_some(),
                "minor-only evidence missing"
            );
        }
    }
    Ok(record)
}

/// A full commit SHA: 40 (SHA-1) or 64 (SHA-256) lowercase hex characters.
/// Both sides of the head comparison pass this before they are compared, so
/// an unexpected answer from either route errors rather than reading as a
/// difference (#2333 S8).
fn valid_sha(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A branch name under `git check-ref-format --branch` rules, never starting
/// with `-` so it can never read as an option to `ls-remote` (#2333 S6).
fn valid_branch(s: &str) -> bool {
    !(s.is_empty()
        || s.len() > 255
        || s == "@"
        || s.starts_with(['-', '/'])
        || s.ends_with(['/', '.'])
        || s.contains("..")
        || s.contains("//")
        || s.contains("@{")
        || s.bytes()
            .any(|b| b < 0x21 || b == 0x7f || b"~^:?*[\\".contains(&b)))
}

/// One path segment of a GitHub owner or repository name.
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && !s.starts_with(['-', '.'])
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
}

/// `owner/repo` from `origin`, accepted only in the two canonical GitHub
/// shapes: `https://github.com/<owner>/<repo>(.git)?` and
/// `git@github.com:<owner>/<repo>(.git)?`.
///
/// Issue #2333 F1: `git ls-remote origin` would take its answer from a
/// configuration file the round's own agent has been writing to, which makes
/// a match forgeable and can offer the credential to a chosen host. The pair
/// is verified here and then used as a literal argument on both routes, so
/// nothing in the worktree's configuration can redirect either read.
fn origin_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    (valid_name(owner) && valid_name(repo)).then(|| (owner.to_string(), repo.to_string()))
}

/// Why a head read did not answer. The variants are a closed set and each
/// prints one program-generated token (#2333 S10): no exit status, no `git`
/// or `gh` message, no branch name, check name or URL may reach the dispatch
/// line or the evaluator's context, because every one of those is written by
/// someone who can open a pull request or name a workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// No route answered.
    ReadFailed,
    /// `origin` is not a GitHub remote in an accepted shape, so no read was made.
    OriginNotGithub,
    /// No PR number in the state, and none resolvable from the worktree.
    PrNumberUnresolved,
    /// The ref resolved to more than one line; no line is picked from several.
    AmbiguousRef,
    /// A route answered with something that is not a commit SHA.
    BadSha,
}

impl Unavailable {
    pub fn token(self) -> &'static str {
        match self {
            Self::ReadFailed => "read-failed",
            Self::OriginNotGithub => "origin-not-github",
            Self::PrNumberUnresolved => "pr-number-unresolved",
            Self::AmbiguousRef => "ambiguous-ref",
            Self::BadSha => "bad-sha",
        }
    }
}

/// The PR head on GitHub against the reviewed local head (#2333). Three
/// outcomes, kept apart, because the single folded check could not tell them
/// apart and threw away clean rounds whose only fault was a spent GraphQL
/// allowance: a difference fails closed, while a read that did not answer
/// leaves the head unconfirmed and is judged by the loop-health evaluator.
pub enum HeadCheck {
    Matches,
    Differs { local: String, remote: String },
    Unavailable(Unavailable),
}

/// What one route returned.
enum Route {
    Answer(String),
    /// The route did not answer; the next one may.
    NoAnswer,
    /// The route answered something that is not usable as a head SHA. The
    /// value is not accepted and no later route can correct it, so this is
    /// terminal for the route chain.
    Unusable(Unavailable),
}

/// `git ls-remote <canonical url> <ref>`, route one.
///
/// Run from outside the PR worktree with the system and global configuration
/// neutralised (#2333 S5), so `url.<base>.insteadOf` and `credential.helper`
/// written into the worktree cannot redirect the read or hand the credential
/// elsewhere. Prompts are off on both the git and the SSH side (S16): a
/// remote that asks for a credential must fail rather than stall the loop,
/// which is the failure this change exists to remove.
fn ls_remote(url: &str, refname: &str) -> Route {
    // Outside any repository, so no worktree's configuration is discovered.
    let o = amplihack_git::command_in(std::env::temp_dir())
        .args(["ls-remote", url, refname])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .stdin(Stdio::null())
        .output();
    let Ok(o) = o else {
        return Route::NoAnswer;
    };
    if !o.status.success() {
        return Route::NoAnswer;
    }
    let text = String::from_utf8_lossy(&o.stdout);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    match lines.as_slice() {
        // The ref does not exist here; the REST route may still answer.
        [] => Route::NoAnswer,
        [one] => match one.split_once('\t') {
            Some((sha, _)) if valid_sha(sha) => Route::Answer(sha.to_string()),
            _ => Route::Unusable(Unavailable::BadSha),
        },
        // Several lines for one ref: never a pick of the first (S9).
        _ => Route::Unusable(Unavailable::AmbiguousRef),
    }
}

/// `gh api repos/<owner>/<repo>/pulls/<n> --jq .head.sha`, route two.
///
/// REST, so it answers while the account's GraphQL allowance is spent. The
/// verified owner and repository are passed literally and `GH_REPO` is
/// removed, so `gh` never resolves them from the worktree's remotes (S3).
/// `GH_HOST` is removed too: the origin has already been checked as
/// github.com, so any other host is wrong by construction and the credential
/// must not be offered to it. `GH_CONFIG_DIR` is left alone, because the
/// loop's own `gh` authentication may legitimately use it.
fn rest_head(owner: &str, repo: &str, pr: u64) -> Route {
    let o = Command::new("gh")
        .args([
            "api",
            &format!("repos/{owner}/{repo}/pulls/{pr}"),
            "--jq",
            ".head.sha",
        ])
        .env("GH_PROMPT_DISABLED", "1")
        .env_remove("GH_REPO")
        .env_remove("GH_HOST")
        .stdin(Stdio::null())
        .output();
    let Ok(o) = o else {
        return Route::NoAnswer;
    };
    if !o.status.success() {
        return Route::NoAnswer;
    }
    let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
    if sha.is_empty() {
        return Route::NoAnswer;
    }
    if valid_sha(&sha) {
        Route::Answer(sha)
    } else {
        Route::Unusable(Unavailable::BadSha)
    }
}

/// Reads the PR head through `git ls-remote`, then REST, and compares it with
/// the reviewed local head. Neither route uses GraphQL, so a spent allowance
/// no longer makes a clean round unreadable.
///
/// The two reads are alternative routes to one fact, not a retry schedule:
/// neither is repeated and no delay is introduced.
///
/// Run only after [`validate`] has accepted the record, and only for a
/// completion claim, so a read fault can never turn an invalid record into a
/// wait (#2333 F2).
pub fn head_check(state: &State) -> Result<HeadCheck> {
    let local = amplihack_git::command_in(&state.repo)
        .args(["rev-parse", "HEAD"])
        .output()?;
    ensure!(local.status.success(), "cannot read current PR head");
    let local = String::from_utf8(local.stdout)?.trim().to_string();
    ensure!(valid_sha(&local), "current PR head is not a commit SHA");

    // F1: the remote is verified before it is read, and never read by alias.
    let origin = amplihack_git::command_in(&state.repo)
        .args(["remote", "get-url", "origin"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()?;
    let Some((owner, repo)) = origin
        .status
        .success()
        .then(|| String::from_utf8_lossy(&origin.stdout).into_owned())
        .as_deref()
        .and_then(origin_repo)
    else {
        return Ok(HeadCheck::Unavailable(Unavailable::OriginNotGithub));
    };
    let url = format!("https://github.com/{owner}/{repo}");

    // The PR number comes from `prepare`, which takes it from the loop's own
    // AD_PR, and from nowhere else (S7). A state file written before it
    // existed has none; the worktree's branch then stands in for the PR ref.
    let (refname, pr) = match state.pr.filter(|n| *n > 0) {
        Some(n) => (format!("refs/pull/{n}/head"), Some(n)),
        None => {
            let out = amplihack_git::command_in(&state.repo)
                .args(["symbolic-ref", "--short", "-q", "HEAD"])
                .output()?;
            let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // The name is not accepted rather than cleaned up, and the
            // outcome is unavailable rather than an error: a detached
            // worktree resolves no pull request (S6).
            if !out.status.success() || !valid_branch(&branch) {
                return Ok(HeadCheck::Unavailable(Unavailable::PrNumberUnresolved));
            }
            (format!("refs/heads/{branch}"), None)
        }
    };

    let remote = match ls_remote(&url, &refname) {
        Route::Answer(sha) => sha,
        Route::Unusable(u) => return Ok(HeadCheck::Unavailable(u)),
        Route::NoAnswer => match pr.map(|n| rest_head(&owner, &repo, n)) {
            Some(Route::Answer(sha)) => sha,
            Some(Route::Unusable(u)) => return Ok(HeadCheck::Unavailable(u)),
            // Neither route answered, or there was no second route to try.
            Some(Route::NoAnswer) | None => {
                return Ok(HeadCheck::Unavailable(Unavailable::ReadFailed));
            }
        },
    };
    Ok(if remote == local {
        HeadCheck::Matches
    } else {
        HeadCheck::Differs { local, remote }
    })
}
