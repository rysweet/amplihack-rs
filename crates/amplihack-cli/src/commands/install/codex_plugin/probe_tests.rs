//! #1728: native plugin inventory probe. amplihack waits for Codex to exit;
//! there is no time limit on any native plugin call.
use super::*;

/// Help text in the shape printed by codex-cli 0.165.0 for `codex plugin --help`.
pub(super) const PLUGIN_HELP: &str = "Manage Codex plugins\n\nUsage: codex plugin [OPTIONS] <COMMAND>\n\nCommands:\n  add          Install a plugin\n  list         List plugins available from configured and remote marketplaces\n  marketplace  Add, list, upgrade, or remove configured plugin marketplaces\n  remove       Uninstall a plugin\n  help         Print this message\n";

/// Top-level help, which a Codex without the `plugin` subcommand prints
/// because it treats `plugin` as a prompt.
pub(super) const TOP_LEVEL_HELP: &str = "Codex CLI\n\nUsage: codex [OPTIONS] [PROMPT]\n       codex [OPTIONS] <COMMAND> [ARGS]\n\nCommands:\n  exec        Run Codex non-interactively\n  login       Manage login\n";

/// Writes a `#!/bin/sh` fake with mode 0700. Every fake writes a canary and
/// an ESC byte to stderr first, so tests can prove no child output escapes.
pub(super) fn probe_fake(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let script = format!(
        "#!/bin/sh\nprintf 'Authorization: Bearer secret-canary\\033[31m\\n' >&2\n{body}\n"
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

/// A help body that prints `text` followed by a stdout canary, then exits.
pub(super) fn help_body(text: &str, code: i32) -> String {
    format!("cat <<'HELP'\n{text}HELP\nprintf 'stdout-canary\\033[31m\\n'\nexit {code}")
}

pub(super) fn assert_no_child_output(rendered: &str) {
    assert!(
        !rendered.contains("canary"),
        "child output leaked: {rendered}"
    );
    assert!(!rendered.contains('\u{1b}'), "ESC byte leaked: {rendered}");
    assert!(
        !rendered.contains("Bearer"),
        "child output leaked: {rendered}"
    );
}

// A cold `codex plugin list --json` took 32.8 s on one host and was misread
// as missing plugin support. A slow Codex must still succeed.
#[test]
fn native_waits_for_slow_codex() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let binary = probe_fake(
        dir.path(),
        "codex",
        "sleep 2\nprintf '{\"installed\":[]}'\nexit 0",
    );
    let value = native(&binary, &["plugin", "list", "--json"], dir.path()).unwrap();
    assert_eq!(value, json!({"installed": []}));
}

#[test]
fn plugin_capability_waits_for_slow_codex() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let binary = probe_fake(
        dir.path(),
        "codex",
        &format!("sleep 2\n{}", help_body(PLUGIN_HELP, 0)),
    );
    assert!(plugin_capability(&binary, dir.path()).unwrap());
}

#[test]
fn native_returns_inventory_and_passes_scoped_codex_home() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("codex-home");
    fs::create_dir_all(&home).unwrap();
    let seen = dir.path().join("seen-home");
    let binary = probe_fake(
        dir.path(),
        "codex",
        &format!(
            "printf '%s' \"$CODEX_HOME\" > '{}'\nprintf '{{\"installed\":[]}}'\nexit 0",
            seen.display()
        ),
    );
    let value = native(&binary, &["plugin", "list", "--json"], &home).unwrap();
    assert_eq!(value, json!({"installed": []}));
    assert_eq!(fs::read_to_string(&seen).unwrap(), home.to_str().unwrap());
}

// V6: a probe with no time limit must not be able to read the user's keystrokes.
#[cfg(target_os = "linux")]
#[test]
fn native_gives_child_null_stdin() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let stdin_target = dir.path().join("stdin-target");
    let binary = probe_fake(
        dir.path(),
        "codex",
        &format!(
            "readlink /proc/self/fd/0 > '{}'\nprintf '{{\"installed\":[]}}'\nexit 0",
            stdin_target.display()
        ),
    );
    native(&binary, &["plugin", "list", "--json"], dir.path()).unwrap();
    assert_eq!(
        fs::read_to_string(&stdin_target).unwrap().trim(),
        "/dev/null"
    );
}

#[test]
fn native_reports_exit_status_without_child_output() {
    let _env_lock = crate::test_support::env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let binary = probe_fake(dir.path(), "codex", "printf 'stdout-canary'\nexit 1");
    let error = native(&binary, &["plugin", "list", "--json"], dir.path()).unwrap_err();
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("native plugin command failed"),
        "{rendered}"
    );
    assert!(rendered.contains("exit status: 1"), "{rendered}");
    assert_no_child_output(&rendered);
}
