//! Native RustyClawd launcher path.
//!
//! The Python implementation mostly detects a preferred Rust-native Claude
//! binary and then reuses the standard Claude launch flow. This module does
//! the same without delegating to `python3 -m amplihack.cli`.

use crate::commands::launch;
use amplihack_utils::launch_target::OverrideOrigin;
use anyhow::Result;
use std::env;
use std::path::{Path, PathBuf};

pub fn run_rustyclawd(args: Vec<String>, no_reflection: bool, subprocess_safe: bool) -> Result<()> {
    let override_origin = configure_preferred_rustyclawd_binary();
    if override_origin == OverrideOrigin::AmplihackSupplied {
        println!("Using RustyClawd (Rust implementation)");
    }

    launch::run_launch(
        "claude",
        "claude",
        false,
        false,
        false,
        true,
        false,
        no_reflection,
        subprocess_safe,
        None,
        args,
        // Issue #1276: the value that used to be a process-global latch. It
        // travels with the call that needs it instead of with the process, so
        // the compiler names every site that must honour it.
        override_origin,
    )
}

/// Point `AMPLIHACK_CLAUDE_BINARY_PATH` at a preferred RustyClawd binary, if
/// there is one, and report which kind of override now stands.
///
/// Issue #1266: an override amplihack sets for itself is a *preference*, not
/// the user's instruction. A user-supplied override that fails the health gate
/// is a hard error; this one warns and falls through, so a broken
/// `rustyclawd` on `$PATH` cannot turn a working `amplihack rustyclawd` into a
/// failed launch.
///
/// Issue #1276: that distinction used to be recorded in a process-global
/// one-way latch inside `launch_target`. It is returned instead. Not a second
/// environment variable, for the original reason — an env marker would be
/// inherited by a nested `amplihack` and would silently demote a real user
/// override into a preference.
///
/// [`OverrideOrigin::User`] is the answer when nothing was set: whatever is in
/// the environment, if anything, is the user's and is not amplihack's to
/// reinterpret.
pub(crate) fn configure_preferred_rustyclawd_binary() -> OverrideOrigin {
    let preferred = find_preferred_rustyclawd_binary();
    if let Some(path) = preferred.as_deref() {
        unsafe { env::set_var("AMPLIHACK_CLAUDE_BINARY_PATH", path) };
    }
    origin_for(preferred.as_deref())
}

/// Which [`OverrideOrigin`] a preferred-binary lookup implies.
///
/// Pure, and split out from the writer above for the reason issue #1276 is
/// about: the mapping is the whole behaviour, and the only way to exercise it
/// through `configure_preferred_rustyclawd_binary` is to write
/// `AMPLIHACK_CLAUDE_BINARY_PATH` into the process, where it outlives the test
/// and changes what every sibling resolves. The old process-global latch had
/// exactly that problem and exactly this consequence: no test called it.
fn origin_for(preferred: Option<&Path>) -> OverrideOrigin {
    match preferred {
        // amplihack chose this one, so a broken value is a preference to fall
        // through, not an instruction to fail on.
        Some(_) => OverrideOrigin::AmplihackSupplied,
        // Nothing was set on amplihack's behalf. Whatever is in the
        // environment, if anything, is the user's and is not amplihack's to
        // reinterpret — and `User` is the strict arm, so the failure mode of
        // getting this wrong is a loud error rather than a silent
        // substitution.
        None => OverrideOrigin::User,
    }
}

fn find_preferred_rustyclawd_binary() -> Option<PathBuf> {
    if let Ok(custom_path) = env::var("RUSTYCLAWD_PATH") {
        let path = PathBuf::from(custom_path);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    find_in_path(&["rustyclawd", "claude-code"])
}

fn find_in_path(names: &[&str]) -> Option<PathBuf> {
    // Issue #1266 / F-S2, now via the one seam (#1274): POSIX reads an empty
    // `$PATH` element as the current directory, so a bare `split_paths` hands
    // back `""` for a stray colon and `dir.join(name)` becomes a bare relative
    // name stat'd against amplihack's cwd. This value is written to
    // `AMPLIHACK_CLAUDE_BINARY_PATH` and becomes an `ExplicitOverride`
    // candidate. `cheap_reject` does answer `NotAbsolute` for it, so the
    // funnel already contains the damage — this stops it being produced at
    // all. The rule used to be copied out at four sites; it is now stated once
    // in `launch_target::path_dirs`.
    for dir in amplihack_utils::launch_target::env_path_dirs() {
        for name in names {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Issue #1276 — the origin the launch is threaded is a function of what
    /// was found, and it is pinned without writing the process environment.
    #[test]
    fn a_preferred_binary_makes_the_override_amplihacks_own_preference() {
        assert_eq!(
            origin_for(Some(Path::new("/opt/bin/rustyclawd"))),
            OverrideOrigin::AmplihackSupplied,
            "amplihack set AMPLIHACK_CLAUDE_BINARY_PATH itself, so a broken \
             value must warn and fall through rather than fail the launch — \
             the regression that shipped green when this lived in a \
             process-global latch"
        );
    }

    #[test]
    fn no_preferred_binary_leaves_any_override_as_the_users_instruction() {
        assert_eq!(
            origin_for(None),
            OverrideOrigin::User,
            "amplihack wrote nothing, so an override in the environment is the \
             user's and a broken one is a hard error"
        );
    }

    #[test]
    fn finds_custom_rustyclawd_path() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("rustyclawd-custom");
        fs::write(&binary, "#!/usr/bin/env bash\n").unwrap();
        let mut perms = fs::metadata(&binary).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary, perms).unwrap();

        let previous = env::var_os("RUSTYCLAWD_PATH");
        unsafe { env::set_var("RUSTYCLAWD_PATH", &binary) };

        let found = find_preferred_rustyclawd_binary();

        match previous {
            Some(value) => unsafe { env::set_var("RUSTYCLAWD_PATH", value) },
            None => unsafe { env::remove_var("RUSTYCLAWD_PATH") },
        }

        assert_eq!(found.as_deref(), Some(binary.as_path()));
    }

    #[test]
    fn finds_rustyclawd_before_claude_code_on_path() {
        let _guard = crate::test_support::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let rustyclawd = dir.path().join("rustyclawd");
        let claude_code = dir.path().join("claude-code");

        for binary in [&rustyclawd, &claude_code] {
            fs::write(binary, "#!/usr/bin/env bash\n").unwrap();
            let mut perms = fs::metadata(binary).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(binary, perms).unwrap();
        }

        let previous_path = env::var_os("PATH");
        let previous_custom = env::var_os("RUSTYCLAWD_PATH");
        unsafe {
            env::set_var("PATH", dir.path());
            env::remove_var("RUSTYCLAWD_PATH");
        }

        let found = find_preferred_rustyclawd_binary();

        match previous_path {
            Some(value) => unsafe { env::set_var("PATH", value) },
            None => unsafe { env::remove_var("PATH") },
        }
        match previous_custom {
            Some(value) => unsafe { env::set_var("RUSTYCLAWD_PATH", value) },
            None => unsafe { env::remove_var("RUSTYCLAWD_PATH") },
        }

        assert_eq!(found.as_deref(), Some(rustyclawd.as_path()));
    }
}
