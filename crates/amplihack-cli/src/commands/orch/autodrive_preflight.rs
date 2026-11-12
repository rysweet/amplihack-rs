//! Shared, non-reconciling PR worktree preflight (#2188).
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[path = "autodrive_preflight/diagnostic.rs"]
mod diagnostic;
#[path = "autodrive_preflight/finalization.rs"]
mod finalization;
#[path = "autodrive_preflight/transport.rs"]
mod transport;
pub(super) use finalization::validate_state_directory;
use finalization::{FinalizationContext, finalize_refusal};

const SCHEMA_VERSION: u8 = 1;

#[derive(serde::Serialize)]
struct Ready<'a> {
    schema_version: u8,
    outcome: &'static str,
    terminal_refusal: bool,
    disposition: &'static str,
    worktree: &'a Path,
    head_sha: &'a str,
    head_matches: bool,
}

#[derive(serde::Serialize)]
struct Refusal {
    schema_version: u8,
    outcome: RefusalOutcome,
    terminal_refusal: bool,
    evidence: Value,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) enum RefusalOutcome {
    #[serde(rename = "pr-worktree-interrupted")]
    Interrupted,
    #[serde(rename = "pr-worktree-dirty")]
    Dirty,
    #[serde(rename = "pr-worktree-behind")]
    Behind,
    #[serde(rename = "pr-worktree-diverged")]
    Diverged,
    #[serde(rename = "pr-worktree-comparison-failed")]
    ComparisonFailed,
    #[serde(rename = "pr-worktree-push-failed")]
    PushFailed,
    #[serde(rename = "pr-worktree-not-found")]
    NotFound,
    #[serde(rename = "pr-worktree-ambiguous")]
    Ambiguous,
    #[serde(rename = "pr-worktree-branch-mismatch")]
    BranchMismatch,
    /// The drive's state directory is a symlink, belongs to someone else or
    /// cannot be made private. Checked before anything else, and never
    /// published as a PR comment: it is a fault of this host, not of the PR.
    #[serde(rename = "state-dir-not-private")]
    StateDirNotPrivate,
}

impl RefusalOutcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Interrupted => "pr-worktree-interrupted",
            Self::Dirty => "pr-worktree-dirty",
            Self::Behind => "pr-worktree-behind",
            Self::Diverged => "pr-worktree-diverged",
            Self::ComparisonFailed => "pr-worktree-comparison-failed",
            Self::PushFailed => "pr-worktree-push-failed",
            Self::NotFound => "pr-worktree-not-found",
            Self::Ambiguous => "pr-worktree-ambiguous",
            Self::BranchMismatch => "pr-worktree-branch-mismatch",
            Self::StateDirNotPrivate => "state-dir-not-private",
        }
    }
}

#[derive(clap::Args, Debug)]
pub struct PreflightRequest {
    /// Pull request number in the base repository.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    pub pr: u64,
    /// Base OWNER/REPO, or . to resolve the launch repository through gh.
    #[arg(long, value_parser = repository_argument)]
    pub repo: String,
    /// Optional expected source branch; a mismatch is a named refusal.
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    pub branch: String,
    /// Drive state directory for the atomic refusal result.
    #[arg(long)]
    pub state_dir: PathBuf,
}

pub(super) fn output(at: &Path, program: &str, args: &[&str]) -> Result<Vec<u8>> {
    output_expected(at, program, args, None)
}

