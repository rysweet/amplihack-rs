//! Filesystem ownership is independent of the caller's execution directory.
use super::{
    models::{PersistedState, Workstream, WorkstreamConfig},
    utils::atomic_write,
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{fs, path::Path};

/// Keep this file open throughout execution or cleanup.
pub(super) fn lock(base: &Path) -> Result<fs::File> {
    private_directory(base)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).read(true).write(true);
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
    if path.exists() {
        let old: Vec<WorkstreamConfig> = serde_json::from_slice(&fs::read(&path)?)?;
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
    atomic_write(&path, &serde_json::to_vec_pretty(items)?)
}

pub(super) fn validate_saved(ws: &Workstream, saved: &PersistedState) -> Result<()> {
    ensure!(
        saved.key == ws.key.as_str()
            || (saved.key.is_empty() && saved.issue == ws.issue && ws.issue.is_some()),
        "[{}] saved identity does not match",
        ws.key
    );
    ensure!(
        saved.task == ws.task && saved.branch == ws.branch && saved.recipe == ws.recipe,
        "[{}] saved task, branch or route does not match",
        ws.key
    );
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
        !matches!(
            saved.lifecycle_state.as_str(),
            "launching" | "running" | "timed_out_resumable"
        ) || saved.last_exit_code.is_some(),
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
        "execution_directory": ws.work_dir, "artefact_directory": ws.artefact_dir});
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "[{}] unsafe ownership marker",
                ws.key
            );
            let recorded: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
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
