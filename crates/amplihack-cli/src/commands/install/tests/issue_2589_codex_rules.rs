//! Issue #2589 (TDD): `amplihack install` writes the Codex execpolicy rules
//! file into each Codex home amplihack manages (F7, T7, T8).
//!
//! Expected seam, re-exported from `codex_plugin`:
//!
//! ```ignore
//! pub(super) fn install_deny_rules() -> DenyRulesWrite;    // resolves CODEX_HOME
//! pub(super) fn uninstall_deny_rules() -> DenyRulesRemoval;
//! ```
//!
//! `CODEX_HOME` and `HOME` are temp directories throughout. Nothing here
//! touches the developer's `~/.codex` (T1, #2586).

use super::super::codex_plugin::{
    DenyRulesRemoval, DenyRulesWrite, RULES_DIR, RULES_FILE, install_deny_rules,
    uninstall_deny_rules,
};
use crate::policy;
use crate::test_support::{env_lock, tempdir_outside_framework_roots};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
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

/// A temp HOME with a temp Codex home inside it, both exported.
struct Fx {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    codex: PathBuf,
}

impl Fx {
    fn new() -> Self {
        let tmp = tempdir_outside_framework_roots();
        let home = tmp.path().join("home");
        let codex = home.join(".codex");
        fs::create_dir_all(&codex).unwrap();
        Self {
            _tmp: tmp,
            home,
            codex,
        }
    }

    fn rules_file(&self) -> PathBuf {
        self.codex.join(RULES_DIR).join(RULES_FILE)
    }

    fn env(&self) -> Env {
        Env::set(&[
            ("HOME", Some(&self.home)),
            ("CODEX_HOME", Some(&self.codex)),
        ])
    }

    fn instance_config(&self, entries: &[&str]) {
        let dir = self.home.join(".amplihack");
        fs::create_dir_all(&dir).unwrap();
        let list = entries
            .iter()
            .map(|e| format!("\"{e}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let path = dir.join("config.toml");
        fs::write(&path, format!("[policy]\ndeny_commands = [{list}]\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

fn fingerprint(path: &Path) -> (Vec<u8>, std::time::SystemTime, u64) {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::symlink_metadata(path).unwrap();
    (
        fs::read(path).unwrap(),
        meta.modified().unwrap(),
        meta.ino(),
    )
}

#[test]
fn install_writes_the_rules_file_into_the_codex_home_it_manages() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    let _env = fx.env();

    assert_eq!(install_deny_rules(), DenyRulesWrite::Written);
    assert!(fx.rules_file().exists(), "{}", fx.rules_file().display());
}

/// T7: forbidden prefix rules, each with justification text, and `not_match`
/// examples covering the ordinary push forms.
#[test]
fn the_rules_file_forbids_the_pattern_kills_with_advice_and_spares_ordinary_pushes() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    let _env = fx.env();
    install_deny_rules();

    let text = fs::read_to_string(fx.rules_file()).unwrap();
    for token in ["\"pkill\"", "\"killall\"", "\"pgrep\""] {
        assert!(text.contains(token), "{token} missing from:\n{text}");
    }
    assert_eq!(
        text.matches("decision = \"forbidden\"").count(),
        policy::DEFAULTS.len(),
        "one forbidden rule per entry:\n{text}"
    );
    assert!(
        text.contains("ps -ef") || text.contains("ps aux"),
        "the justification routes to `ps`, never back to a denied command:\n{text}"
    );
    assert!(
        !text.contains("pgrep -af"),
        "D6: the old advice is gone:\n{text}"
    );
    for ordinary in ["git push -u origin main", "git push origin HEAD:topic"] {
        assert!(
            text.contains(ordinary),
            "{ordinary:?} must be a not_match example:\n{text}"
        );
    }
}

#[test]
fn a_second_install_leaves_the_rules_file_byte_identical() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    let _env = fx.env();

    install_deny_rules();
    let before = fingerprint(&fx.rules_file());
    assert_eq!(install_deny_rules(), DenyRulesWrite::AlreadyCurrent);
    assert_eq!(fingerprint(&fx.rules_file()), before);
}

/// T8/R15: amplihack replaces only its own file.
#[test]
fn an_unrelated_file_already_in_the_rules_directory_is_untouched() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    let _env = fx.env();
    let dir = fx.codex.join(RULES_DIR);
    fs::create_dir_all(&dir).unwrap();
    let other = dir.join("no-pattern-kill.rules");
    fs::write(&other, "# applied by hand\n").unwrap();
    let before = fingerprint(&other);

    install_deny_rules();
    assert_eq!(fingerprint(&other), before);
    assert!(
        fx.rules_file().exists(),
        "amplihack's own file is written too"
    );
}

/// T6: an instance addition joins the defaults in the Codex rendering too.
#[test]
fn an_instance_addition_renders_a_rule_alongside_the_defaults() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    fx.instance_config(&["shutdown"]);
    let _env = fx.env();

    install_deny_rules();
    let text = fs::read_to_string(fx.rules_file()).unwrap();
    assert!(text.contains("\"shutdown\""), "{text}");
    assert!(text.contains("\"pkill\""), "the defaults stay:\n{text}");
    assert_eq!(
        text.matches("prefix_rule(").count(),
        policy::DEFAULTS.len() + 1
    );
}

/// SEC-A1: a project file may not add deny entries. The key is user-only, so
/// a cloned repository cannot change what any harness denies.
#[test]
fn a_project_layer_entry_reaches_no_rendering() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    let project = fx._tmp.path().join("project");
    fs::create_dir_all(project.join(".amplihack")).unwrap();
    fs::create_dir_all(project.join(".git")).unwrap();
    fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(
        project.join(".amplihack/config.toml"),
        "[policy]\ndeny_commands = [\"git-commit\"]\n",
    )
    .unwrap();
    // A guard, not a hand-rolled pair: the assertion below panics on failure
    // and a leaked cwd would follow every later test in this binary.
    let _cwd = crate::test_support::CwdGuard::set(&project).unwrap();
    let _env = fx.env();

