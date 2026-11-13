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

/// The `github.com` owner and repository of `repo`'s `origin` remote.
///
/// One `git -C <repo> remote get-url origin`, with no shell, stdin closed,
/// terminal prompts off, stderr discarded and stdout capped at 4 KiB.
fn origin_repo(repo: &Path) -> Option<(String, String)> {
    let mut child = amplihack_git::command()
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = Vec::new();
    let read = child
        .stdout
        .take()
        .map(|s| s.take(MAX_REMOTE_BYTES).read_to_end(&mut out));
    let status = child.wait().ok()?;
    if !status.success() || !matches!(read, Some(Ok(_))) {
        return None;
    }
    canonical_repo_from_remote(std::str::from_utf8(&out).ok()?)
}

/// The per-PR guidance folder named by the run's context: `pr_url`, or a
/// numeric `pr_number` with the `origin` of `repo_path` (relative to
/// `working_dir`; `working_dir` itself when unset).
fn pr_dir_from_context(
    context: &BTreeMap<String, String>,
    working_dir: &Path,
    state_root: &Path,
) -> Option<PathBuf> {
    if !state_root.is_absolute() {
        return None;
    }
    if let Some(url) = context
        .get("pr_url")
        .map(|u| u.trim())
        .filter(|u| !u.is_empty())
        && let Ok(pr) = parse_pr_url(url)
    {
        return Some(pr_guidance_dir(
            state_root,
            &pr_key(&pr.owner, &pr.repo, pr.number),
        ));
    }
    let number = context.get("pr_number").map(|n| n.trim())?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u64 = number.parse().ok().filter(|n| *n > 0)?;
    let repo = match context
        .get("repo_path")
        .map(|r| r.trim())
        .filter(|r| !r.is_empty())
    {
        Some(path) => working_dir.join(path),
        None => working_dir.to_path_buf(),
    };
    let (owner, name) = origin_repo(&repo)?;
    Some(pr_guidance_dir(state_root, &pr_key(&owner, &name, number)))
}

/// The value `recipe run` gives its runner: the inherited list, then
/// `<run_dir>/guidance`, then the per-PR folder from the context, without
/// duplicates and in that order. `None` when the list is empty (a dry run
/// with nothing inherited).
pub(crate) fn runner_env_value(
    inherited: Option<&OsString>,
    run_dir: Option<&Path>,
    context: &BTreeMap<String, String>,
    working_dir: &Path,
    state_root: &Path,
) -> Option<OsString> {
    let mut list: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if !p.as_os_str().is_empty() && !list.contains(&p) {
            list.push(p);
        }
    };
    if let Some(value) = inherited.filter(|v| !v.is_empty()) {
        std::env::split_paths(value).for_each(&mut push);
    }
    if let Some(run_dir) = run_dir {
        push(run_dir.join(GUIDANCE_DIR));
    }
    if let Some(pr) = pr_dir_from_context(context, working_dir, state_root) {
        push(pr);
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
