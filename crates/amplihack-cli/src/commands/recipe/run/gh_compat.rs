//! Issue #1484 — route the run's `gh` through the bundle's compatibility layer.
//!
//! Claude Code on the web sessions reach GitHub through a proxy that serves
//! REST but refuses GraphQL, which every `gh issue` / `gh pr` / `gh label`
//! subcommand uses. The fix for that lives in ONE shell file,
//! `amplifier-bundle/tools/workflow_gh_compat.sh`; this module only makes every
//! step, helper and agent of a recipe run reach it, by writing a tiny `gh`
//! launcher into the run's private runtime directory and putting that
//! directory first on the runner's PATH.
//!
//! The launcher is only installed when a real `gh` is already on PATH, so a
//! host without `gh` keeps seeing "no gh" exactly as before. The script itself
//! passes straight through to the real `gh` unless GraphQL is blocked. Set
//! `AMPLIHACK_GH_COMPAT=0` to opt out entirely.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Bundle-relative path of the compatibility script.
pub(super) const GH_COMPAT_SCRIPT: &str = "amplifier-bundle/tools/workflow_gh_compat.sh";

/// Marker file that identifies a launcher directory. The script skips any PATH
/// entry carrying it when it looks for the real `gh`, so nested runs (each of
/// which prepends its own launcher) never recurse into one another.
pub(super) const GH_COMPAT_MARKER: &str = ".amplihack-gh-compat";

/// Write the launcher under `runtime_dir` and return the directory to prepend
/// to PATH, or `None` when the layer should not (or cannot) be installed.
pub(super) fn install_gh_compat_launcher(
    runtime_dir: &Path,
    amplihack_home: Option<&str>,
    path: Option<&OsStr>,
) -> Option<PathBuf> {
    if !cfg!(unix) || std::env::var("AMPLIHACK_GH_COMPAT").is_ok_and(|v| v.trim() == "0") {
        return None;
    }
    let script = Path::new(amplihack_home.filter(|h| !h.is_empty())?).join(GH_COMPAT_SCRIPT);
    if !script.is_file() || !real_gh_on_path(path?) {
        return None;
    }

    let dir = runtime_dir.join("gh-compat");
    match write_launcher(&dir, &script) {
        Ok(()) => Some(dir),
        Err(error) => {
            tracing::warn!(%error, dir = %dir.display(), "gh-compat launcher not installed");
            None
        }
    }
}

/// True when some PATH entry that is not itself a launcher holds a `gh` file.
fn real_gh_on_path(path: &OsStr) -> bool {
    std::env::split_paths(path).any(|dir| {
        !dir.as_os_str().is_empty()
            && !dir.join(GH_COMPAT_MARKER).exists()
            && dir.join("gh").is_file()
    })
}

fn write_launcher(dir: &Path, script: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(GH_COMPAT_MARKER), "")?;
    let quoted = script.to_string_lossy().replace('\'', r"'\''");
    let launcher = dir.join("gh");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\n# amplihack gh-compat launcher (issue #1484), generated for one recipe run.\nexec bash '{quoted}' \"$@\"\n"
        ),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle_with_script() -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        let script = home.path().join(GH_COMPAT_SCRIPT);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "#!/usr/bin/env bash\n").unwrap();
        home
    }

    fn dir_with_gh() -> tempfile::TempDir {
        let bin = tempfile::tempdir().unwrap();
        std::fs::write(bin.path().join("gh"), "").unwrap();
        bin
    }

    #[test]
    #[cfg(unix)]
    fn installs_launcher_that_execs_the_bundle_script() {
        let home = bundle_with_script();
        let bin = dir_with_gh();
        let runtime = tempfile::tempdir().unwrap();
        let dir = install_gh_compat_launcher(
            runtime.path(),
            Some(home.path().to_str().unwrap()),
            Some(bin.path().as_os_str()),
        )
        .expect("launcher installed when gh and the script both exist");

        assert!(dir.join(GH_COMPAT_MARKER).exists());
        let launcher = std::fs::read_to_string(dir.join("gh")).unwrap();
        let script = home.path().join(GH_COMPAT_SCRIPT);
        assert!(launcher.contains(&format!("exec bash '{}' \"$@\"", script.display())));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("gh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "launcher must be executable");
    }

    #[test]
    fn skipped_without_a_real_gh() {
        let home = bundle_with_script();
        let empty = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let path = std::env::join_paths([empty.path()]).unwrap();
        assert!(
            install_gh_compat_launcher(
                runtime.path(),
                Some(home.path().to_str().unwrap()),
                Some(&path)
            )
            .is_none()
        );
    }

    #[test]
    fn a_launcher_dir_does_not_count_as_a_real_gh() {
        let home = bundle_with_script();
        let launcher_only = dir_with_gh();
        std::fs::write(launcher_only.path().join(GH_COMPAT_MARKER), "").unwrap();
        let runtime = tempfile::tempdir().unwrap();
        assert!(
            install_gh_compat_launcher(
                runtime.path(),
                Some(home.path().to_str().unwrap()),
                Some(launcher_only.path().as_os_str())
            )
            .is_none()
        );
    }

    #[test]
    fn skipped_when_the_bundle_lacks_the_script() {
        let home = tempfile::tempdir().unwrap();
        let bin = dir_with_gh();
        let runtime = tempfile::tempdir().unwrap();
        assert!(
            install_gh_compat_launcher(
                runtime.path(),
                Some(home.path().to_str().unwrap()),
                Some(bin.path().as_os_str())
            )
            .is_none()
        );
        assert!(
            install_gh_compat_launcher(runtime.path(), None, Some(bin.path().as_os_str()))
                .is_none()
        );
    }
}
