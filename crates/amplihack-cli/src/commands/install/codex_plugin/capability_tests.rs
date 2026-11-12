//! #1728: `plugin --help` capability check.
use super::probe_tests::*;
use super::*;

// T2: 0.165.0-style help means the subcommand exists.
#[test]
fn plugin_capability_accepts_plugin_help_with_list_command() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let binary = probe_fake(dir.path(), "codex", &help_body(PLUGIN_HELP, 0));
    assert!(plugin_capability(&binary, dir.path()).unwrap());
}

// T3: an older Codex rejects the subcommand with a usage error.
#[test]
fn plugin_capability_rejects_top_level_usage_exit_2() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let body = "printf \"error: unrecognized subcommand 'plugin'\\n\\nUsage: codex [OPTIONS] [PROMPT]\\n\" >&2\nexit 2";
    let binary = probe_fake(dir.path(), "codex", body);
    assert!(!plugin_capability(&binary, dir.path()).unwrap());
}

// A Codex that treats `plugin` as a prompt prints top-level help and exits 0.
#[test]
fn plugin_capability_rejects_top_level_help_exit_0() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let binary = probe_fake(dir.path(), "codex", &help_body(TOP_LEVEL_HELP, 0));
    assert!(!plugin_capability(&binary, dir.path()).unwrap());
}

// A help command that fails for another reason is an error, not missing
// plugin support, so the caller does not advise an update.
#[test]
fn plugin_capability_reports_other_nonzero_exit_as_error() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let binary = probe_fake(dir.path(), "codex", &help_body(PLUGIN_HELP, 1));
    let error = plugin_capability(&binary, dir.path()).unwrap_err();
    let rendered = format!("{error:#}");
    assert!(rendered.contains("exit status: 1"), "{rendered}");
    assert_no_child_output(&rendered);
}

#[test]
fn plugin_capability_reports_exit_2_without_unrecognized_subcommand_as_error() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let body = "printf 'error: config file is invalid\n' >&2\nexit 2";
    let binary = probe_fake(dir.path(), "codex", body);
    let error = plugin_capability(&binary, dir.path()).unwrap_err();
    let rendered = format!("{error:#}");
    assert!(rendered.contains("exit status: 2"), "{rendered}");
    assert_no_child_output(&rendered);
}

// The usage line is matched by subcommand, not by the binary's name.
#[test]
fn plugin_capability_accepts_plugin_help_under_another_binary_name() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let help = PLUGIN_HELP.replace("Usage: codex plugin", "Usage: codex-x86_64 plugin");
    let binary = probe_fake(dir.path(), "codex-x86_64", &help_body(&help, 0));
    assert!(plugin_capability(&binary, dir.path()).unwrap());
}

#[test]
fn plugin_capability_requires_list_as_a_command_line() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // `list` appears only inside prose and as a prefix of another word.
    let help = "Usage: codex plugin [OPTIONS] <COMMAND>\n\nCommands:\n  listing  Show a listing\n  add      Add or list plugins\n";
    let binary = probe_fake(dir.path(), "codex", &help_body(help, 0));
    assert!(!plugin_capability(&binary, dir.path()).unwrap());
}

#[test]
fn plugin_capability_ignores_stderr_help() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let body = format!("cat >&2 <<'HELP'\n{PLUGIN_HELP}HELP\nexit 0");
    let binary = probe_fake(dir.path(), "codex", &body);
    assert!(!plugin_capability(&binary, dir.path()).unwrap());
}

#[test]
fn plugin_capability_runs_plugin_help_with_scoped_home_and_no_json_flag() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("codex-home");
    fs::create_dir_all(&home).unwrap();
    let log = dir.path().join("calls");
    let body = format!(
        "printf '%s|%s\\n' \"$*\" \"$CODEX_HOME\" >> '{}'\n{}",
        log.display(),
        help_body(PLUGIN_HELP, 0)
    );
    let binary = probe_fake(dir.path(), "codex", &body);
    assert!(plugin_capability(&binary, &home).unwrap());
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        format!("plugin --help|{}\n", home.display())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn plugin_capability_gives_child_null_stdin() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let stdin_target = dir.path().join("stdin-target");
    let body = format!(
        "readlink /proc/self/fd/0 > '{}'\n{}",
        stdin_target.display(),
        help_body(PLUGIN_HELP, 0)
    );
    let binary = probe_fake(dir.path(), "codex", &body);
    assert!(plugin_capability(&binary, dir.path()).unwrap());
    assert_eq!(
        fs::read_to_string(&stdin_target).unwrap().trim(),
        "/dev/null"
    );
}

#[test]
fn plugin_capability_reports_spawn_failure_as_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("no-such-codex");
    let error = plugin_capability(&missing, dir.path()).unwrap_err();
    assert_no_child_output(&format!("{error:#}"));
}
