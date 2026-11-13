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
#[path = "../../../run_isolation/exec_storage.rs"]
pub(crate) mod exec_storage;

pub(crate) const RUN_CHECKOUT_ENV: &str = "AMPLIHACK_RUN_CHECKOUT";

/// The stable path an attempt with a recovery identity prepares at, the run
/// folder that claims it, and how to tell whether an earlier owner has ended.
#[derive(Clone, Copy)]
pub(crate) struct StableStorage<'a> {
    pub(crate) path: &'a Path,
    pub(crate) owner_run_dir: &'a Path,
    pub(crate) owner_ended: exec_storage::OwnerEnded,
}

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
        Self::prepare_with_inputs(working_dir, scratch, dry_run, &[], None)
    }

    /// `stable_storage`, when given, is where this attempt's private
    /// repository and checkout are created: a path derived from the run's
    /// recovery identity, so a relaunch prepares its fresh checkout at the same
    /// path and replay can match checkout-dependent steps. The attempt claims
    /// it through [`exec_storage::claim`], which reclaims a path an earlier
    /// attempt left once that attempt's run has ended and no durable worktree
    /// uses it. When the path is still held, this attempt says why and uses a
    /// fresh random path instead.
    pub(crate) fn prepare_with_inputs(
        working_dir: &Path,
        scratch: &Path,
        dry_run: bool,
        inputs: &[PathBuf],
        stable_storage: Option<StableStorage<'_>>,
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
        let storage = match stable_storage {
            Some(stable) => {
                match exec_storage::claim(stable.path, stable.owner_run_dir, stable.owner_ended)? {
                    exec_storage::Claim::Claimed(path) => Some(path),
                    exec_storage::Claim::Held(reason) => {
                        eprintln!(
                            "amplihack: {} is still held by an earlier attempt ({reason}); this \
                             attempt uses a fresh execution path, so recovery replay re-runs \
                             steps that depend on the checkout",
                            stable.path.display()
                        );
                        None
                    }
                }
            }
            None => None,
        };
        let storage = match storage {
            Some(storage) => storage,
            None => tempfile::Builder::new()
                .prefix("execution-")
                .tempdir_in(parent)?
                .keep(),
        };
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
        let mut copied_origin = false;
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
                copied_origin |= key == "remote.origin.url";
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
        if copied_origin {
            seed_origin(&caller_top, &repository).map_err(|error| {
                anyhow::anyhow!(
                    "cannot seed origin refs in the private repository: {error:#}; partial resources retained at {}",
                    storage.display()
                )
            })?;
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
            // A symlink alias of the caller's checkout names the same files, so
            // an existing absolute path is compared by its canonical form.
            let canonical = Path::new(value)
                .is_absolute()
                .then(|| Path::new(value).canonicalize().ok())
                .flatten();
            let path = match &canonical {
                Some(resolved) if resolved.starts_with(caller_top) => resolved.as_path(),
                _ => Path::new(value),
            };
            let replacement = if path.is_absolute() && path.starts_with(caller_top) {
                refuse_parent_dir(path, &format!("context path {key}"))?;
                root.join(path.strip_prefix(caller_top)?)
                    .into_os_string()
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("context path {key} is not UTF-8"))?
            } else {
                if caller_top
                    .to_str()
                    .is_some_and(|prefix| embeds_path(value, prefix))
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
            refuse_parent_dir(&absolute, &format!("path {}", absolute.display()))?;
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
/// Gives the private repository the caller's `origin` view, so recipes that
/// resolve a base from `origin/HEAD` or `origin/<branch>` find one: the normal
/// fetch refspec, the caller's `refs/remotes/origin/*` (one local fetch, so
/// every object they name is present), and `origin/HEAD` when the caller has
/// it as a symbolic ref to a resolvable `refs/remotes/origin/*` target.
/// Nothing is fetched over the network, and no remote URL is printed.
fn seed_origin(caller_top: &Path, repository: &Path) -> Result<()> {
    let output = preparation_command(repository)
        .args([
            "config",
            "--local",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ])
        .output()?;
    if !output.status.success() {
        bail!("cannot set the origin fetch refspec");
    }
    let refs = git(
        caller_top,
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "refs/remotes/origin/",
        ],
    )?;
    if trim_lf(&refs).is_empty() {
        // The recipe's own `git fetch origin` fills them through the refspec.
        return Ok(());
    }
    let output = preparation_command(repository)
        .env("GIT_ALLOW_PROTOCOL", "file")
        .args(["fetch", "--no-tags", "--quiet", "--"])
        .arg(caller_top)
        .arg("+refs/remotes/origin/*:refs/remotes/origin/*")
        .output()?;
    if !output.status.success() {
        bail!(
            "local fetch failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let head = preparation_command(caller_top)
        .args(["symbolic-ref", "-q", "refs/remotes/origin/HEAD"])
        .output()?;
    let target = match head.status.code() {
        Some(0) => String::from_utf8(trim_lf(&head.stdout).to_vec())
            .context("the caller's origin/HEAD target is not UTF-8")?,
        // Absent, or not a symbolic ref: never invent a default branch.
        Some(1) => return Ok(()),
        _ => bail!(
            "cannot read the caller's origin/HEAD: {}",
            String::from_utf8_lossy(&head.stderr).trim()
        ),
    };
    let well_formed = target.starts_with("refs/remotes/origin/")
        && !target.chars().any(|c| c.is_whitespace() || c.is_control())
        && preparation_command(repository)
            .args(["check-ref-format", &target])
            .output()?
            .status
            .success();
    let resolves = well_formed
        && preparation_command(repository)
            .args(["rev-parse", "--verify", "--quiet", &target])
            .output()?
            .status
            .success();
    if !resolves {
        eprintln!(
            "amplihack: warning: the caller's origin/HEAD does not name a resolvable refs/remotes/origin/ branch; the private checkout has no origin/HEAD"
        );
        return Ok(());
    }
    let output = preparation_command(repository)
        .args(["symbolic-ref", "refs/remotes/origin/HEAD", &target])
        .output()?;
    if !output.status.success() {
        bail!(
            "cannot set origin/HEAD: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}
/// A caller path inside the checkout must not climb out of it with `..`:
/// joined onto the private root, it would resolve outside the checkout.
fn refuse_parent_dir(path: &Path, what: &str) -> Result<()> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        bail!("{what} contains '..'; use a canonical repository path");
    }
    Ok(())
}
/// Whether `text` contains `root` as a whole path: followed by `/` or by the
/// end of a path (end of text, whitespace, a quote or another delimiter), not
/// as the prefix of a longer name such as `<root>-alias`.
fn embeds_path(text: &str, root: &str) -> bool {
    text.match_indices(root).any(|(at, _)| {
        text[at + root.len()..]
            .chars()
            .next()
            .is_none_or(|next| !(next.is_alphanumeric() || matches!(next, '-' | '_' | '.')))
    })
}
