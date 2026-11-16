//! Issue #2589 (TDD): `amplihack update` reaches the policy writers, and a
//! failed write is loud rather than silent (F12, R16, SEC-C3).
//!
//! Expected seam, in `install/mod.rs`:
//!
//! ```ignore
//! /// Applies the dangerous-command policy to every harness amplihack
//! /// manages. Never fatal: a harness that could not be written returns a
//! /// warning line and the install carries on.
//! pub(super) fn apply_dangerous_command_policy() -> PolicyReport;
//!
//! pub(super) struct PolicyReport { pub lines: Vec<String>, pub warnings: Vec<String> }
//! ```
//!
//! Visibility this file needs: `update::post_install` becomes `pub(crate)`
//! so the post-update path can be driven from here, and `policy` items are
//! `pub(crate)`.
//!
//! Two things are checked separately, because they fail for different
//! reasons: that the function does the work (behaviour, against temp homes),
//! and that both install entry points call it (structure, against the source
//! of `install/mod.rs` — driving the whole installer in-process needs a
//! framework source this test binary does not have).

use super::super::apply_dangerous_command_policy;
use super::super::codex_plugin::{RULES_DIR, RULES_FILE};
use crate::policy;
use crate::test_support::{env_lock, tempdir_outside_framework_roots};
use crate::update::post_install::run_post_update_install;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

struct Env(Vec<(&'static str, Option<OsString>)>);

impl Env {
    fn set(vars: &[(&'static str, Option<&Path>)]) -> Self {
        let mut previous = Vec::new();
        for (key, value) in vars {
            previous.push((*key, std::env::var_os(key)));
            // SAFETY: edition 2024 requires unsafe; callers hold env_lock().
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
        Self(previous)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..) {
            // SAFETY: as in `set`.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

fn temp_homes() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempdir_outside_framework_roots();
    let home = tmp.path().join("home");
    let codex = home.join(".codex");
    fs::create_dir_all(&codex).unwrap();
    (tmp, home, codex)
}

#[test]
fn applying_the_policy_writes_the_codex_rules_file() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (_tmp, home, codex) = temp_homes();
    let _env = Env::set(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(codex.join(RULES_DIR).join(RULES_FILE).exists());
}

#[test]
fn applying_the_policy_twice_changes_nothing() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (_tmp, home, codex) = temp_homes();
    let _env = Env::set(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    apply_dangerous_command_policy();
    let path = codex.join(RULES_DIR).join(RULES_FILE);
    let before = fs::read(&path).unwrap();
    apply_dangerous_command_policy();
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn the_report_carries_the_install_output_lines() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (_tmp, home, codex) = temp_homes();
    let _env = Env::set(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    let text = report.lines.join("\n");
    for expected in policy::install_report_lines() {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
}

/// SEC-C3: a security control that is not installed must not be silent, and
/// must not fail the install either.
#[test]
fn a_refused_codex_write_warns_and_does_not_fail_the_install() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (tmp, home, codex) = temp_homes();
    let rules = codex.join(RULES_DIR);
    fs::create_dir_all(&rules).unwrap();
    let outside = tmp.path().join("outside.rules");
    fs::write(&outside, "untouched\n").unwrap();
    symlink(&outside, rules.join(RULES_FILE)).unwrap();
    let _env = Env::set(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    let report = apply_dangerous_command_policy();
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    let warning = &report.warnings[0];
    assert!(warning.contains("Codex"), "{warning}");
    assert!(
        warning.contains("dangerous-command"),
        "the warning must say which control is missing: {warning}"
    );
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "untouched\n",
        "and nothing is written through the link"
    );
}

/// The post-update path runs the same installer closure `amplihack update`
/// passes, so the policy is applied on update as well as on install.
#[test]
fn the_post_update_install_path_applies_the_policy() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (_tmp, home, codex) = temp_homes();
    let _env = Env::set(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    run_post_update_install(false, || {
        apply_dangerous_command_policy();
        Ok(())
    })
    .expect("the post-update install runs its installer");
    assert!(codex.join(RULES_DIR).join(RULES_FILE).exists());
}

#[test]
fn skip_install_applies_nothing() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let (_tmp, home, codex) = temp_homes();
    let _env = Env::set(&[("HOME", Some(&home)), ("CODEX_HOME", Some(&codex))]);

    run_post_update_install(true, || {
        apply_dangerous_command_policy();
        Ok(())
    })
    .expect("--skip-install is not an error");
    assert!(!codex.join(RULES_DIR).join(RULES_FILE).exists());
}

/// Structural: both install entry points call the writer. Without this, the
/// behaviour tests above pass while nothing ever calls the function.
#[test]
fn both_install_entry_points_call_the_policy_writer() {
    const SOURCE: &str = include_str!("../mod.rs");
    let calls = SOURCE.matches("apply_dangerous_command_policy()").count();
    assert!(
        calls >= 3,
        "expected the definition and a call from each of `install` and the \
         post-update install path; found {calls} occurrence(s)"
    );
}

/// SEC-A3: a Codex home amplihack does not manage is never created by the
/// policy writer, and its absence is not a warning.
#[test]
fn an_absent_codex_home_is_neither_created_nor_warned_about() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = tempdir_outside_framework_roots();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    let _env = Env::set(&[("HOME", Some(&home)), ("CODEX_HOME", None)]);

    let report = apply_dangerous_command_policy();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(!home.join(".codex").exists());
}
