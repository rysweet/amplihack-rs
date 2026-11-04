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
    if !trusted_file(&script, false) {
        return None;
    }

    let real_gh = match std::env::var_os("AMPLIHACK_REAL_GH").filter(|p| !p.is_empty()) {
        Some(candidate) => terminal_gh(Path::new(&candidate)),
        None => terminal_gh_on_path(path?),
    }?;
    let dir = runtime_dir.join("gh-compat");
    match write_launcher(&dir, &script, &real_gh) {
        Ok(()) => Some(dir),
        Err(error) => {
            tracing::warn!(%error, dir = %dir.display(), "gh-compat launcher not installed");
            None
        }
    }
}

/// Resolve one executable transport before PATH gains this run's launcher.
/// Marked launchers, the merge guard and copies of either cannot be transport.
fn terminal_gh_on_path(path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path).find_map(|dir| {
        if !dir.is_absolute() || dir.join(GH_COMPAT_MARKER).exists() {
            return None;
        }
        terminal_gh(&dir.join("gh"))
    })
}

fn terminal_gh(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || path.parent()?.join(GH_COMPAT_MARKER).exists() {
        return None;
    }
    let candidate = path.canonicalize().ok()?;
    if candidate == std::env::current_exe().ok()? || !trusted_executable(&candidate) {
        return None;
    }
    // Inspect only a bounded header, never an arbitrary executable body.
    use std::io::Read;
    let mut header = Vec::new();
    std::fs::File::open(&candidate)
        .ok()?
        .take(4096)
        .read_to_end(&mut header)
        .ok()?;
    let header = String::from_utf8_lossy(&header);
    if header.contains("workflow_gh_compat.sh")
        || header.contains("amplihack gh-compat launcher")
        || header.contains("amplihack gh merge guard")
        || header.contains("amplihack gh-merge-guard")
    {
        return None;
    }
    Some(candidate)
}

fn trusted_executable(path: &Path) -> bool {
    trusted_file(path, true)
}

fn trusted_file(path: &Path, executable: bool) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments and no memory preconditions.
        let uid = unsafe { libc::geteuid() };
        let Ok(parent) = std::fs::metadata(path.parent().unwrap_or(Path::new("."))) else {
            return false;
        };
        meta.is_file()
            && (!executable || meta.mode() & 0o111 != 0)
            && meta.mode() & 0o022 == 0
            && (meta.uid() == 0 || meta.uid() == uid)
            && parent.is_dir()
            && parent.mode() & 0o022 == 0
            && (parent.uid() == 0 || parent.uid() == uid)
    }
    #[cfg(not(unix))]
    {
        let _ = executable;
        meta.is_file()
    }
}

