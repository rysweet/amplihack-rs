//! `AMPLIHACK_RUN_GUIDANCE_DIRS`: which guidance folders a launch reads
//! (issue #2535, D6, I-7, I-8, F-3).
//!
//! `recipe run` sets the variable on the runner it starts. The launcher reads
//! only the folders listed there, so an interactive launch (variable unset)
//! never changes a prompt.

use super::fs_safety::{DirState, dir_state, untrusted_reason};
use super::pr_key::{PrRef, canonical_repo_from_remote, parse_pr_url, pr_guidance_dir, pr_key};
use super::sanitize::sanitize_line;
use super::{GUIDANCE_DIR, MAX_ENV_DIRS};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Cap on what `git remote get-url origin` may print.
const MAX_REMOTE_BYTES: u64 = 4096;

/// The one warning for a listed folder whose entries are not read. Skipped,
/// never refused: the caller goes on with the folders that pass, so the
/// warning carries no verdict word (issue #2809), and it names the folder
/// once. The path is escaped ([`sanitize_line`]) because the list comes from
/// the environment.
pub(crate) fn warn_skipped(path: &Path, reason: &str) {
    eprintln!(
        "{}",
        sanitize_line(&format!(
            "amplihack: ignoring operator guidance folder {}: {reason}",
            path.display()
        ))
    );
}

/// What the read-side check of one guidance folder found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DirCheck {
    /// It and its parent pass the trust rule: its entries are read.
    Trusted,
    /// It, or its parent, does not exist yet (no entry was ever added).
    Missing,
    /// It is not a guidance folder, or it or its parent fails a check: the
    /// fault alone, with no path and no verdict word.
    Untrusted(String),
}

