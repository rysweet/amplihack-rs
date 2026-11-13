//! Filesystem ownership is independent of the caller's execution directory.
use super::{
    models::{PersistedState, Workstream, WorkstreamConfig},
    utils::atomic_write,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs, path::Path};

/// Local Git identity: common repository plus the originating worktree. Remote
/// URLs are deliberately absent: they are neither unique nor credential-free.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryIdentity {
    common_dir: std::path::PathBuf,
    git_dir: std::path::PathBuf,
    worktree: std::path::PathBuf,
}

impl RepositoryIdentity {
    pub(super) fn current() -> Result<Self> {
        let mut command = amplihack_git::command();
        command.args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--git-dir",
            "--show-toplevel",
        ]);
        let output =
            crate::util::run_output_with_timeout(command, std::time::Duration::from_secs(30))
                .context("read source Git identity")?;
        ensure!(
            output.status.success(),
            "cannot establish source Git identity; run from the originating worktree"
        );
        let text = String::from_utf8(output.stdout)?;
        let paths: Vec<_> = text
            .lines()
            .map(fs::canonicalize)
            .collect::<std::io::Result<_>>()?;
        ensure!(paths.len() == 3, "invalid source Git identity");
        Ok(Self {
            common_dir: paths[0].clone(),
            git_dir: paths[1].clone(),
            worktree: paths[2].clone(),
        })
    }
}

/// Read only regular, non-symlink ownership inputs in the private runtime.
fn read_owned(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "unsafe ownership input {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Keep this file open throughout execution or cleanup.
pub(super) fn lock(base: &Path) -> Result<fs::File> {
    private_directory(base)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(base.join("execution.lock"))?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        ensure!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "runtime directory is already in use; choose a private TMPDIR or wait for its owner to finish"
        );
    }
    Ok(file)
}