    install_deny_rules();
    let text = fs::read_to_string(fx.rules_file()).unwrap();
    assert!(
        !text.contains("git-commit"),
        "a project file must not add a deny entry:\n{text}"
    );
}

#[test]
fn with_no_codex_home_variable_the_file_goes_under_the_temp_home() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    let _env = Env::set(&[("HOME", Some(&fx.home)), ("CODEX_HOME", None)]);

    install_deny_rules();
    assert!(
        fx.home
            .join(".codex")
            .join(RULES_DIR)
            .join(RULES_FILE)
            .exists(),
        "the default Codex home is resolved from HOME, which is temporary here"
    );
}

#[test]
fn uninstall_removes_the_file_amplihack_wrote() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    let _env = fx.env();

    install_deny_rules();
    assert_eq!(uninstall_deny_rules(), DenyRulesRemoval::Removed);
    assert!(!fx.rules_file().exists());
}

/// Fail-safe: the shipped defaults live in code, so a rejected instance entry
/// loses only itself. A bad line in the user file must not quietly disable
/// the whole policy, which would be worse than having none because it would
/// be believed.
#[test]
fn an_invalid_instance_entry_does_not_stop_the_defaults_being_written() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    fx.instance_config(&["shell(x)"]);
    let _env = fx.env();

    install_deny_rules();
    let text = fs::read_to_string(fx.rules_file()).unwrap();
    assert!(
        !text.contains("shell(x)"),
        "the bad entry is dropped:\n{text}"
    );
    assert_eq!(
        text.matches("decision = \"forbidden\"").count(),
        policy::DEFAULTS.len(),
        "every default still renders:\n{text}"
    );
}

/// SEC-A1: a user file the group or another user can write is not trusted for
/// a user-only key, and again the defaults are unaffected.
#[test]
fn an_insecure_user_config_adds_nothing_while_the_defaults_still_write() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let fx = Fx::new();
    fx.instance_config(&["shutdown"]);
    fs::set_permissions(
        fx.home.join(".amplihack/config.toml"),
        fs::Permissions::from_mode(0o666),
    )
    .unwrap();
    let _env = fx.env();

    install_deny_rules();
    let text = fs::read_to_string(fx.rules_file()).unwrap();
    assert!(
        !text.contains("\"shutdown\""),
        "an insecure user file adds no entry:\n{text}"
    );
    assert!(
        text.contains("\"pkill\""),
        "the defaults still write:\n{text}"
    );
}