/// The read-side check of one folder: it must be absolute and end in
/// `guidance`, and both its parent and itself must pass the trust rule.
pub(crate) fn check_guidance_dir(dir: &Path) -> DirCheck {
    let untrusted = |reason: &str| DirCheck::Untrusted(reason.to_string());
    if !dir.is_absolute() {
        return untrusted("it is not an absolute path");
    }
    if dir.file_name() != Some(OsStr::new(GUIDANCE_DIR)) {
        return untrusted("its last component is not `guidance`");
    }
    let Some(parent) = dir.parent() else {
        return untrusted("it has no parent folder");
    };
    match std::fs::symlink_metadata(parent) {
        Ok(meta) => {
            if let Some(reason) = untrusted_reason(&meta, true) {
                return untrusted(&format!("its parent folder fails a check: {reason}"));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DirCheck::Missing,
        Err(e) => return untrusted(&format!("cannot check its parent folder: {e}")),
    }
    match dir_state(dir) {
        Ok(DirState::Trusted) => DirCheck::Trusted,
        Ok(DirState::Missing) => DirCheck::Missing,
        Ok(DirState::Untrusted(reason)) => DirCheck::Untrusted(reason),
        Err(e) => DirCheck::Untrusted(e.to_string()),
    }
}

/// Every folder `value` lists, in order and without duplicates or empty
/// entries, each with its [`check_guidance_dir`]: at most [`MAX_ENV_DIRS`]
/// entries are considered, and a longer list is named in one warning.
///
/// `recipe guidance` shows the result as is, missing and untrusted folders
/// included; [`dirs_from_env_value`] keeps only the trusted ones.
pub(crate) fn listed_dirs(value: &OsStr) -> Vec<(PathBuf, DirCheck)> {
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
    let mut out: Vec<(PathBuf, DirCheck)> = Vec::new();
    for dir in listed.into_iter().take(MAX_ENV_DIRS) {
        if dir.as_os_str().is_empty() || out.iter().any(|(seen, _)| *seen == dir) {
            continue;
        }
        let check = check_guidance_dir(&dir);
        out.push((dir, check));
    }
    out
}

/// The folders of `value` that pass the read-side checks, in order and
/// without duplicates: at most [`MAX_ENV_DIRS`] entries are considered; each
/// must be absolute, end in `guidance`, and both it and its parent must pass
/// the trust rule. A folder that does not exist yet (no entry was ever
/// added) is skipped silently; anything else that fails is skipped with a
/// warning naming the path.
pub(crate) fn dirs_from_env_value(value: &OsStr) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for (dir, check) in listed_dirs(value) {
        match check {
            DirCheck::Trusted => out.push(dir),
            DirCheck::Missing => {}
            DirCheck::Untrusted(reason) => warn_skipped(&dir, &reason),
        }
    }
    out
}

/// `<state_root>/auto-drive/<key>/guidance` for pull request `number` of
/// `owner/repo`. The readers of a per-PR guidance folder, the run context
/// reader below and `recipe guidance`, both resolve it here.
pub(crate) fn pr_dir_for(number: u64, owner: &str, repo: &str, state_root: &Path) -> PathBuf {
    pr_guidance_dir(state_root, &pr_key(owner, repo, number))
}

/// The `github.com` owner and repository of `repo`'s `origin` remote, or
/// why it could not be found.
///
/// One `git -C <repo> remote get-url origin`, with no shell, stdin closed,
/// terminal prompts off, stderr discarded and stdout capped at 4 KiB.
///
/// `repo` comes from the run context's `repo_path`, so git runs in a folder
/// the recipe names. `remote get-url` reads only configuration; any other
/// git subcommand added here needs a security review first, because repository
/// configuration can name programs that some subcommands run.
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

/// The pull request a run's context names: `pr_url`, or a numeric
/// `pr_number` with the `origin` of `repo_path` (relative to `working_dir`;
/// `working_dir` itself when unset). `None` when the context names none or
/// it cannot be resolved; each cause is passed to `warn`, never the value.
///
/// One resolver for the two readers of a run's pull request: the folder
/// list `recipe run` gives its runner ([`runner_env_value`]) and the
/// live-run discovery of `recipe guide` and `recipe guidance show`, which
/// reads the same fields back from a session pointer (issue #3161).
fn pr_ref_with_warnings(
    context: &BTreeMap<String, String>,
    working_dir: &Path,
    warn: &mut dyn FnMut(&str),
) -> Option<PrRef> {
    let field = |key: &str| context.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
    if let Some(url) = field("pr_url") {
        match parse_pr_url(url) {
            Ok(pr) => return Some(pr),
            // The error names the rule broken, not the URL.
            Err(e) => warn(&format!("pr_url in the run context is {e}")),
        }
    }
    let number = field("pr_number")?;
    let Some(number) = number
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| number.parse::<u64>().ok())
        .flatten()
        .filter(|n| *n > 0)
    else {
        warn("pr_number in the run context is not a positive whole number");
        return None;
    };
    let repo = match field("repo_path") {
        Some(path) => working_dir.join(path),
        None => working_dir.to_path_buf(),
    };
    match origin_repo(&repo) {
        Ok((owner, repo)) => Some(PrRef {
            owner,
            repo,
            number,
        }),
        Err(reason) => {
            warn(&format!("cannot resolve pr_number: {reason}"));
            None
        }
    }
}

/// [`pr_ref_with_warnings`] without the warnings: the pull request a run's
/// context (or a session pointer's recorded fields) names, if it resolves.
pub(crate) fn pr_ref_from_context(
    context: &BTreeMap<String, String>,
    working_dir: &Path,
) -> Option<PrRef> {
    pr_ref_with_warnings(context, working_dir, &mut |_| {})
}

/// The per-PR guidance folder named by the run's context
/// ([`pr_ref_with_warnings`]).
///
/// `None`, silently, when the context names no pull request. When it names
/// one that cannot be resolved, `None` with one stderr line per cause: the
/// run still starts, without the per-PR folder.
fn pr_dir_from_context(
    context: &BTreeMap<String, String>,
    working_dir: &Path,
    state_root: Result<&Path, &str>,
) -> Option<PathBuf> {
    let named = |key: &str| context.get(key).is_some_and(|v| !v.trim().is_empty());
    if !named("pr_url") && !named("pr_number") {
        return None;
    }
    let root = match state_root {
        Ok(root) => root,
        Err(reason) => {
            warn_no_pr_dir(&format!("cannot find the state folder: {reason}"));
            return None;
        }
    };
    let pr = pr_ref_with_warnings(context, working_dir, &mut |cause| warn_no_pr_dir(cause))?;
    Some(pr_dir_for(pr.number, &pr.owner, &pr.repo, root))
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
