//! Owned execution checkouts. Independent invocations never reuse a lease.
//! This isolates Git state; it does not sandbox arbitrary same-user file I/O.
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../../run_isolation/input_snapshot.rs"]
mod input_snapshot;
use input_snapshot::{InputSnapshot, trim_lf};

pub(crate) const RUN_CHECKOUT_ENV: &str = "AMPLIHACK_RUN_CHECKOUT";

/// A non-cloneable lease. Repository metadata lives outside disposable runtime/.
#[derive(Debug)]
pub(crate) enum RunCheckout {
    Caller(PathBuf),
    Private {
        exec_dir: PathBuf,
        root: PathBuf,
        caller_top: PathBuf,
        repository: PathBuf,
        storage: PathBuf,
        owner: String,
        identities: [(u64, u64); 3],
        released: bool,
    },
}

impl RunCheckout {
    #[cfg(test)]
    #[allow(
        dead_code,
        reason = "direct component contracts import this source module separately"
    )]
    pub(crate) fn prepare(working_dir: &Path, scratch: &Path, dry_run: bool) -> Result<Self> {
        Self::prepare_with_inputs(working_dir, scratch, dry_run, &[])
    }

    pub(crate) fn prepare_with_inputs(
        working_dir: &Path,
        scratch: &Path,
        dry_run: bool,
        inputs: &[PathBuf],
    ) -> Result<Self> {
        if dry_run {
            return Ok(Self::Caller(
                working_dir
                    .canonicalize()
                    .context("resolve dry-run working directory")?,
            ));
        }
        let working_dir = working_dir
            .canonicalize()
            .context("resolve recipe working directory")?;
        let probe = preparation_command(&working_dir)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .context(
                "cannot execute Git for recipe checkout discovery; install Git before retrying",
            )?;
        if !probe.status.success() {
            // Absence is established by both Git's diagnostic and an ancestor
            // walk. A corrupt gitfile, inaccessible ancestor or missing Git is
            // never permission to run in the caller's directory.
            for ancestor in working_dir.ancestors() {
                match fs::symlink_metadata(ancestor.join(".git")) {
                    Ok(metadata)
                        if metadata.is_dir()
                            && fs::read_dir(ancestor.join(".git"))?.next().is_none() => {}
                    Ok(_) => bail!(
                        "cannot inspect Git metadata at {}: {}",
                        ancestor.display(),
                        String::from_utf8_lossy(&probe.stderr).trim()
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e).context("cannot establish repository absence"),
                }
            }
            if !String::from_utf8_lossy(&probe.stderr).contains("not a git repository") {
                bail!(
                    "Git repository discovery failed: {}",
                    String::from_utf8_lossy(&probe.stderr).trim()
                );
            }
            return Ok(Self::Caller(working_dir));
        }
        if trim_lf(&probe.stdout) != b"true" {
            bail!("recipe execution requires a working tree, not a bare repository");
        }
        let caller_top =
            path_output(git(&working_dir, &["rev-parse", "--show-toplevel"])?)?.canonicalize()?;
        let relative = working_dir
            .strip_prefix(&caller_top)
            .context("working directory outside discovered checkout")?;
        if std::env::var_os(RUN_CHECKOUT_ENV).is_some() {
            eprintln!(
                "amplihack: ignoring inherited {RUN_CHECKOUT_ENV}; this independent invocation owns a fresh checkout"
            );
        }
        let before = InputSnapshot::capture(&caller_top, inputs)?;
        let parent = scratch
            .parent()
            .context("runtime directory has no parent")?;
        let storage = tempfile::Builder::new()
            .prefix("execution-")
            .tempdir_in(parent)?
            .keep();
        let repository = storage.join("repository.git");
        let root = storage.join("checkout");
        let owner = uuid::Uuid::new_v4().to_string();
        // No caller worktree registration, object alternates or hardlinks.
        // --no-local forces object transfer instead of copying object files.
        let output = preparation_command(&caller_top)
            .args([
                "clone",
                "--bare",
                "--no-local",
                "--no-hardlinks",
                "--quiet",
                "--",
            ])
            .arg(&caller_top)
            .arg(&repository)
            .output()?;
        if !output.status.success() {
            bail!(
                "cannot create private recipe repository (partial resources retained at {}): {}",
                storage.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let head = std::str::from_utf8(trim_lf(&before.head))?;
        let output = preparation_command(&repository)
            .args(["worktree", "add", "--detach", "--no-checkout", "--quiet"])
            .arg(&root)
            .arg(head)
            .output()?;
        if !output.status.success() {
            bail!(
                "cannot create detached execution checkout: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        // Start with an empty index: installation is authoritative for staged
        // deletions as well as staged additions and working-tree deletions.
        git(&root, &["read-tree", "--empty"])?;
        before.install(&caller_top, &root)?;
        if InputSnapshot::capture(&caller_top, inputs)? != before {
            bail!(
                "caller inputs changed during snapshot; private evidence retained at {}. Retry when inputs are stable",
                storage.display()
            );
        }
        // Copy only benign commit identity and remote destinations. Caller
        // hooks, filters, include files and executable extensions never enter
        // the private configuration. Remote URLs are not emitted in logs.
        let removed_origin = preparation_command(&repository)
            .args(["config", "--remove-section", "remote.origin"])
            .output()?;
        if !removed_origin.status.success() && removed_origin.status.code() != Some(128) {
            bail!("cannot remove clone-local caller destination from isolated configuration");
        }
        for key in [
            "user.name",
            "user.email",
            "remote.origin.url",
            "remote.origin.pushurl",
        ] {
            let output = preparation_command(&caller_top)
                .args(["config", "--local", "--get", key])
                .output()?;
            if output.status.success() {
                let value = OsString::from_vec(trim_lf(&output.stdout).to_vec());
                let output = preparation_command(&repository)
                    .args(["config", "--local", key])
                    .arg(value)
                    .output()?;
                if !output.status.success() {
                    bail!("cannot set isolated Git configuration {key}");
                }
            } else if output.status.code() != Some(1) {
                bail!("cannot inspect caller Git configuration {key}");
            }
        }
        let identities = [
            identity(&storage)?,
            identity(&repository)?,
            identity(&root)?,
        ];
        let manifest = serde_json::json!({"schema_version":1,"owner":owner,"repository":repository,"checkout":root,"identities":identities,"head":head});
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(storage.join("owner.json"))?;
        file.write_all(&serde_json::to_vec(&manifest)?)?;
        file.sync_all()?;
        fs::File::open(&storage)?.sync_all()?;
        Ok(Self::Private {
            exec_dir: root.join(relative),
            root,
            caller_top,
            repository,
            storage,
            owner,
            identities,
            released: false,
        })
    }

    pub(crate) fn exec_dir(&self) -> &Path {
        match self {
            Self::Caller(dir) => dir,
            Self::Private { exec_dir, .. } => exec_dir,
        }
    }

    /// Map exact invocation paths; opaque text referring into the caller is
    /// rejected rather than edited with string substitution.
    pub(crate) fn map_context(
        &self,
        context: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>> {
        let Self::Private {
            caller_top, root, ..
        } = self
        else {
            return Ok(context.clone());
        };
        let mut mapped = BTreeMap::new();
        for (key, value) in context {
            let path = Path::new(value);
            let replacement = if path.is_absolute() && path.starts_with(caller_top) {
                if path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    bail!("context path {key} contains '..'; use a canonical repository path");
                }
                root.join(path.strip_prefix(caller_top)?)
                    .into_os_string()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("context path {key} is not UTF-8"))?
            } else {
                if caller_top
                    .to_str()
                    .is_some_and(|prefix| value.contains(prefix))
                {
                    bail!(
                        "context {key} embeds a caller-checkout path in opaque text; pass the path as a separate context value"
                    );
                }
                value.clone()
            };
            mapped.insert(key.clone(), replacement);
        }
        Ok(mapped)
    }

    pub(crate) fn map_path(&self, path: &Path) -> Result<PathBuf> {
        let Self::Private {
            caller_top, root, ..
        } = self
        else {
            return Ok(path.to_path_buf());
        };
        // The caller resolved invocation paths before changing directories.
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        if let Ok(relative) = absolute.strip_prefix(caller_top) {
            Ok(root.join(relative))
        } else {
            Ok(absolute)
        }
    }

    pub(crate) fn apply_to(&self, command: &mut Command) {
        amplihack_git::scrub(command);
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        match self {
            Self::Caller(_) => {
                command.env_remove(RUN_CHECKOUT_ENV);
            }
            Self::Private { root, .. } => {
                command.env(RUN_CHECKOUT_ENV, root);
            }
        }
    }

    /// Explicit, fallible release. Substituted resources are retained.
    pub(crate) fn remove(&mut self) -> Result<()> {
        let Self::Private {
            root,
            repository,
            storage,
            owner,
            identities,
            released,
            ..
        } = self
        else {
            return Ok(());
        };
        if *released {
            return Ok(());
        }
        for (path, expected) in [
            (&*storage, identities[0]),
            (&*repository, identities[1]),
            (&*root, identities[2]),
        ] {
            if identity(path)? != expected {
                bail!(
                    "execution checkout ownership identity changed at {}; resources retained",
                    path.display()
                );
            }
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(storage.join("owner.json"))?;
        let manifest: serde_json::Value = serde_json::from_reader(file)?;
        if manifest["schema_version"] != 1
            || manifest["owner"] != *owner
            || manifest["checkout"] != root.to_str().context("owned path encoding")?
            || manifest["repository"] != repository.to_str().context("repository path encoding")?
        {
            bail!("execution ownership manifest changed; resources retained");
        }
        let output = preparation_command(repository)
            .args(["worktree", "remove", "--force"])
            .arg(&*root)
            .output()?;
        if !output.status.success() {
            bail!(
                "cannot release execution checkout: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let list = git(repository, &["worktree", "list", "--porcelain", "-z"])?;
        let dependents = list
            .split(|b| *b == 0)
            .filter(|s| s.starts_with(b"worktree "))
            .count();
        // A workflow may have created durable worktrees in this repository.
        // Its metadata must survive until those dependencies are released.
        if dependents > 1 {
            fs::write(storage.join("durable-dependencies"), list)?;
        } else {
            fs::remove_dir_all(&*storage)?;
        }
        *released = true;
        Ok(())
    }
}
impl Drop for RunCheckout {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!("amplihack: execution checkout retained: {error:#}");
        }
    }
}
fn identity(path: &Path) -> Result<(u64, u64)> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("owned path is no longer a directory: {}", path.display());
    }
    Ok((metadata.dev(), metadata.ino()))
}
fn path_output(bytes: Vec<u8>) -> Result<PathBuf> {
    Ok(PathBuf::from(OsString::from_vec(trim_lf(&bytes).to_vec())))
}
/// Git used while preparing/releasing resources must not run caller code.
fn preparation_command(dir: &Path) -> Command {
    let mut command = amplihack_git::command_in(dir);
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.templateDir=",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
        ]);
    command
}
fn git(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = preparation_command(dir)
        .args(args)
        .output()
        .context("cannot execute Git")?;
    if !output.status.success() {
        bail!(
            "Git {} failed in {}: {}",
            args.first().unwrap_or(&"inspection"),
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}
