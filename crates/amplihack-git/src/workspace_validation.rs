//! Read-only checkout identity validation. No implicit repository creation.
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    common: PathBuf,
    git_dir: PathBuf,
}

impl Workspace {
    pub fn open(root: &Path) -> Result<Self> {
        let root = root.canonicalize().context("cannot resolve workspace")?;
        let top = run(&root, &["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(top.trim()).canonicalize()?;
        if top != root {
            bail!("workspace must be the root of an existing non-bare Git checkout");
        }
        if run(&root, &["rev-parse", "--is-bare-repository"])?.trim() != "false" {
            bail!("bare repositories cannot be workflow workspaces");
        }
        let common = PathBuf::from(
            run(
                &root,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )?
            .trim(),
        )
        .canonicalize()?;
        let git_dir = PathBuf::from(run(&root, &["rev-parse", "--absolute-git-dir"])?.trim())
            .canonicalize()?;
        Ok(Self {
            root,
            common,
            git_dir,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn common(&self) -> &Path {
        &self.common
    }
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }
    pub fn require_workflow(&self) -> Result<()> {
        if self.common.join("info/grafts").exists() {
            bail!("local Git grafts cannot define publication history");
        }
        let origin = run(&self.root, &["remote", "get-url", "origin"])
            .context("workflow requires an origin remote")?;
        if origin.strip_suffix('\n').unwrap_or(&origin).is_empty() {
            bail!("workflow requires a non-empty origin remote");
        }
        run(&self.root, &["rev-parse", "--verify", "HEAD^{commit}"])
            .context("workflow requires committed base history")?;
        if run(&self.root, &["rev-parse", "--is-shallow-repository"])?.trim() != "false" {
            bail!("workflow requires complete history");
        }
        Ok(())
    }
}
pub(crate) fn bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = crate::command_in(root)
        .args(args)
        .output()
        .context("cannot execute Git")?;
    if !out.status.success() {
        bail!(
            "Git {} failed: {}",
            args.first().unwrap_or(&"command"),
            crate::publication_validation::redact(&String::from_utf8_lossy(&out.stderr))
        );
    }
    Ok(out.stdout)
}
pub(crate) fn run(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(bytes(root, args)?).context("Git returned an unsupported non-UTF-8 name")
}

/// Discover checkout identity from an invocation directory, preserving linked worktrees.
pub fn checkout_root(invocation: &Path) -> Result<PathBuf> {
    let top = run(invocation, &["rev-parse", "--show-toplevel"])?;
    Ok(Workspace::open(Path::new(top.trim()))?.root)
}
