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

pub fn validate(state: &State, expected: &Snapshot, completing: bool) -> Result<serde_json::Value> {
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
    if completing {
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

/// What GitHub says the PR head is, against the reviewed head (#2333).
///
/// Completion keeps this comparison on purpose. The record binds the review
/// to the worktree's `HEAD`; only GitHub can say whether that commit is what
/// would be merged. A completion granted while the PR head on GitHub is some
/// other commit would carry an unreviewed head to the merge step, so neither
/// `Differs` nor `Unavailable` ever completes. Neither ends the run either:
/// dispatch hands both to the loop-health evaluator with the fault named,
/// and a later round can complete once the read answers and agrees.
#[derive(Debug, PartialEq, Eq)]
pub enum RemoteHead {
    Matches,
    /// The PR head on GitHub, which is not the reviewed head.
    Differs(String),
    /// The read did not answer; the text names why, with gh's own stderr.
    Unavailable(String),
}

impl RemoteHead {
    /// The fault in one line, for the evaluator and the operator.
    pub fn fault(&self, head: &str) -> Option<String> {
        match self {
            Self::Matches => None,
            Self::Differs(remote) => Some(format!(
                "head differs: the PR head on GitHub is {remote}, but the reviewed head is {head}"
            )),
            Self::Unavailable(why) => Some(format!("head unavailable: {why}")),
        }
    }
}

/// One REST read of the PR head, so a GraphQL refusal does not hide it.
/// `gh api` fills `{owner}/{repo}` from the worktree's remote.
pub fn remote_head(repo: &Path, head: &str) -> RemoteHead {
    let pr = match pr_number(repo) {
        Ok(pr) => pr,
        Err(why) => return RemoteHead::Unavailable(why),
    };
    let endpoint = format!("repos/{{owner}}/{{repo}}/pulls/{pr}");
    let sha = match gh_api(repo, &endpoint, ".head.sha") {
        Ok(sha) => sha,
        Err(why) => return RemoteHead::Unavailable(why),
    };
    let sha = sha.trim();
    if sha.is_empty() {
        RemoteHead::Unavailable(format!("gh api {endpoint} returned no head SHA"))
    } else if sha == head.trim() {
        RemoteHead::Matches
    } else {
        RemoteHead::Differs(sha.to_string())
    }
}

/// The PR number: the loop's `pr_number` context, passed as `PR_NUMBER`.
/// When a caller passed it empty, the open PR whose head is the worktree's
/// branch, which is what `gh pr view` used to infer. A missing or ambiguous
/// answer is returned as a named blocker, never guessed.
fn pr_number(repo: &Path) -> std::result::Result<String, String> {
    let given = std::env::var("PR_NUMBER").unwrap_or_default();
    let given = given.trim();
    if !given.is_empty() {
        return if given.bytes().all(|b| b.is_ascii_digit()) {
            Ok(given.to_string())
        } else {
            Err(format!("PR_NUMBER {given:?} is not a PR number"))
        };
    }
    let branch = amplihack_git::command_in(repo)
        .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty())
        .ok_or("PR_NUMBER is empty and the worktree has no branch to find the PR by")?;
    let endpoint = format!(
        "repos/{{owner}}/{{repo}}/pulls?state=open&head={{owner}}:{}",
        query_escape(&branch)
    );
    let numbers = gh_api(repo, &endpoint, ".[].number")?;
    let numbers: Vec<&str> = numbers.split_whitespace().collect();
    match numbers.as_slice() {
        [one] if one.bytes().all(|b| b.is_ascii_digit()) => Ok((*one).to_string()),
        [] => Err(format!(
            "PR_NUMBER is empty and no open PR has head branch {branch}"
        )),
        _ => Err(format!(
            "PR_NUMBER is empty and branch {branch} matches more than one open PR ({})",
            numbers.join(", ")
        )),
    }
}

fn gh_api(repo: &Path, endpoint: &str, jq: &str) -> std::result::Result<String, String> {
    let o = Command::new("gh")
        .current_dir(repo)
        .env_remove("GH_REPO")
        .args(["api", endpoint, "--jq", jq])
        .output()
        .map_err(|e| format!("gh api {endpoint} could not run: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "gh api {endpoint} failed ({}): {}",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        ));
    }
    String::from_utf8(o.stdout).map_err(|_| format!("gh api {endpoint} returned non-UTF-8 output"))
}

/// Percent-encodes a branch name for a query value; `/` stays as it is.
fn query_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_are_escaped_for_the_query() {
        assert_eq!(query_escape("feat/issue-2333_x.y"), "feat/issue-2333_x.y");
        assert_eq!(query_escape("a&b c#d"), "a%26b%20c%23d");
    }

    #[test]
    fn faults_name_both_heads_or_the_cause() {
        assert_eq!(RemoteHead::Matches.fault("a"), None);
        let differs = RemoteHead::Differs("b".into()).fault("a").unwrap();
        assert!(differs.contains("GitHub is b") && differs.contains("reviewed head is a"));
        let gone = RemoteHead::Unavailable("HTTP 502".into())
            .fault("a")
            .unwrap();
        assert_eq!(gone, "head unavailable: HTTP 502");
    }
}
