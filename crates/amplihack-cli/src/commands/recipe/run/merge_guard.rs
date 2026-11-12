//! Issue #1551: auto-drive runs with a `gh` shim first on `PATH` that refuses
//! every merge not made by `autodrive_merge_gate.sh`.
//!
//! This module decides when the shim applies and finds it in the bundle. The
//! shim itself is `amplifier-bundle/tools/gh-merge-guard/gh`.

use crate::env_builder::EnvBuilder;
use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};

/// Set to `1` for a guarded run. Any non-empty inherited value keeps a nested
/// run guarded; no value switches the guard off.
pub(crate) const MERGE_GUARD_ENV: &str = "AMPLIHACK_MERGE_GUARD";

/// The shim's directory, relative to `AMPLIHACK_HOME`.
pub(crate) const GUARD_REL: &str = "amplifier-bundle/tools/gh-merge-guard";

/// The shim's directory, relative to the bundle.
const GUARD_IN_BUNDLE: &str = "tools/gh-merge-guard";

/// The `BASH_ENV` file in the guard directory. It puts the guard back first
/// on `PATH` in every non-interactive bash of the run, after a login shell's
/// profile has run.
pub(crate) const BASH_ENV_FILE: &str = "bash-env";

/// Where the user's own `BASH_ENV` is kept while the guard's is in place;
/// the guard's file sources it first.
pub(crate) const USER_BASH_ENV: &str = "AMPLIHACK_MERGE_GUARD_USER_BASH_ENV";

/// The files in the guard directory's parent (`tools/`) that the shim trusts
/// to merge: the gate, and every helper the gate sources, directly or through
/// another helper (`autodrive_state.sh` sources `autodrive_unmerged.sh`,
/// `autodrive_trust.sh` loads `autodrive_qa_scope.sh`, #1763, and that loads
/// `autodrive_qa_docs.sh`, #2219).
/// Replacing any of them is as good as replacing the shim, so they get the
/// shim's mode check.
pub(crate) const GATE_FILES: [&str; 7] = [
    "autodrive_merge_gate.sh",
    // Sourced by the gate, so it runs inside the process this shim trusts to
    // merge; it is mode-checked with the rest (#2056).
    "autodrive_merge_message.sh",
    "autodrive_qa_docs.sh",
    "autodrive_qa_scope.sh",
    "autodrive_state.sh",
    "autodrive_trust.sh",
    "autodrive_unmerged.sh",
];

/// Whether a run of the recipe `name`, read from `path`, gets the guard.
///
/// True for `auto-drive-to-merge` and any `autodrive-*` recipe, matched on
/// the `name:` or the file stem, and for any run that inherits a non-empty
/// [`MERGE_GUARD_ENV`].
pub(crate) fn guard_applies(name: &str, path: &Path, inherited: Option<&str>) -> bool {
    if inherited.is_some_and(|value| !value.is_empty()) {
        return true;
    }
    let is_auto_drive = |s: &str| s == "auto-drive-to-merge" || s.starts_with("autodrive-");
    is_auto_drive(name)
        || path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(is_auto_drive)
}

/// Keeps the guard first on `PATH` for an agent CLI launched inside a guarded
/// run (`inherited` is the launcher's [`MERGE_GUARD_ENV`]). The launcher may
/// prepend the agent binary's own directory, which can also hold the real
/// `gh`, so the guard is put back ahead of it. Fails closed like the runner.
pub(crate) fn keep_guard_first(
    env_builder: EnvBuilder,
    inherited: Option<&str>,
) -> Result<EnvBuilder> {
    if !inherited.is_some_and(|value| !value.is_empty()) {
        return Ok(env_builder);
    }
    let dir = guard_dir_for_home(env_builder.var("AMPLIHACK_HOME"))?;
    Ok(apply_guard(env_builder.prepend_path_first(&dir), &dir))
}

/// Exports the guard for a run whose `PATH` already has `dir` in front: the
/// marker, and `BASH_ENV` pointing at the guard's [`BASH_ENV_FILE`] so that a
/// shell whose profile reorders `PATH` puts the guard back first. A user
/// `BASH_ENV` other than the guard's own is kept in [`USER_BASH_ENV`].
pub(crate) fn apply_guard(env_builder: EnvBuilder, dir: &Path) -> EnvBuilder {
    let bash_env = dir.join(BASH_ENV_FILE).to_string_lossy().into_owned();
    let current = env_builder
        .var("BASH_ENV")
        .map(str::to_string)
        .or_else(|| std::env::var("BASH_ENV").ok())
        .filter(|v| !v.is_empty() && *v != bash_env);
    let env_builder = match current {
        Some(user) => env_builder.set(USER_BASH_ENV, user),
        None => env_builder,
    };
    env_builder
        .set("BASH_ENV", bash_env)
        .set(MERGE_GUARD_ENV, "1")
}

