//! `AMPLIHACK_RUN_GUIDANCE_DIRS`: which guidance folders a launch reads
//! (issue #2535, D6, I-7, I-8, F-3).
//!
//! `recipe run` sets the variable on the runner it starts. The launcher reads
//! only the folders listed there, so an interactive launch (variable unset)
//! never changes a prompt.

use super::fs_safety::{check_dir, untrusted_reason};
use super::pr_key::{canonical_repo_from_remote, parse_pr_url, pr_guidance_dir, pr_key};
use super::{GUIDANCE_DIR, MAX_ENV_DIRS};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Cap on what `git remote get-url origin` may print.
const MAX_REMOTE_BYTES: u64 = 4096;

fn warn_skipped(path: &Path, reason: &str) {
    eprintln!(
        "amplihack: ignoring operator guidance folder {}: {reason}",
        path.display()
    );
}

/// The folders of `value` that pass the read-side checks, in order and
/// without duplicates: at most [`MAX_ENV_DIRS`] entries are considered; each
/// must be absolute, end in `guidance`, and both it and its parent must pass
/// the trust rule. A folder that does not exist yet (no entry was ever
/// added) is skipped silently; anything else that fails is skipped with a
/// warning naming the path.
pub(crate) fn dirs_from_env_value(value: &OsStr) -> Vec<PathBuf> {
    if value.is_empty() {
        return Vec::new();
    }
    let listed: Vec<PathBuf> = std::env::split_paths(value).collect();
    if listed.len() > MAX_ENV_DIRS {
        eprintln!(
            "amplihack: {} lists {} folders; reading the first {MAX_ENV_DIRS}",
            super::GUIDANCE_DIRS_ENV,
            listed.len()
        );
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for dir in listed.into_iter().take(MAX_ENV_DIRS) {
        if dir.as_os_str().is_empty() || out.contains(&dir) {
            continue;
        }
        if !dir.is_absolute() {
            warn_skipped(&dir, "it is not an absolute path");
            continue;
        }
        if dir.file_name() != Some(OsStr::new(GUIDANCE_DIR)) {
            warn_skipped(&dir, "its last component is not `guidance`");
            continue;
        }
        let Some(parent) = dir.parent() else {
            warn_skipped(&dir, "it has no parent folder");
            continue;
        };
        match std::fs::symlink_metadata(parent) {
            Ok(meta) => {
                if let Some(reason) = untrusted_reason(&meta, true) {
                    warn_skipped(&dir, &format!("its parent folder fails a check: {reason}"));
                    continue;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                warn_skipped(&dir, &format!("cannot check its parent folder: {e}"));
                continue;
            }
        }
        match check_dir(&dir) {
            Ok(true) => out.push(dir),
            Ok(false) => {}
            Err(e) => warn_skipped(&dir, &e.to_string()),
        }
    }
    out
}

/// The `github.com` owner and repository of `repo`'s `origin` remote, or
/// why it could not be found.
///
/// One `git -C <repo> remote get-url origin`, with no shell, stdin closed,
/// terminal prompts off, stderr discarded and stdout capped at 4 KiB.
fn origin_repo(repo: &Path) -> Result<(String, String), &'static str> {
    let mut child = amplihack_git::command()
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "`git remote get-url origin` could not be started")?;
    let mut out = Vec::new();
    let read = child
        .stdout
        .take()
        .map(|s| s.take(MAX_REMOTE_BYTES).read_to_end(&mut out));
    let status = child
        .wait()
        .map_err(|_| "`git remote get-url origin` could not be waited for")?;
    if !status.success() {
        return Err("`git remote get-url origin` failed in repo_path");
    }
    if !matches!(read, Some(Ok(_))) {
        return Err("the output of `git remote get-url origin` could not be read");
    }
    let remote = std::str::from_utf8(&out)
        .map_err(|_| "the origin remote of repo_path is not valid UTF-8")?;
    canonical_repo_from_remote(remote)
        .ok_or("the origin remote of repo_path is not a github.com repository")
}

