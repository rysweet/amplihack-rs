//! The per-pull-request guidance folder (issue #2535, D5).
//!
//! The key is always built from `https://github.com/<owner>/<repo>/<number>`,
//! whatever form the `origin` remote has, with the character rule of
//! `autodrive_state_key` in `amplifier-bundle/tools/autodrive_state.sh`:
//! every byte outside `[A-Za-z0-9._-]` becomes `_`. For an `https` origin the
//! folder is the drive's own state folder; for an `ssh` origin it is a
//! separate folder that the drive still reads (through
//! `AMPLIHACK_RUN_GUIDANCE_DIRS`).

use super::GuideError;
use std::path::{Path, PathBuf};

const GITHUB_HTTPS: &str = "https://github.com/";
const MAX_OWNER: usize = 39;
const MAX_REPO: usize = 100;

/// A pull request named by owner, repository and number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrRef {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

fn valid_owner(s: &str) -> bool {
    (1..=MAX_OWNER).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_repo(s: &str) -> bool {
    (1..=MAX_REPO).contains(&s.len())
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn bad_url(reason: &str) -> GuideError {
    GuideError::Usage(format!("not a GitHub pull request URL: {reason}"))
}

/// Why `url` does not start with `https://github.com/`.
fn prefix_problem(url: &str) -> &'static str {
    let Some(rest) = url.strip_prefix("https://") else {
        return "the scheme must be https";
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        "a user name or password is not allowed"
    } else if authority.contains(':') {
        "port is not allowed"
    } else {
        "the host must be github.com, in lower case"
    }
}

/// Parses `https://github.com/<owner>/<repo>/pull/<n>`, with an optional
/// trailing slash, query or fragment.
///
/// # Errors
///
/// [`GuideError::Usage`] (exit 2) for any other form: another scheme or host
/// (including a different case, user information or a port), percent
/// escapes, an owner over 39 bytes, a repository over 100 bytes, `.` or
/// `..`, a number that is zero or does not fit in 64 bits, or extra path
/// segments.
pub(crate) fn parse_pr_url(url: &str) -> Result<PrRef, GuideError> {
    let Some(rest) = url.strip_prefix(GITHUB_HTTPS) else {
        return Err(bad_url(prefix_problem(url)));
    };
    let path = rest.split(['?', '#']).next().unwrap_or_default();
    if path.contains('%') {
        return Err(bad_url("percent escapes are not accepted"));
    }
    let path = path.strip_suffix('/').unwrap_or(path);
    let parts: Vec<&str> = path.split('/').collect();
    let [owner, repo, "pull", number] = parts.as_slice() else {
        return Err(bad_url("the path must be /<owner>/<repo>/pull/<number>"));
    };
    if !valid_owner(owner) {
        return Err(bad_url("the owner is not a GitHub account name"));
    }
    if !valid_repo(repo) {
        return Err(bad_url("the repository is not a GitHub repository name"));
    }
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad_url("the pull request number is not a number"));
    }
    let number: u64 = number
        .parse()
        .map_err(|_| bad_url("the pull request number is too large"))?;
    if number == 0 {
        return Err(bad_url("the pull request number is zero"));
    }
    Ok(PrRef {
        owner: (*owner).to_string(),
        repo: (*repo).to_string(),
        number,
    })
}

/// The owner and repository of a `github.com` remote in `https`, `ssh://`
/// or scp form (`git@github.com:o/r.git`). `None` for any other host.
pub(crate) fn canonical_repo_from_remote(remote: &str) -> Option<(String, String)> {
    let remote = remote.trim();
    let path = [
        "https://github.com/",
        "ssh://git@github.com/",
        "ssh://github.com/",
        "git@github.com:",
    ]
    .iter()
    .find_map(|prefix| remote.strip_prefix(prefix))?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, repo) = path.split_once('/')?;
    (valid_owner(owner) && valid_repo(repo)).then(|| (owner.to_string(), repo.to_string()))
}

/// Every byte outside `[A-Za-z0-9._-]` becomes `_` (the shell's
/// `tr -c 'A-Za-z0-9._-' '_'`).
pub(crate) fn sanitize(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-') {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

/// The per-PR key: one path component of at most 180 bytes for valid input.
pub(crate) fn pr_key(owner: &str, repo: &str, number: u64) -> String {
    sanitize(&format!("{GITHUB_HTTPS}{owner}/{repo}/{number}"))
}

/// `<state_root>/auto-drive/<key>/guidance`.
pub(crate) fn pr_guidance_dir(state_root: &Path, key: &str) -> PathBuf {
    pr_guidance_parent(state_root, key).join(super::GUIDANCE_DIR)
}

/// `<state_root>/auto-drive/<key>`, the drive's per-PR state folder.
pub(crate) fn pr_guidance_parent(state_root: &Path, key: &str) -> PathBuf {
    state_root.join(super::AUTO_DRIVE_DIR).join(key)
}

/// `${AMPLIHACK_STATE_DIR:-$HOME/.amplihack/state}`, as the shell helpers
/// resolve it. `None` when neither is set.
pub(crate) fn state_root() -> Option<PathBuf> {
    match std::env::var_os("AMPLIHACK_STATE_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => crate::user_config::home_dir().map(|h| h.join(".amplihack").join("state")),
    }
}