/// Exported with the bundle a guarded run pinned, for information only. It
/// is never read (issue #1954 S3): a value in the environment cannot choose
/// which guard a run trusts.
pub(crate) const BUNDLE_DIR_ENV: &str = "AMPLIHACK_BUNDLE_DIR";

/// [`locate_guard_dir`] for the run's `AMPLIHACK_HOME`, which must be set,
/// in the bundle [`pin_bundle`] pins for the run.
pub(crate) fn guard_dir_for_home(home: Option<&str>) -> Result<PathBuf> {
    let home = home.ok_or_else(|| {
        anyhow!(
            "AMPLIHACK_HOME is unset; refusing to run auto-drive without the merge guard (#1551)"
        )
    })?;
    let home = Path::new(home);
    if !home.is_absolute() {
        return locate_guard_dir(home);
    }
    locate_guard_dir_at(home, &pin_bundle(home)?.join(GUARD_IN_BUNDLE))
}

/// The bundle directory a run uses for its whole life (issue #1954 D5).
///
/// `home/amplifier-bundle` as given when it is a real directory (or absent,
/// so [`locate_guard_dir`] reports it). When it is a symlink, it is resolved
/// once, here, and the run keeps the release it named even after a later
/// install switches the link. The target must be a real directory inside
/// `home/releases/`, its release must have `.complete`, and the release root
/// and `releases/` must not be writable by group or others (S2). The guard
/// directory's own G11 checks run on the pinned tree too.
///
/// [`BUNDLE_DIR_ENV`] is never consulted (S3).
pub(crate) fn pin_bundle(home: &Path) -> Result<PathBuf> {
    use crate::commands::install::release_layout;
    let link = home.join("amplifier-bundle");
    let is_link = std::fs::symlink_metadata(&link).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link {
        return Ok(link);
    }
    let releases = release_layout::releases_dir(home);
    let Some(release_root) = release_layout::release_root_of(&releases, &link) else {
        bail!(
            "amplihack: {} links outside {}; refusing to trust its merge guard (#1954)",
            link.display(),
            releases.display()
        );
    };
    if !release_root.join(release_layout::COMPLETE_MARKER).is_file() {
        bail!(
            "amplihack: release {} has no {}; refusing to trust its merge guard (#1954)",
            release_root.display(),
            release_layout::COMPLETE_MARKER
        );
    }
    let pinned = std::fs::canonicalize(&link)
        .map_err(|e| anyhow!("amplihack: cannot resolve {}: {e} (#1954)", link.display()))?;
    if !std::fs::symlink_metadata(&pinned).is_ok_and(|m| m.is_dir()) {
        bail!(
            "amplihack: {} is not a directory; refusing to trust its merge guard (#1954)",
            pinned.display()
        );
    }
    // S2: every directory from the bundle up to `releases/`.
    for dir in [&pinned, &release_root, &releases] {
        if is_group_or_other_writable(dir) {
            bail!(
                "amplihack: {}, which holds the merge guard of this run, is writable by group or \
                 others; refusing to run auto-drive (#1954). Rerun `amplihack install`, or \
                 `chmod go-w` it.",
                dir.display()
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owned = std::fs::symlink_metadata(&release_root)
            .is_ok_and(|m| m.uid() == release_layout::euid());
        if !owned {
            bail!(
                "amplihack: release {} is not owned by this user; refusing to trust its merge \
                 guard (#1954)",
                release_root.display()
            );
        }
    }
    let dir = pinned.join(GUARD_IN_BUNDLE);
    if dir.join("gh").exists() {
        check_guard_modes(&dir)?;
    }
    Ok(pinned)
}

/// The guard directory under `home`, provided it holds an executable `gh`
/// file (symlinks followed) and its [`BASH_ENV_FILE`]. The directory is returned as given:
/// canonicalisation happens once, in [`pin_bundle`] at run start (issue #1954), and the
/// pinned path is the one used for `PATH`, `BASH_ENV` and their de-duplication.
///
/// There is no fallback location: a missing shim, a `home` that is not an
/// absolute path (which would resolve against the working directory), a
/// non-Unix host, or a host without `/proc` or `/bin/bash`
/// ([`check_guard_host`]), stops the run. So does a shim or guard directory that group or others can write
/// (G11): anyone with that access could replace the guard. The same holds
/// for the `tools/` directory above it and for each [`GATE_FILES`] entry that
/// exists there, since the shim lets the gate merge.
pub(crate) fn locate_guard_dir(home: &Path) -> Result<PathBuf> {
    locate_guard_dir_at(home, &home.join(GUARD_REL))
}

/// [`locate_guard_dir`] for the guard directory `dir` of the run's (possibly
/// pinned) bundle under `home`.
fn locate_guard_dir_at(home: &Path, dir: &Path) -> Result<PathBuf> {
    let dir = dir.to_path_buf();
    let shim = dir.join("gh");
    let bash_env = dir.join(BASH_ENV_FILE);
    if !home.is_absolute() {
        bail!(
            "amplihack: AMPLIHACK_HOME is not absolute ({}); refusing to run auto-drive without \
             the merge guard (#1551)",
            home.display()
        );
    }
    if !is_executable_file(&shim) {
        bail!(
            "amplihack: merge guard not found at {}; refusing to run auto-drive without it (#1551)",
            dir.display()
        );
    }
    // The shim is an extensionless bash script that reads /proc: off Unix it
    // is never the `gh` an agent runs, so the guard would be on in name only.
    if cfg!(not(unix)) {
        bail!(
            "amplihack: the merge guard needs a Unix host; refusing to run auto-drive on this \
             platform (#1551)"
        );
    }
    check_guard_host(Path::new(PROC_SELF_STAT), Path::new(SHIM_INTERPRETER))?;
    if !bash_env.is_file() {
        bail!(
            "amplihack: the merge guard's {} is missing; without it a shell profile that \
             reorders PATH puts another gh ahead of the guard. Refusing to run auto-drive \
             (#1551). Rerun `amplihack install`.",
            bash_env.display()
        );
    }
    check_guard_modes(&dir)?;
    Ok(dir)
}

/// The G11 owner-and-mode rule for the guard directory `dir`, its shim and
/// bash-env, `tools/` above it and each [`GATE_FILES`] entry there.
fn check_guard_modes(dir: &Path) -> Result<()> {
    let shim = dir.join("gh");
    let bash_env = dir.join(BASH_ENV_FILE);
    if is_group_or_other_writable(&shim)
        || is_group_or_other_writable(dir)
        || is_group_or_other_writable(&bash_env)
    {
        bail!(
            "amplihack: merge guard at {} is writable by group or others; refusing to trust it \
             (#1551). Rerun `amplihack install`, or chmod 0755 the directory and its gh.",
            dir.display()
        );
    }
    let tools = dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("amplihack: merge guard dir has no parent (#1551)"))?;
    let gate_files = GATE_FILES.iter().map(|name| tools.join(name));
    for path in std::iter::once(tools.clone()).chain(gate_files.filter(|p| p.exists())) {
        if is_group_or_other_writable(&path) {
            bail!(
                "amplihack: {}, which the merge guard trusts to merge, is writable by group or \
                 others; refusing to run auto-drive (#1551). Rerun `amplihack install`, or \
                 `chmod go-w` it.",
                path.display()
            );
        }
    }
    Ok(())
}

/// Read by the shim to trust the gate's own merge (`parent_is_gate`).
pub(crate) const PROC_SELF_STAT: &str = "/proc/self/stat";

/// The shim's `#!` interpreter, and the bash the gate must be run by.
pub(crate) const SHIM_INTERPRETER: &str = "/bin/bash";

/// Stops the run at launch on a host where the guard cannot work, instead of
/// after three phases of agent work.
///
/// * No `proc_stat` (macOS, BSD): the shim can never confirm its parent is
///   the gate, so it refuses the gate's own merge and the run could only
///   ever fail at the final merge.
/// * No executable `interpreter` (NixOS has no `/bin/bash`): the shim cannot
///   start, so every `gh` call in the run would fail, not only merges.
///
/// The paths are parameters so tests can point them at a missing file.
pub(crate) fn check_guard_host(proc_stat: &Path, interpreter: &Path) -> Result<()> {
    if std::fs::metadata(proc_stat).is_err() {
        bail!(
            "amplihack: auto-drive needs {} to let autodrive_merge_gate.sh merge, and this host \
             has none (macOS and the BSDs have no /proc). The merge guard would refuse the \
             gate's own merge after every phase had run, so auto-drive stops now (#1551). \
             Run auto-drive on Linux.",
            proc_stat.display()
        );
    }
    if !is_executable_file(interpreter) {
        bail!(
            "amplihack: the merge guard runs under {}, which is missing or not executable on \
             this host (NixOS has none), so every gh call in an auto-drive run would fail; \
             refusing to start auto-drive (#1551)",
            interpreter.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// Symlinks are followed: a link is judged by its target. A path whose mode
/// cannot be read counts as writable.
#[cfg(unix)]
fn is_group_or_other_writable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map_or(true, |meta| meta.permissions().mode() & 0o022 != 0)
}

#[cfg(not(unix))]
fn is_group_or_other_writable(_path: &Path) -> bool {
    false
}