pub(super) fn private_directory(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "unsafe runtime directory {}",
            path.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                meta.uid() == unsafe { libc::geteuid() },
                "foreign runtime owner: {}",
                path.display()
            );
            ensure!(
                meta.mode() & 0o077 == 0,
                "runtime directory must be private: {}",
                path.display()
            );
        }
    } else {
        if let Some(parent) = path.parent() {
            ensure!(
                parent.is_dir(),
                "missing runtime parent {}",
                parent.display()
            );
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .with_context(|| format!("create private directory {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn persist_plan(items: &[WorkstreamConfig], state_dir: &Path) -> Result<()> {
    private_directory(state_dir)?;
    let path = state_dir.join("canonical-plan.json");
    let identity = RepositoryIdentity::current()?;
    let identity_path = state_dir.join("repository.json");
    if path.try_exists()? || identity_path.try_exists()? {
        let recorded: RepositoryIdentity = serde_json::from_slice(&read_owned(&identity_path)
            .context("saved plan lacks source repository identity; preserve it and use a fresh runtime directory")?)?;
        ensure!(
            recorded == identity,
            "saved plan belongs to another repository or source worktree; use its originating checkout or a fresh runtime directory"
        );
    }
    if path.exists() {
        let value: serde_json::Value = serde_json::from_slice(&read_owned(&path)?)?;
        let old: Vec<WorkstreamConfig> = serde_json::from_value(value.clone())?;
        let validated = super::plan::normalise(&value, "default-workflow")?;
        for (recorded, normalised) in old.iter().zip(&validated) {
            ensure!(
                recorded.key == normalised.key
                    && recorded.classification == normalised.classification
                    && recorded.recipe == normalised.recipe,
                "saved canonical plan has a conflicting identity or route; reconcile it before resuming"
            );
        }
        ensure!(
            old.len() == items.len(),
            "saved canonical plan has different leaves; use its saved configuration or a fresh runtime directory"
        );
        for (old, new) in old.iter().zip(items) {
            ensure!(
                old.key == new.key
                    && old.task == new.task
                    && old.branch == new.branch
                    && old.classification == new.classification
                    && old.recipe == new.recipe
                    && old.dependencies == new.dependencies
                    && old.create_issue == new.create_issue,
                "saved canonical plan conflicts with workstream {:?}",
                new.key
            );
            let previous = old.issue()?;
            let next = new.issue()?;
            ensure!(
                previous.is_none() || next.is_none() || previous == next,
                "conflicting issue attachment for {:?}",
                new.key
            );
        }
    }
    atomic_write(&identity_path, &serde_json::to_vec(&identity)?)?;
    atomic_write(&path, &serde_json::to_vec_pretty(items)?)
}

pub(super) fn validate_saved(ws: &Workstream, saved: &PersistedState) -> Result<()> {
    ensure!(
        saved.source_repository != RepositoryIdentity::default()
            && saved.source_repository == ws.source_repository,
        "[{}] saved state lacks compatible source repository/worktree identity; use the originating checkout or a fresh runtime directory",
        ws.key
    );
    ensure!(
        saved.key == ws.key.as_str()
            || (saved.key.is_empty() && saved.issue == ws.issue && ws.issue.is_some()),
        "[{}] saved identity does not match",
        ws.key
    );
    ensure!(
        saved.task == ws.task
            && saved.branch == ws.branch
            && saved.recipe == ws.recipe
            && saved.classification == ws.classification
            && saved.dependencies == ws.dependencies
            && saved.create_issue == ws.create_issue,
        "[{}] saved task, branch or route does not match",
        ws.key
    );
    ensure!(
        saved
            .timeout_policy
            .as_deref()
            .is_none_or(|p| matches!(p, "interrupt-preserve" | "continue-preserve")),
        "[{}] invalid saved timeout policy",
        ws.key
    );
    ensure!(
        saved.lifecycle_state != "completed" || saved.last_exit_code == Some(0),
        "[{}] saved success has no successful process outcome",
        ws.key
    );
    if !saved.worktree_path.is_empty() {
        let worktree = fs::canonicalize(&saved.worktree_path)
            .context("saved worktree no longer exists; reconcile recovery state")?;
        ensure!(
            worktree.starts_with(fs::canonicalize(&ws.artefact_dir)?),
            "[{}] saved worktree escapes owned execution directory",
            ws.key
        );
    }
    ensure!(
        saved.work_dir == ws.work_dir.to_string_lossy(),
        "[{}] saved execution directory does not match",
        ws.key
    );
    ensure!(
        saved.issue.is_none() || ws.issue.is_none() || saved.issue == ws.issue,
        "[{}] conflicting tracked issue",
        ws.key
    );
    // A PID without an owned Child and an event observer cannot authorise reuse.
    ensure!(
        saved.last_pid.is_none()
            && !matches!(saved.lifecycle_state.as_str(), "launching" | "running")
            && (saved.lifecycle_state != "timed_out_resumable" || saved.last_exit_code.is_some()),
        "[{}] saved live process cannot be safely observed; reconcile its ownership and exit before resuming",
        ws.key
    );
    Ok(())
}

fn marker(ws: &Workstream) -> std::path::PathBuf {
    ws.state_file.with_extension("owner.json")
}

pub(super) fn claim(ws: &Workstream) -> Result<()> {
    let path = marker(ws);
    let expected = json!({"key": ws.key.as_str(), "task": ws.task, "recipe": ws.recipe,
        "source_repository": ws.source_repository, "classification": ws.classification,
        "branch": ws.branch, "dependencies": ws.dependencies, "create_issue": ws.create_issue,
        "execution_directory": ws.work_dir, "artefact_directory": ws.artefact_dir});
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "[{}] unsafe ownership marker",
                ws.key
            );
            let recorded: serde_json::Value = serde_json::from_slice(&read_owned(&path)?)?;
            ensure!(
                recorded == expected,
                "[{}] incompatible runtime ownership",
                ws.key
            );
            private_directory(&ws.artefact_dir)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ensure!(
                !ws.artefact_dir.try_exists()? && fs::symlink_metadata(&ws.artefact_dir).is_err(),
                "[{}] refusing an unowned runtime directory {}",
                ws.key,
                ws.artefact_dir.display()
            );
            private_directory(&ws.artefact_dir)?;
            atomic_write(&path, &serde_json::to_vec(&expected)?)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub(super) fn prepare_clone(ws: &Workstream) -> Result<()> {
    claim(ws)?;
    fs::remove_dir(&ws.artefact_dir).context("owned clone destination is not empty")
}

pub(super) fn verify(ws: &Workstream) -> Result<()> {
    ensure!(
        marker(ws).is_file(),
        "[{}] missing runtime ownership",
        ws.key
    );
    claim(ws)
}