/// Says, on stderr, why the run gets no per-PR guidance folder. Names the
/// context field and the cause, never its value.
fn warn_no_pr_dir(reason: &str) {
    eprintln!("amplihack: no pull request operator guidance for this run: {reason}");
}

/// The per-PR guidance folder named by the run's context: `pr_url`, or a
/// numeric `pr_number` with the `origin` of `repo_path` (relative to
/// `working_dir`; `working_dir` itself when unset).
///
/// `None`, silently, when the context names no pull request. When it names
/// one that cannot be resolved, `None` with one stderr line per cause: the
/// run still starts, without the per-PR folder.
fn pr_dir_from_context(
    context: &BTreeMap<String, String>,
    working_dir: &Path,
    state_root: Result<&Path, &str>,
) -> Option<PathBuf> {
    let field = |key: &str| context.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
    let url = field("pr_url");
    let number = field("pr_number");
    if url.is_none() && number.is_none() {
        return None;
    }
    let root = match state_root {
        Ok(root) => root,
        Err(reason) => {
            warn_no_pr_dir(&format!("cannot find the state folder: {reason}"));
            return None;
        }
    };
    if let Some(url) = url {
        match parse_pr_url(url) {
            Ok(pr) => {
                return Some(pr_guidance_dir(
                    root,
                    &pr_key(&pr.owner, &pr.repo, pr.number),
                ));
            }
            // The error names the rule broken, not the URL.
            Err(e) => warn_no_pr_dir(&format!("pr_url in the run context is {e}")),
        }
    }
    let number = number?;
    let Some(number) = number
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| number.parse::<u64>().ok())
        .flatten()
        .filter(|n| *n > 0)
    else {
        warn_no_pr_dir("pr_number in the run context is not a positive whole number");
        return None;
    };
    let repo = match field("repo_path") {
        Some(path) => working_dir.join(path),
        None => working_dir.to_path_buf(),
    };
    match origin_repo(&repo) {
        Ok((owner, name)) => Some(pr_guidance_dir(root, &pr_key(&owner, &name, number))),
        Err(reason) => {
            warn_no_pr_dir(&format!("cannot resolve pr_number: {reason}"));
            None
        }
    }
}

/// The value `recipe run` gives its runner: `<run_dir>/guidance`, then the
/// per-PR folder from the context, then the inherited list, without
/// duplicates and in that order. The run's own folders come first so that
/// the launcher's [`MAX_ENV_DIRS`] limit, in a deep nesting, drops the
/// outermost inherited folders and never the innermost run's own. Entries
/// are sorted by time across folders, so this order does not change the
/// block. A folder whose path cannot be put in a path list (it holds the
/// separator) is left out with a warning; the others are kept. `None` when
/// the list is empty (a dry run with nothing inherited).
pub(crate) fn runner_env_value(
    inherited: Option<&OsString>,
    run_dir: Option<&Path>,
    context: &BTreeMap<String, String>,
    working_dir: &Path,
    state_root: Result<&Path, &str>,
) -> Option<OsString> {
    let mut list: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.as_os_str().is_empty() || list.contains(&p) {
            return;
        }
        if std::env::join_paths([&p]).is_err() {
            eprintln!(
                "amplihack: leaving operator guidance folder {} out of {}: its path contains the list separator",
                p.display(),
                super::GUIDANCE_DIRS_ENV
            );
            return;
        }
        list.push(p);
    };
    if let Some(run_dir) = run_dir {
        push(run_dir.join(GUIDANCE_DIR));
    }
    if let Some(pr) = pr_dir_from_context(context, working_dir, state_root) {
        push(pr);
    }
    if let Some(value) = inherited.filter(|v| !v.is_empty()) {
        std::env::split_paths(value).for_each(&mut push);
    }
    if list.is_empty() {
        return None;
    }
    match std::env::join_paths(&list) {
        Ok(value) => Some(value),
        Err(e) => {
            eprintln!(
                "amplihack: cannot list operator guidance folders in {}: {e}",
                super::GUIDANCE_DIRS_ENV
            );
            None
        }
    }
}
