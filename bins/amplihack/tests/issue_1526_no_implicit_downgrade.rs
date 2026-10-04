//! Issue #1526: an ordinary command run from an older amplihack build must not
//! deploy that build over a newer install.
//!
//! The reporter ran `amplihack recipe run` from a `cargo install --git` build of
//! the v0.18.39 commit, which reported `0.18.0`. Startup self-heal saw that the
//! stamp (`0.18.39`) differed from its own version and re-staged anyway:
//! `~/.local/bin/amplihack{,-hooks}` were overwritten and
//! `~/.claude/settings.json` was rewritten, with no install command given.
//!
//! This drives the real binary against a temp HOME seeded with a newer install
//! (stamp `9999.0.0`) and checks that nothing a re-stage would touch has
//! changed, and that stderr carries exactly one refusal line.
//!
//! `amplihack version` is the trigger because it reaches self-heal (only
//! `--version`/`-V` are skipped) and nothing else. `recipe run` is covered by
//! the self_heal unit tests, since here it could go on to look up the recipe
//! runner or reach the network.
//!
//! On origin/main this test fails: `deploy_binaries` honours
//! `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH`, so the re-stage succeeds and
//! visibly replaces the sentinels.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;

const NEWER_STAMP: &str = "9999.0.0";
const REFUSAL_PREFIX: &str = "amplihack: refusing implicit re-stage:";

/// Same formula as `cli_golden::version_format_is_semver`.
fn expected_version() -> String {
    match option_env!("AMPLIHACK_RELEASE_VERSION") {
        Some(v) => v.to_string(),
        None => format!("{}-dev", env!("CARGO_PKG_VERSION")),
    }
}

fn write_mode(path: &Path, contents: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

/// Every path under `root` (relative), without following symlinks.
fn all_paths(root: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                walk(root, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

/// Settings and install backups a re-stage writes. Searched recursively: the
/// install backup really lands under `~/.amplihack/.claude/runtime/sessions/`.
fn backups_under(root: &Path) -> Vec<PathBuf> {
    all_paths(root)
        .into_iter()
        .filter(|rel| {
            let name = rel
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            name.starts_with("settings.json.backup.")
                || (name.starts_with("install_") && name.ends_with("_backup.json"))
        })
        .collect()
}

#[test]
fn version_from_older_build_does_not_redeploy_or_rewrite_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let outside = tmp.path().join("outside");
    let cargo_home = tmp.path().join("cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();

    // ── A newer install, as the reporter had. ──
    let settings = home.join(".claude").join("settings.json");
    let bin_amplihack = home.join(".local").join("bin").join("amplihack");
    let bin_hooks = home.join(".local").join("bin").join("amplihack-hooks");
    let stamp = home.join(".amplihack").join(".installed-version");
    let sentinels: BTreeMap<&PathBuf, Vec<u8>> = [
        (&settings, br#"{"sentinel":true}"#.to_vec(), 0o644),
        (
            &bin_amplihack,
            b"#!/bin/sh\necho sentinel-installed-amplihack\n".to_vec(),
            0o755,
        ),
        (
            &bin_hooks,
            b"#!/bin/sh\necho sentinel-installed-amplihack-hooks\n".to_vec(),
            0o755,
        ),
        (&stamp, NEWER_STAMP.as_bytes().to_vec(), 0o600),
    ]
    .into_iter()
    .map(|(path, bytes, mode)| {
        write_mode(path, &bytes, mode);
        (path, bytes)
    })
    .collect();

    // A hooks binary outside ~/.local/bin, so a re-stage on origin/main has
    // something to deploy and succeeds — making the regression visible as a
    // changed sentinel rather than as an unrelated install error.
    let outside_hooks = outside.join("amplihack-hooks");
    write_mode(
        &outside_hooks,
        b"#!/bin/sh\necho outside-stub-amplihack-hooks\n",
        0o755,
    );

    // ── A hermetic environment (C1). ──
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_amplihack"));
    let mut cmd = Command::new(&bin);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("AMPLIHACK_") {
            cmd.env_remove(&key);
        }
    }
    for key in [
        "CLAUDECODE",
        "CLAUDE_CONFIG_DIR",
        "COPILOT_HOME",
        "CODEX_HOME",
        "RUST_LOG",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("HOME", &home)
        .env("CARGO_HOME", &cargo_home)
        .env("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &outside_hooks)
        .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_RECIPE_RUNNER_INSTALL", "1")
        .env("AMPLIHACK_SKIP_MMDC", "1")
        .current_dir(tmp.path())
        .arg("version")
        .timeout(Duration::from_secs(60));

    let output = cmd.output().expect("failed to run amplihack version");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "a refusal must not fail the user's command; status={:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status
    );

    // ── Nothing a re-stage touches has changed. ──
    for (path, seeded) in &sentinels {
        let now = fs::read(path).unwrap_or_else(|e| panic!("{} vanished: {e}", path.display()));
        assert_eq!(
            &now,
            seeded,
            "{} was rewritten by an implicit re-stage from an older build (issue #1526)\n\
             was: {:?}\nnow: {:?}\nstderr:\n{stderr}",
            path.display(),
            String::from_utf8_lossy(seeded),
            String::from_utf8_lossy(&now),
        );
    }
    assert_eq!(
        backups_under(&home),
        Vec::<PathBuf>::new(),
        "a refusal must not leave settings or install backups\nstderr:\n{stderr}"
    );

    // ── Exactly one refusal line, naming both versions and both paths. ──
    let expected = expected_version();
    let refusals: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with(REFUSAL_PREFIX))
        .collect();
    assert_eq!(
        refusals.len(),
        1,
        "expected exactly one refusal line on stderr; got:\n{stderr}"
    );
    let line = refusals[0];
    assert!(
        line.contains(&format!("v{NEWER_STAMP}")),
        "refusal must name the installed version: {line}"
    );
    assert!(
        line.contains(&format!("v{expected}")),
        "refusal must name the running version v{expected}: {line}"
    );
    assert!(
        line.contains(&format!("{bin_amplihack:?}")),
        "refusal must name the installed binary {bin_amplihack:?}: {line}"
    );
    // `current_exe` may or may not resolve symlinks depending on the platform.
    let running_raw = format!("{bin:?}");
    let running_canonical = format!("{:?}", fs::canonicalize(&bin).unwrap());
    assert!(
        line.contains(&running_raw) || line.contains(&running_canonical),
        "refusal must name the running binary ({running_raw} or {running_canonical}): {line}"
    );
    assert!(
        line.contains('"'),
        "paths must be Debug-quoted so they cannot forge a line: {line}"
    );
    assert!(
        line.contains("nothing was changed") && line.contains("'amplihack install'"),
        "refusal must say nothing changed and name the explicit escape hatch: {line}"
    );

    // ── stdout is the command's own output, nothing else. ──
    assert!(
        !stdout.contains("refusing implicit re-stage") && !stdout.contains("re-staged"),
        "self-heal output must never reach stdout:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("amplihack-rs {expected}")),
        "`amplihack version` must still run and report v{expected}:\n{stdout}"
    );
}
