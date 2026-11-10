//! Protected refusal persistence and one informational PR publication.
use super::{PreflightRequest, RefusalOutcome, identity, output};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) struct FinalizedRefusal {
    pub(super) envelope: Value,
}

pub(crate) fn validate_state_directory(dir: &Path) -> Result<()> {
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(dir)
    };
    let mut prefix = PathBuf::new();
    for component in absolute.components() {
        prefix.push(component);
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                // macOS exposes these system-owned aliases in TMPDIR.
                #[cfg(target_os = "macos")]
                if metadata.uid() == 0
                    && ((prefix == Path::new("/var")
                        && fs::canonicalize(&prefix)? == Path::new("/private/var"))
                        || (prefix == Path::new("/tmp")
                            && fs::canonicalize(&prefix)? == Path::new("/private/tmp")))
                {
                    continue;
                }
                bail!("state directory has a symlink component")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    #[cfg(unix)]
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if !dir.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder.create(dir)?;
    }
    let m = fs::symlink_metadata(dir)?;
    if !m.is_dir() || m.file_type().is_symlink() {
        bail!("unsafe state directory");
    }
    #[cfg(unix)]
    if m.mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
        bail!(
            "state directory must be owned by current user with owner-only permissions (chmod 700 after inspecting the directory)"
        );
    }
    Ok(())
}
fn persist(dir: &Path, value: &Value) -> Result<()> {
    validate_state_directory(dir)?;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    #[cfg(unix)]
    {
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        };
        let directory = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(dir)?;
        let m = directory.metadata()?;
        if m.mode() & 0o077 != 0 || m.uid() != unsafe { libc::geteuid() } {
            bail!("state directory changed ownership or permissions");
        }
        let name = std::ffi::CString::new(format!(".preflight-{}", uuid::Uuid::new_v4()))?;
        let target = c"preflight-result.json";
        let mut target_stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                target.as_ptr(),
                target_stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == 0
        {
            let target_stat = unsafe { target_stat.assume_init() };
            if target_stat.st_mode & libc::S_IFMT != libc::S_IFREG
                || target_stat.st_mode & 0o777 != 0o600
                || target_stat.st_uid != unsafe { libc::geteuid() }
            {
                bail!("unsafe existing preflight result");
            }
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
        // The directory descriptor pins the validated directory across both
        // creation and replacement, even if its pathname is renamed.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        let write = (|| -> Result<()> {
            serde_json::to_writer(&mut file, value)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            if unsafe {
                libc::renameat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    directory.as_raw_fd(),
                    target.as_ptr(),
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            directory.sync_all()?;
            Ok(())
        })();
        if write.is_err() && unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0
        {
            let cleanup = std::io::Error::last_os_error();
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                return write.context(format!("temporary state cleanup also failed: {cleanup}"));
            }
        }
        write?;
    }
    #[cfg(not(unix))]
    {
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(&mut tmp, value)?;
        tmp.write_all(b"\n")?;
        tmp.as_file().sync_all()?;
        tmp.persist(dir.join("preflight-result.json"))?;
    }
    Ok(())
}

pub(super) struct FinalizationContext<'a> {
    pub(super) request: &'a PreflightRequest,
    pub(super) cwd: &'a Path,
}

pub(super) fn finalize_refusal(
    context: FinalizationContext<'_>,
    mut v: Value,
) -> Result<FinalizedRefusal> {
    super::diagnostic::sanitize_value(&mut v);
    let r = context.request;
    let cwd = context.cwd;

    v["persistence"] = json!({"initial_record_written":true});
    if let Err(e) = persist(&r.state_dir, &v) {
        v["persistence_error"] = json!(super::diagnostic::sanitize(&e.to_string()));
        v["persistence"]["initial_record_written"] = json!(false);
    }
    let outcome: RefusalOutcome = serde_json::from_value(v["outcome"].clone())?;
    let remedy = match outcome {
        RefusalOutcome::Interrupted => {
            "Inspect git status and deliberately complete or abandon the interrupted operation after preserving the work."
        }
        RefusalOutcome::Dirty => {
            "Review and preserve the changes, then commit or deliberately stash them before restarting."
        }
        RefusalOutcome::Behind => {
            "Fetch and inspect the source history, then deliberately fast-forward the clean PR worktree."
        }
        RefusalOutcome::Diverged => {
            "Inspect both histories and deliberately reconcile them under repository policy; preserve local commits."
        }
        RefusalOutcome::ComparisonFailed => {
            "Restore repository access and complete history availability before comparing again."
        }
        RefusalOutcome::PushFailed => {
            "Inspect hook, transport, rejection and verification diagnostics. A push may have succeeded; inspect both heads before restarting."
        }
        RefusalOutcome::NotFound => {
            "Establish the intended PR branch worktree deliberately before restarting."
        }
        RefusalOutcome::Ambiguous => {
            "Inspect all candidates and leave one uniquely identifiable PR worktree; preserve all recovery state."
        }
        RefusalOutcome::BranchMismatch => {
            "Correct existing_branch to the PR source branch or omit it."
        }
    };
    let mut body = format!(
        "Auto-drive stopped: {}. {remedy} Detailed evidence is in the protected preflight result. No history was reconciled.",
        outcome.as_str()
    );
    if let Some(operations) = v["evidence"]["operations"].as_array() {
        let names: Vec<_> = operations
            .iter()
            .filter_map(|o| o["operation"].as_str())
            .collect();
        body.push_str(&format!(" Operations: {}.", names.join(", ")));
    }
    if v["evidence"]["no_unmerged_paths"] == true {
        body.push_str(" No unmerged paths were found; operation metadata still requires recovery.");
    }
    for key in ["paths", "unmerged_paths"] {
        if let Some(paths) = v["evidence"][key].as_array() {
            for path in paths.iter().take(30) {
                let name = path["value"]
                    .as_str()
                    .filter(|_| path["encoding"] == "utf-8")
                    .and_then(|p| Path::new(p).file_name())
                    .and_then(|p| p.to_str())
                    .map(|p| p.escape_default().to_string())
                    .unwrap_or_else(|| "[encoded path in protected record]".into());
                body.push_str(&format!("\nAffected path: {name}"));
            }
        }
    }
    let body = super::diagnostic::sanitize(&body);
    let pr = r.pr.to_string();
    let mut comment = vec!["pr", "comment", &pr, "--body", &body];
    if identity(&r.repo) {
        comment.extend(["--repo", &r.repo]);
    }
    if let Err(e) = output(cwd, "gh", &comment) {
        v["publication_error"] = json!(super::diagnostic::sanitize(&e.to_string()));
    }
    if let Err(e) = persist(&r.state_dir, &v) {
        v["persistence_error"] = json!(super::diagnostic::sanitize(&e.to_string()));
    }

    Ok(FinalizedRefusal { envelope: v })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn issue_2188_only_owner_private_directories_preserve_recovery_records() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("state");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        persist(&dir, &json!({"recovery":"preserve"})).unwrap();
        let original = fs::read(dir.join("preflight-result.json")).unwrap();
        for mode in [0o755, 0o750, 0o770, 0o777, 0o707] {
            fs::set_permissions(&dir, fs::Permissions::from_mode(mode)).unwrap();
            assert!(persist(&dir, &json!({"replacement":true})).is_err());
            assert_eq!(
                fs::read(dir.join("preflight-result.json")).unwrap(),
                original
            );
            assert_eq!(
                fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                mode
            );
        }
    }
}