fn write_launcher(dir: &Path, script: &Path, real_gh: &Path) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind, Write};
    let runtime = dir
        .parent()
        .ok_or_else(|| Error::other("missing runtime parent"))?;
    let runtime_meta = std::fs::symlink_metadata(runtime)?;
    if !runtime_meta.is_dir() || runtime_meta.file_type().is_symlink() {
        return Err(Error::other("unsafe gh-compat runtime directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no arguments and no memory preconditions.
        if runtime_meta.uid() != unsafe { libc::geteuid() } || runtime_meta.mode() & 0o077 != 0 {
            return Err(Error::other("gh-compat runtime directory is not private"));
        }
    }
    // create_dir (not create_dir_all) refuses an unexpected ancestor. A retry
    // can reuse exactly this run's private directory, never a symlink.
    match std::fs::create_dir(dir) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(dir)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(Error::other("unsafe gh-compat launcher directory"));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let parent = std::fs::metadata(
                    dir.parent()
                        .ok_or_else(|| Error::other("missing runtime parent"))?,
                )?;
                if meta.mode() & 0o077 != 0 || meta.uid() != parent.uid() {
                    return Err(Error::other("gh-compat launcher directory is not private"));
                }
            }
        }
        Err(e) => return Err(e),
    }
    let quote = |path: &Path| path.to_string_lossy().replace('\'', r"'\''");
    let quoted = quote(script);
    let terminal = quote(real_gh);
    let text = format!(
        "#!/bin/sh\n# amplihack gh-compat launcher (issue #1484), generated for one recipe run.\nexport AMPLIHACK_REAL_GH='{terminal}'\nexec /bin/bash '{quoted}' \"$@\"\n"
    );
    for (name, content) in [(GH_COMPAT_MARKER, ""), ("gh", text.as_str())] {
        let file = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
        {
            Ok(mut out) => {
                out.write_all(content.as_bytes())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    out.set_permissions(std::fs::Permissions::from_mode(if name == "gh" {
                        0o700
                    } else {
                        0o600
                    }))?;
                }
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let meta = std::fs::symlink_metadata(&file)?;
                if !meta.is_file()
                    || meta.file_type().is_symlink()
                    || !trusted_file(&file, name == "gh")
                    || std::fs::read(&file)? != content.as_bytes()
                {
                    return Err(Error::other(
                        "gh-compat launcher file already exists with different contents",
                    ));
                }
            }
            Err(e) => return Err(e),
        }
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
        let gh = bin.path().join("gh");
        std::fs::write(&gh, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
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
        assert!(launcher.contains(&format!("exec /bin/bash '{}' \"$@\"", script.display())));
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("gh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o100, "launcher must be executable");
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
    #[cfg(unix)]
    fn continuation_non_executable_gh_does_not_install_a_launcher() {
        use std::os::unix::fs::PermissionsExt;

        let home = bundle_with_script();
        let bin = dir_with_gh();
        std::fs::set_permissions(
            bin.path().join("gh"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let runtime = tempfile::tempdir().unwrap();

        assert!(
            install_gh_compat_launcher(
                runtime.path(),
                Some(home.path().to_str().unwrap()),
                Some(bin.path().as_os_str()),
            )
            .is_none(),
            "an ordinary non-executable file is not a terminal gh client"
        );
    }

    #[test]
    #[cfg(unix)]
    fn continuation_symlinked_launcher_directory_is_refused_without_writes() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let home = bundle_with_script();
        let bin = dir_with_gh();
        std::fs::set_permissions(
            bin.path().join("gh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let neighbour = tempfile::tempdir().unwrap();
        let sentinel = neighbour.path().join("gh");
        std::fs::write(&sentinel, "neighbour-owned sentinel\n").unwrap();
        symlink(neighbour.path(), runtime.path().join("gh-compat")).unwrap();

        let installed = install_gh_compat_launcher(
            runtime.path(),
            Some(home.path().to_str().unwrap()),
            Some(bin.path().as_os_str()),
        );

        assert_eq!(
            std::fs::read_to_string(&sentinel).unwrap(),
            "neighbour-owned sentinel\n",
            "launcher creation must not follow a directory symlink"
        );
        assert!(installed.is_none(), "the unsafe runtime must be refused");
        assert!(!neighbour.path().join(GH_COMPAT_MARKER).exists());
    }

    #[test]
    #[cfg(unix)]
    fn continuation_launcher_pins_terminal_gh_across_child_path_changes() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let bundle = home.path().join("bundle 'quoted'");
        let script = bundle.join(GH_COMPAT_SCRIPT);
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        // This fixture observes the launcher/adapter boundary. It never calls
        // a provider or reads the controller's environment or credentials.
        std::fs::write(
            &script,
            "#!/bin/bash\nexec \"${AMPLIHACK_REAL_GH:?terminal gh was not pinned}\" \"$@\"\n",
        )
        .unwrap();
        let bin = tempfile::tempdir().unwrap();
        let terminal = bin.path().join("gh");
        std::fs::write(&terminal, "#!/bin/sh\nprintf '<%s>\\n' \"$@\"\n").unwrap();
        std::fs::set_permissions(&terminal, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let dir = install_gh_compat_launcher(
            runtime.path(),
            Some(bundle.to_str().unwrap()),
            Some(bin.path().as_os_str()),
        )
        .expect("install with an executable terminal client");
        let private_home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(dir.join("gh"))
            .args(["issue", "comment", "7", "--body", "two words; $(literal)"])
            .env_clear()
            .env("HOME", private_home.path())
            .env("TMPDIR", private_home.path())
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "launcher lost the selected terminal client: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "<issue>\n<comment>\n<7>\n<--body>\n<two words; $(literal)>\n"
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
