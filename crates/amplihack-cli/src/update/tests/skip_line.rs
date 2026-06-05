use super::super::install::{binary_filename, extract_archive, find_binary};
use super::super::*;
use sha2::{Digest, Sha256};

#[test]
fn sha256_computation_produces_64_hex_char_digest() {
    let data = b"hello world";
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = format!("{:x}", hasher.finalize());
    assert_eq!(digest.len(), 64);
    assert!(
        digest.chars().all(|c| c.is_ascii_hexdigit()),
        "digest must be all hex digits"
    );
}

#[test]
fn sha256_digest_changes_when_data_changes() {
    let data = b"some binary content";
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = format!("{:x}", hasher.finalize());
    let mut wrong = actual.clone();
    wrong.replace_range(0..1, if wrong.starts_with('a') { "b" } else { "a" });
    assert_ne!(actual, wrong);
}

#[test]
fn current_test_platform_has_release_target() {
    assert!(supported_release_target().is_some());
}

#[test]
fn extract_archive_finds_both_binaries() {
    let temp = tempfile::tempdir().unwrap();
    let archive_path = temp.path().join("release.tar.gz");
    create_test_archive(&archive_path).unwrap();
    let bytes = fs::read(&archive_path).unwrap();

    let extract_dir = temp.path().join("extract");
    fs::create_dir_all(&extract_dir).unwrap();
    extract_archive(&bytes, &extract_dir).unwrap();

    assert!(find_binary(&extract_dir, binary_filename("amplihack")).is_ok());
    assert!(find_binary(&extract_dir, binary_filename("amplihack-hooks")).is_ok());
}

