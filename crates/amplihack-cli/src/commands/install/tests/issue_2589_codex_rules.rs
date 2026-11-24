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
use crate::test_support::{env_lock, env_paths, tempdir_outside_framework_roots};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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

    fn env(&self) -> crate::test_support::EnvGuard {
        env_paths(&[
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
    let _env = env_paths(&[("HOME", Some(&fx.home)), ("CODEX_HOME", None)]);

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

// --- what Codex itself makes of the file (#1453, repeated) ------------------
//
// Every other test in this file reads back what amplihack wrote and asserts
// it matches what amplihack meant to write. That is the shape of test issue
// #1453 was filed about: the Codex config tests passed with the wrong
// directory, the wrong format and the wrong key, because none of them ever
// asked Codex. The fix there was `bootstrap::codex_reads_the_config_amplihack
// _writes`, and this is the same test for the rules file, which carries more
// than the config did: a `.rules` file Codex cannot parse stops Codex
// starting at all.
//
// The vendor behaviour it rests on, checked against codex-cli 0.166.0:
// `codex execpolicy check --rules <path> -- <tokens...>` exits 0 for both
// decisions and prints the verdict on stdout, so the assertion reads stdout.
// A forbidden command gives `{"matchedRules":[{..}],"decision":"forbidden"}`
// and an unmatched one `{"matchedRules":[]}`. A file that does not parse
// exits 1 with `Error: failed to parse policy at <path>` on stderr.

/// What Codex decides about `command`, or `None` when no rule matched it.
///
/// Panics naming `rules` if Codex could not load the file, which is the
/// failure with the blast radius.
fn codex_decision(codex: &Path, rules: &Path, command: &str) -> Option<String> {
    let output = std::process::Command::new(codex)
        .arg("execpolicy")
        .arg("check")
        .arg("--rules")
        .arg(rules)
        .arg("--")
        .args(command.split_whitespace())
        .output()
        .expect("failed to run codex execpolicy check");
    assert!(
        output.status.success(),
        "codex could not load {}: {}{}",
        rules.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict: serde_json::Value =
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout:?}"));
    let matched = verdict["matchedRules"]
        .as_array()
        .unwrap_or_else(|| panic!("no matchedRules array in {stdout:?}"));
    if matched.is_empty() {
        return None;
    }
    Some(
        verdict["decision"]
            .as_str()
            .unwrap_or_else(|| panic!("a match with no decision: {stdout:?}"))
            .to_string(),
    )
}

/// Skips with a message when the Codex binary is absent, and never fails for
/// that reason. Both installs happen inside this one test so the skip is
/// written once.
#[test]
fn codex_loads_the_rules_file_amplihack_writes_and_decides_as_the_examples_promise() {
    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let Some(codex) = crate::bootstrap::which("codex") else {
        eprintln!(
            "skipping codex_loads_the_rules_file_amplihack_writes_and_decides_as_the_examples_promise: codex is not installed"
        );
        return;
    };

    // 1. The shipped defaults, with no instance config. Every `match`
    //    example must be forbidden and every `not_match` example unmatched,
    //    which is the promise the reference page makes command by command.
    {
        let fx = Fx::new();
        let _env = fx.env();
        assert_eq!(install_deny_rules(), DenyRulesWrite::Written);
        let rules = fx.rules_file();

        for entry in policy::DEFAULTS {
            for example in entry.match_examples {
                assert_eq!(
                    codex_decision(&codex, &rules, example).as_deref(),
                    Some("forbidden"),
                    "entry {}: Codex must forbid the match example {example:?}",
                    entry.id
                );
            }
            for example in entry.not_match_examples {
                assert_eq!(
                    codex_decision(&codex, &rules, example),
                    None,
                    "entry {}: Codex must leave the not_match example {example:?} unmatched",
                    entry.id
                );
            }
        }
    }

    // 2. An instance addition. This is the user-controlled half of the
    //    rendering and the one the hand verification exercised least: it is
    //    the only path that reaches `word_pattern` with a value no shipped
    //    constant supplied.
    //
    //    `kill` is deliberate. It is a `not_match` example of the `pkill` and
    //    `killall` entries, so adding it puts a rule in the file that denies
    //    a command another rule promises to spare. The file loading anyway is
    //    what makes `policy/tests.rs`'s assumption concrete: Codex validates
    //    `not_match` per rule, so an addition cannot render the file
    //    unloadable and stop Codex starting. Were it validated across the
    //    file, this install would be a denial of service any user could
    //    cause with one config line.
    {
        let fx = Fx::new();
        fx.instance_config(&["shutdown", "kill"]);
        let _env = fx.env();
        assert_eq!(install_deny_rules(), DenyRulesWrite::Written);
        let rules = fx.rules_file();

        for command in ["shutdown -h now", "kill 1234"] {
            assert_eq!(
                codex_decision(&codex, &rules, command).as_deref(),
                Some("forbidden"),
                "an instance addition must reach Codex's own decision: {command:?}"
            );
        }
        assert_eq!(
            codex_decision(&codex, &rules, "pkill -f vitest").as_deref(),
            Some("forbidden"),
            "and an addition displaces no default"
        );
        assert_eq!(
            codex_decision(&codex, &rules, "ps -ef"),
            None,
            "nor does it widen one"
        );
    }
}