fn output_expected(
    at: &Path,
    program: &str,
    args: &[&str],
    expected: Option<(&str, &str)>,
) -> Result<Vec<u8>> {
    let mut command = if program == "git" {
        amplihack_git::command_in(at)
    } else {
        Command::new(program)
    };
    amplihack_git::scrub(&mut command);
    let _transport =
        if program == "git" && matches!(args.first(), Some(&"fetch" | &"push" | &"ls-remote")) {
            let url = args
                .iter()
                .find(|arg| arg.starts_with("https://github.com/"))
                .context("destination-policy: missing canonical source destination")?;
            Some(transport::configure(&mut command, at, url, expected)?)
        } else {
            None
        };
    if program == "git" {
        command.args([
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
        ]);
    }
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    command
        .current_dir(at)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?);
    let status = command.status()?;
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    let mut diagnostic = Vec::new();
    stdout.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    stderr.take(16 * 1024).read_to_end(&mut diagnostic)?;
    if !status.success() {
        bail!(
            "{program} failed ({}): {}",
            status,
            diagnostic::sanitize(&String::from_utf8_lossy(&diagnostic))
        );
    }
    if bytes.len() > 1024 * 1024 {
        bail!("{program} comparison output exceeds capture limit");
    }
    Ok(bytes)
}
pub(super) fn git(at: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(output(at, "git", args)?)?.trim().into())
}
fn identity(s: &str) -> bool {
    let parts: Vec<_> = s.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && *p != "."
                && *p != ".."
                && !p.starts_with('-')
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}
fn repository_argument(s: &str) -> std::result::Result<String, String> {
    if s == "." || identity(s) {
        Ok(s.into())
    } else {
        Err("expected GitHub OWNER/REPO".into())
    }
}
fn refusal(outcome: RefusalOutcome, evidence: Value) -> Value {
    serde_json::to_value(Refusal {
        schema_version: SCHEMA_VERSION,
        outcome,
        terminal_refusal: true,
        evidence,
    })
    .expect("refusal consists only of JSON values and a known enum")
}
fn git_path(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(path) => json!({"encoding":"utf-8","value":path}),
        Err(_) => {
            use base64::Engine;
            json!({"encoding":"base64","value":base64::engine::general_purpose::STANDARD.encode(bytes)})
        }
    }
}
fn status_paths(bytes: &[u8]) -> Result<Vec<Value>> {
    let mut entries = bytes.split(|b| *b == 0).filter(|e| !e.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            bail!("malformed NUL-delimited Git status");
        }
        paths.push(git_path(&entry[3..]));
        if entry[..2].iter().any(|c| *c == b'R' || *c == b'C') {
            paths.push(git_path(entries.next().context("missing rename source")?));
        }
    }
    Ok(paths)
}
fn ancestry(at: &Path, a: &str, b: &str) -> Result<bool> {
    let o = amplihack_git::command()
        .current_dir(at)
        .args(["merge-base", "--is-ancestor", a, b])
        .output()?;
    match o.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("ancestry comparison failed"),
    }
}
fn read_operation_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "operation metadata is not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "operation metadata exceeds capture limit",
        ));
    }
    Ok(bytes)
}
fn operation(at: &Path) -> Result<Value> {
    let mut operations = Vec::new();
    for name in [
        "rebase-merge",
        "rebase-apply",
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "sequencer",
    ] {
        let p = git(at, &["rev-parse", "--git-path", name])?;
        let path = at.join(&p);
        let marker = match fs::symlink_metadata(&path) {
            Ok(marker) => marker,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        {
            let mut metadata = serde_json::Map::new();
            if marker.file_type().is_symlink() {
                metadata.insert(
                    "diagnostic".into(),
                    json!("operation marker is a symlink; not followed"),
                );
            } else if marker.is_dir() {
                for field in [
                    "head-name",
                    "orig-head",
                    "onto",
                    "head",
                    "todo",
                    "abort-safety",
                ] {
                    match read_operation_file(&path.join(field)) {
                        Ok(bytes) => {
                            metadata.insert(field.into(), git_path(&bytes));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            metadata.insert(format!("{field}_error"), json!(e.to_string()));
                        }
                    }
                }
            } else {
                match read_operation_file(&path) {
                    Ok(bytes) => {
                        metadata.insert("head".into(), git_path(&bytes));
                    }
                    Err(error) => {
                        metadata.insert("read_error".into(), json!(error.to_string()));
                    }
                }
            }
            operations.push(json!({"operation":name,"metadata":metadata}));
        }
    }
    let unmerged = match output(at, "git", &["ls-files", "-u", "-z"]) {
        Ok(bytes) => bytes,
        Err(error) if !operations.is_empty() => {
            return Ok(
                json!({"operations":operations,"unmerged_paths":[],"no_unmerged_paths":false,"unmerged_paths_error":error.to_string()}),
            );
        }
        Err(error) => return Err(error),
    };
    let paths: Vec<_> = unmerged
        .split(|b| *b == 0)
        .filter(|e| !e.is_empty())
        .map(|entry| {
            entry
                .splitn(2, |b| *b == b'\t')
                .nth(1)
                .map(git_path)
                .context("malformed unmerged index entry")
        })
        .collect::<Result<_>>()?;
    Ok(
        json!({"operations":operations,"unmerged_paths":paths,"no_unmerged_paths":unmerged.is_empty()}),
    )
}
fn clean(at: &Path) -> Result<bool> {
    Ok(output(
        at,
        "git",
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?
    .is_empty())
}
struct WorktreeRecord {
    path: PathBuf,
    branch: Option<Vec<u8>>,
    prunable: bool,
}
/// Parse `git worktree list --porcelain -z`: attribute lines end in NUL and a
/// record ends with an empty line.
fn worktree_records(listing: &[u8]) -> Vec<WorktreeRecord> {
    let mut records: Vec<WorktreeRecord> = Vec::new();
    for line in listing.split(|b| *b == 0) {
        if let Some(path) = line.strip_prefix(b"worktree ") {
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStrExt;
                PathBuf::from(std::ffi::OsStr::from_bytes(path))
            };
            #[cfg(not(unix))]
            let path = PathBuf::from(String::from_utf8_lossy(path).into_owned());
            records.push(WorktreeRecord {
                path,
                branch: None,
                prunable: false,
            });
        } else if let Some(record) = records.last_mut() {
            if let Some(branch) = line.strip_prefix(b"branch ") {
                record.branch = Some(branch.to_vec());
            } else if line == b"prunable" || line.starts_with(b"prunable ") {
                record.prunable = true;
            }
        }
    }
    records
}
/// The canonical worktree path when it has the PR branch checked out, or is
/// rebasing it.
fn worktree_matches(path: &Path, common: &Path, source_ref: &str) -> Result<Option<PathBuf>> {
    let path = fs::canonicalize(path)?;
    if fs::canonicalize(path.join(git(&path, &["rev-parse", "--git-common-dir"])?))? != common {
        bail!("worktree belongs to another repository");
    }
    let symbolic = amplihack_git::command()
        .current_dir(&path)
        .args(["symbolic-ref", "-q", "HEAD"])
        .output()?;
    let checked = match symbolic.status.code() {
        Some(0) => String::from_utf8(symbolic.stdout)?.trim().to_owned(),
        Some(1) => String::new(),
        _ => bail!("cannot inspect worktree branch"),
    };
    if checked == source_ref {
        return Ok(Some(path));
    }
    for op in ["rebase-merge", "rebase-apply"] {
        let p = git(
            &path,
            &["rev-parse", "--git-path", &format!("{op}/head-name")],
        )?;
        match read_operation_file(&path.join(p)) {
            Ok(head) if String::from_utf8_lossy(&head).trim() == source_ref => {
                return Ok(Some(path));
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(None)
}
/// The push runs from an isolated Git directory (transport.rs), so Git does
/// not move the worktree's remote-tracking ref. Once the source repository
/// has been verified at `local`, record that for every configured remote
/// whose URL is the source repository, so `git status` in the worktree no
/// longer reports the pushed commits as ahead.
fn record_verified_push(at: &Path, url: &str, branch: &str, local: &str) -> Result<()> {
    let bare = url.trim_end_matches(".git");
    for remote in git(at, &["remote"])?.lines() {
        let configured =
            git(at, &["config", "--get", &format!("remote.{remote}.url")]).unwrap_or_default();
        if configured.trim_end_matches('/').trim_end_matches(".git") != bare {
            continue;
        }
        git(
            at,
            &[
                "update-ref",
                "-m",
                "amplihack auto-drive preflight: verified push",
                &format!("refs/remotes/{remote}/{branch}"),
                local,
            ],
        )?;
    }
    Ok(())
}
fn evaluate(r: &PreflightRequest, cwd: &Path) -> Result<Value> {
    let bytes = output(
        cwd,
        "gh",
        &[
            "pr",
            "view",
            &r.pr.to_string(),
            "--repo",
            &r.repo,
            "--json",
            "number,state,headRefName,headRefOid,headRepository,headRepositoryOwner",
        ],
    )?;
    let meta: Value = serde_json::from_slice(&bytes)?;
    if meta["number"].as_u64() != Some(r.pr) || meta["state"] != "OPEN" {
        bail!("PR metadata does not identify the requested open PR");
    }
    let branch = meta["headRefName"]
        .as_str()
        .context("missing source branch")?;
    if !r.branch.is_empty() && r.branch != branch {
        return Ok(refusal(
            RefusalOutcome::BranchMismatch,
            json!({"expected":r.branch,"actual":branch}),
        ));
    }
    git(cwd, &["check-ref-format", &format!("refs/heads/{branch}")])?;
    let source = meta["headRepository"]["nameWithOwner"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            Some(format!(
                "{}/{}",
                meta["headRepositoryOwner"]["login"].as_str()?,
                meta["headRepository"]["name"].as_str()?
            ))
        })
        .context("missing source repository")?;
    if !identity(&source) {
        bail!("unsupported source repository");
    }
    let url = format!("https://github.com/{source}.git");
    if let Some(repository_url) = meta["headRepository"]["url"].as_str()
        && repository_url
            .trim_end_matches('/')
            .trim_end_matches(".git")
            != format!("https://github.com/{source}")
    {
        bail!("unsupported source repository destination");
    }
    let listing = output(cwd, "git", &["worktree", "list", "--porcelain", "-z"])?;
    let common = fs::canonicalize(cwd.join(git(cwd, &["rev-parse", "--git-common-dir"])?))?;
    let source_ref = format!("refs/heads/{branch}");
    let mut candidates = Vec::new();
    for record in worktree_records(&listing) {
        // git-worktree(1): a "prunable" record names a worktree whose
        // directory is gone. It cannot be the PR worktree, and one stale
        // record must not stop every drive in the repository.
        if record.prunable {
            continue;
        }
        match worktree_matches(&record.path, &common, &source_ref) {
            Ok(Some(path)) => {
                if !candidates.contains(&path) {
                    candidates.push(path);
                }
            }
            Ok(None) => {}
            // A worktree on another branch that cannot be read is not ours to
            // judge. One listed on the PR branch is: its failure is reported.
            Err(_) if record.branch.as_deref() != Some(source_ref.as_bytes()) => {}
            Err(error) => return Err(error),
        }
    }
    if candidates.len() != 1 {
        return Ok(refusal(
            if candidates.is_empty() {
                RefusalOutcome::NotFound
            } else {
                RefusalOutcome::Ambiguous
            },
            json!({"branch":branch,"candidates":candidates.iter().map(|p| git_path(p.as_os_str().as_encoded_bytes())).collect::<Vec<_>>()}),
        ));
    }
    let at = &candidates[0];
    if at.to_str().is_none_or(|p| p.chars().any(char::is_control)) {
        return Ok(refusal(
            RefusalOutcome::ComparisonFailed,
            json!({"worktree":git_path(at.as_os_str().as_encoded_bytes()),"diagnostic":"recipe continuation requires a UTF-8 worktree path"}),
        ));
    }
    let op = operation(at)?;
    if !op["operations"].as_array().unwrap().is_empty() || op["no_unmerged_paths"] == false {
        return Ok(refusal(RefusalOutcome::Interrupted, op));
    }
    if !clean(at)? {
        return Ok(refusal(
            RefusalOutcome::Dirty,
            json!({"worktree":at,"paths":status_paths(&output(at,"git", &["status","--porcelain=v1","-z"])? )?}),
        ));
    }
    let local = git(at, &["rev-parse", "HEAD"])?;
    let mut tracking_error = None;
    git(
        at,
        &[
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            &url,
            &source_ref,
        ],
    )?;
    let advertised = git(at, &["ls-remote", "--heads", &url, &source_ref])?;
    let remote = advertised
        .split_whitespace()
        .next()
        .context("source branch unavailable")?;
    if !matches!(remote.len(), 40 | 64) || !remote.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid advertised source commit");
    }
    if meta["headRefOid"].as_str() != Some(remote) {
        bail!("advertised source head changed since PR metadata was read");
    }
    git(at, &["cat-file", "-e", &format!("{remote}^{{commit}}")])?;
    if git(at, &["rev-parse", "--is-shallow-repository"])? != "false" {
        bail!("incomplete shallow history");
    }
    if local != remote {
        if !ancestry(at, remote, &local)? {
            return Ok(refusal(
                if ancestry(at, &local, remote)? {
                    RefusalOutcome::Behind
                } else {
                    RefusalOutcome::Diverged
                },
                json!({"local":local,"remote":remote,"worktree":at}),
            ));
        }
        let mut stage = "local-recheck";
        let mut publish = || -> Result<()> {
            if git(at, &["rev-parse", "HEAD"])? != local
                || !clean(at)?
                || operation(at)? != op
                || git(at, &["symbolic-ref", "-q", "HEAD"])? != source_ref
            {
                bail!("local worktree changed before push");
            }
            if git(at, &["ls-remote", "--heads", &url, &source_ref])?
                .split_whitespace()
                .next()
                != Some(remote)
            {
                bail!("source branch changed before push");
            }
            stage = "push";
            output_expected(
                at,
                "git",
                &["push", &url, &format!("{local}:{source_ref}")],
                Some((&source_ref, remote)),
            )?;
            stage = "remote-verification";
            let verified = git(at, &["ls-remote", "--heads", &url, &source_ref])?;
            if verified.split_whitespace().next() != Some(local.as_str()) {
                bail!("remote verification did not match captured commit");
            }
            stage = "local-verification";
            if git(at, &["rev-parse", "HEAD"])? != local
                || !clean(at)?
                || operation(at)? != op
                || git(at, &["symbolic-ref", "-q", "HEAD"])? != source_ref
            {
                bail!("local worktree changed during push");
            }
            Ok(())
        };
        if let Err(e) = publish() {
            return Ok(refusal(
                RefusalOutcome::PushFailed,
                json!({"stage":stage,"diagnostic":e.to_string(),"local":local}),
            ));
        }
        // The push itself is verified; a tracking ref that cannot be moved
        // is reported in the ready envelope, not turned into a push failure.
        tracking_error = record_verified_push(at, &url, branch, &local)
            .err()
            .map(|e| diagnostic::sanitize(&e.to_string()));
    }
    if operation(at)? != op
        || !clean(at)?
        || git(at, &["rev-parse", "HEAD"])? != local
        || git(at, &["symbolic-ref", "-q", "HEAD"])? != source_ref
    {
        bail!("local worktree changed before continuation");
    }
    let mut ready = serde_json::to_value(Ready {
        schema_version: SCHEMA_VERSION,
        outcome: "ready",
        terminal_refusal: false,
        disposition: if local == remote {
            "synchronized"
        } else {
            "pushed"
        },
        worktree: at,
        head_sha: &local,
        head_matches: true,
    })?;
    if let Some(error) = tracking_error {
        ready["tracking_ref_error"] = json!(error);
    }
    Ok(ready)
}
/// Evaluate once, finalize refusals, and emit one versioned envelope.
pub(crate) fn inspect(mut r: PreflightRequest, cwd: &Path) -> Result<Value> {
    if r.pr == 0 || (r.repo != "." && !identity(&r.repo)) {
        bail!("invalid PR number or repository identity");
    }
    // The state directory comes first: a drive that cannot keep its records
    // privately must stop before it pushes or comments on anything. The rule
    // is the one autodrive_private_dir and the merge gate apply.
    if let Err(error) = validate_state_directory(&r.state_dir) {
        let mut v = refusal(
            RefusalOutcome::StateDirNotPrivate,
            json!({
                "state_dir": git_path(r.state_dir.as_os_str().as_encoded_bytes()),
                "diagnostic": diagnostic::sanitize(&error.to_string()),
                "remedy": "Inspect the state directory; it must be a real directory owned by this user with no group or world write bit, or choose another AUTODRIVE_STATE_DIR.",
            }),
        );
        v["pr"] = json!(r.pr);
        v["repository"] = json!(r.repo);
        return Ok(v);
    }
    let decision = (|| -> Result<Value> {
        if r.repo == "." {
            r.repo = String::from_utf8(output(
                cwd,
                "gh",
                &[
                    "repo",
                    "view",
                    "--json",
                    "nameWithOwner",
                    "--jq",
                    ".nameWithOwner",
                ],
            )?)?
            .trim()
            .to_owned();
            if !identity(&r.repo) {
                bail!("repository identity is unavailable");
            }
        }
        evaluate(&r, cwd)
    })();
    let mut v = decision.unwrap_or_else(|e| {
        refusal(
            RefusalOutcome::ComparisonFailed,
            json!({"diagnostic":e.to_string()}),
        )
    });
    v["pr"] = json!(r.pr);
    v["repository"] = json!(r.repo);
    if v["outcome"] != "ready" {
        let v = finalize_refusal(FinalizationContext { request: &r, cwd }, v)?;
        return Ok(v.envelope);
    }
    Ok(v)
}

/// CLI adapter; the runner uses the same in-process decision directly.
///
/// A refusal is also handed to the `recipe run` that started this step, so
/// its final event names the outcome even when the loop ran as a sub-recipe
/// of auto-drive-to-merge (see `autodrive_runner::read_handoff`).
pub fn run(r: PreflightRequest) -> Result<()> {
    let v = inspect(r, &std::env::current_dir()?)?;
    println!("{v}");
    if v["terminal_refusal"] == true {
        if v["outcome"] == RefusalOutcome::StateDirNotPrivate.as_str() {
            eprintln!(
                "ERROR: state-dir-not-private: {}",
                v["evidence"]["diagnostic"].as_str().unwrap_or("")
            );
        }
        if let Err(error) = super::autodrive_runner::write_handoff(&v) {
            eprintln!(
                "WARNING: the preflight refusal could not be handed to the recipe run: {}",
                diagnostic::sanitize(&error.to_string())
            );
        }
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_2188_git_paths_preserve_invalid_utf8_and_rename_endpoints() {
        use base64::Engine;
        let paths = status_paths(b"R  destination\0source\xff\0?? line\n\tname\0").unwrap();
        assert_eq!(paths[0], json!({"encoding":"utf-8","value":"destination"}));
        assert_eq!(paths[1]["encoding"], "base64");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(paths[1]["value"].as_str().unwrap())
                .unwrap(),
            b"source\xff"
        );
        assert_eq!(paths[2]["value"], "line\n\tname");
        assert!(status_paths(b"R  destination\0").is_err());
    }
}