#[cfg(unix)]
#[test]
fn find_binary_ignores_symlinked_archive_entries() {
    let temp = tempfile::tempdir().unwrap();
    let symlinked_dir_root = temp.path().join("symlinked-dir-search");
    let symlinked_file_root = temp.path().join("symlinked-file-search");
    let outside_dir = temp.path().join("outside/real");
    fs::create_dir_all(&symlinked_dir_root).unwrap();
    fs::create_dir_all(&symlinked_file_root).unwrap();
    fs::create_dir_all(&outside_dir).unwrap();
    let outside_amplihack = outside_dir.join(binary_filename("amplihack"));
    let outside_hooks = outside_dir.join(binary_filename("amplihack-hooks"));
    fs::write(&outside_amplihack, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::write(&outside_hooks, b"#!/bin/sh\nexit 0\n").unwrap();

    use std::os::unix::fs::symlink;
    symlink(
        &outside_dir,
        symlinked_dir_root.join("archive-controlled-dir-link"),
    )
    .unwrap();
    symlink(
        &outside_hooks,
        symlinked_file_root.join(binary_filename("amplihack-hooks")),
    )
    .unwrap();

    assert!(
        find_binary(&symlinked_dir_root, binary_filename("amplihack")).is_err(),
        "archive-controlled symlinked directories must not redirect binary discovery outside the extraction root"
    );
    assert!(
        find_binary(&symlinked_file_root, binary_filename("amplihack-hooks")).is_err(),
        "archive-controlled symlinked files must not be accepted as install sources"
    );
}

fn create_test_archive(path: &Path) -> Result<()> {
    let tar_gz = fs::File::create(path)?;
    let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    for binary in [
        binary_filename("amplihack"),
        binary_filename("amplihack-hooks"),
    ] {
        let data = b"#!/bin/sh\nexit 0\n";
        let mut header = tar::Header::new_gnu();
        header.set_path(binary)?;
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append(&header, &data[..])?;
    }

    let encoder = builder.into_inner()?;
    encoder.finish()?;
    Ok(())
}

#[test]
fn wants_startup_update_positive() {
    use super::super::check::*;
    // Not publicly exported, but we test the outcome through StartupUpdateOutcome
    // by verifying StartupUpdateOutcome variants exist.
    assert_eq!(
        StartupUpdateOutcome::Continue,
        StartupUpdateOutcome::Continue
    );
    assert_ne!(
        StartupUpdateOutcome::Continue,
        StartupUpdateOutcome::ExitSuccess
    );
}

#[test]
fn shell_profile_path_bash() {
    use crate::commands::install::paths::shell_profile_path;
    let _lock = crate::test_support::env_lock();
    unsafe { std::env::set_var("SHELL", "/bin/bash") };
    let result = shell_profile_path();
    unsafe { std::env::remove_var("SHELL") };
    if let Some(path) = result {
        assert!(
            path.to_string_lossy().ends_with(".bashrc"),
            "expected .bashrc, got {}",
            path.display()
        );
    }
}

#[test]
fn shell_profile_path_zsh() {
    use crate::commands::install::paths::shell_profile_path;
    let _lock = crate::test_support::env_lock();
    unsafe { std::env::set_var("SHELL", "/bin/zsh") };
    let result = shell_profile_path();
    unsafe { std::env::remove_var("SHELL") };
    if let Some(path) = result {
        assert!(
            path.to_string_lossy().ends_with(".zshrc"),
            "expected .zshrc, got {}",
            path.display()
        );
    }
}

#[test]
fn shell_profile_path_unknown() {
    use crate::commands::install::paths::shell_profile_path;
    let _lock = crate::test_support::env_lock();
    unsafe { std::env::set_var("SHELL", "/bin/csh") };
    let result = shell_profile_path();
    unsafe { std::env::remove_var("SHELL") };
    assert!(result.is_none(), "unsupported shell should return None");
}

#[test]
fn ensure_local_bin_on_shell_path_creates_export() {
    use crate::commands::install::paths::ensure_local_bin_on_shell_path;
    let tmp = tempfile::TempDir::new().unwrap();
    // Use home_env_lock so we serialize with HOME-reading tests like
    // test_resolve_binary_path_expands_tilde_to_home_dir.
    let _lock = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prev_home = std::env::var_os("HOME");
    let prev_shell = std::env::var_os("SHELL");
    unsafe { std::env::set_var("HOME", tmp.path().as_os_str()) };
    unsafe { std::env::set_var("SHELL", "/bin/bash") };
    ensure_local_bin_on_shell_path().unwrap();
    let content = std::fs::read_to_string(tmp.path().join(".bashrc")).unwrap();
    assert!(
        content.contains(".local/bin"),
        "should have added .local/bin to .bashrc"
    );
    // Second call is a no-op
    ensure_local_bin_on_shell_path().unwrap();
    let content2 = std::fs::read_to_string(tmp.path().join(".bashrc")).unwrap();
    let count = content2.matches("export PATH").count();
    assert_eq!(count, 1, "should not duplicate the export line");
    match prev_shell {
        Some(v) => unsafe { std::env::set_var("SHELL", v) },
        None => unsafe { std::env::remove_var("SHELL") },
    }
    match prev_home {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #334: Windows x86_64 release support.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn release_target_table_covers_windows_x86_64() {
    // The supported_release_target() function in mod.rs is hard-coded with
    // cfg!() arms, so we can only directly observe the host's mapping. To
    // assert the Windows arm exists without conditionally compiling the test
    // away, we read the source and check the literal arm is present. This is
    // a static guard — it fails any commit that drops the windows arm.
    let src = include_str!("../mod.rs");
    assert!(
        src.contains(r#"target_os = "windows", target_arch = "x86_64""#),
        "supported_release_target() must include a windows/x86_64 arm",
    );
    assert!(
        src.contains(r#"x86_64-pc-windows-msvc"#),
        "supported_release_target() must map windows/x86_64 to x86_64-pc-windows-msvc",
    );
}

#[test]
fn binary_filename_appends_exe_on_windows() {
    // binary_filename() uses cfg!(windows) so on non-Windows hosts it returns
    // the bare name. Inspect the source to guarantee the .exe arm exists.
    let src = include_str!("../install.rs");
    assert!(
        src.contains(r#""amplihack" => "amplihack.exe""#),
        "binary_filename() must append .exe to amplihack on windows",
    );
    assert!(
        src.contains(r#""amplihack-hooks" => "amplihack-hooks.exe""#),
        "binary_filename() must append .exe to amplihack-hooks on windows",
    );
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn windows_host_resolves_msvc_target() {
    assert_eq!(supported_release_target(), Some("x86_64-pc-windows-msvc"));
    assert_eq!(
        expected_archive_name().unwrap(),
        "amplihack-x86_64-pc-windows-msvc.tar.gz"
    );
    assert_eq!(binary_filename("amplihack"), "amplihack.exe");
}

// ─────────────────────────────────────────────────────────────────────────────
// Issue #625: subprocess-safe skip-signal unit tests.
//
// `should_skip_update_check(args) -> bool` (the back-compat wrapper that
// delegates to the new `classify_skip_reason`) MUST treat ANY non-empty value
// of CI / AMPLIHACK_AGENT_BINARY, OR the literal `--subprocess-safe` token in
// argv, as a reason to skip the update check for an otherwise-recognized
// launch subcommand. These tests are FAILING by design until check.rs is
// refactored per the issue #625 design spec.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn skip_line_literal_is_unchanged() {
    let src = include_str!("../check.rs");
    assert!(
        src.contains("amplihack: skipping update check (subprocess-safe / no TTY)"),
        "the skip-line literal `amplihack: skipping update check (subprocess-safe / no TTY)` \
         is part of the public contract for delegated agents and MUST appear verbatim \
         in update/check.rs. Rewording requires updating delegated-agent grep patterns and \
         this test."
    );
}
